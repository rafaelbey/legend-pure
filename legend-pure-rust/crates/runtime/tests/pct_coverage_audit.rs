// Copyright 2026 Goldman Sachs
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Native-coverage diff between the compiled platform's `native function`
//! declarations and the `NativeRegistry::standard()` keyset.
//!
//! Every Pure `native function` declaration in the platform requires a
//! corresponding Rust `impl NativeFunction` registered under the same
//! mangled FQN — without it, any call to that function raises
//! `Cannot resolve function` at compile/dispatch time. This audit walks
//! the platform model, mangles each native's signature, and reports the
//! set that has no Rust backing.
//!
//! Output: a sorted Markdown report at
//! `target/test-output/pct_native_gaps.md`, grouped by package so
//! follow-up coverage work can attack the largest packages first.
//!
//! The test asserts a hard ceiling on the gap count so future regressions
//! (a native dropped from the registry, a new platform declaration without
//! a backing impl) trip a CI failure. Ceiling lowers as natives land —
//! never raise.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use legend_pure_core_platform::platform;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_runtime::native::NativeRegistry;
// Reference every shipped extension crate so the `linkme`
// distributed-slice entries register at link time. Without one of these
// `use`s, cargo-test's linker prunes the crate as unreferenced and
// `NativeRegistry::discovered()` reports the extension's natives as
// Missing — making the audit falsely flag legitimately-implemented
// natives. The renames (`as _`) keep the imports namespace-clean
// without shadowing.
use legend_pure_dsl_mapping_runtime::MappingDSLPopulator as _;
use legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator as _;
use legend_pure_store_relational_runtime::RelationalStoreExtension as _;

/// Hard ceiling on the number of [`GapKind::Missing`] findings — i.e.
/// platform `native function` declarations whose simple name has *no*
/// `NativeFunction` registered under any signature in
/// `NativeRegistry::discovered()`. These are the "the runtime can't
/// possibly call this" gaps that Stream 2b drives down.
///
/// **Baseline measured 2026-05-18 (post 2b third batch):** 3 Missing
/// after `removeOverride` and `rawEvalProperty` landed. The remaining
/// 3 are milestoning (`getAllVersions`, `getAllVersionsInRange`) and
/// tree-mutation (`replaceTreeNode`) — each needs a real subsystem
/// implementation, not aliasing. Ceiling sits at 7 — ~4 of headroom.
/// **Do not raise** — a regression means a native got dropped from
/// the registry or a new platform declaration landed without an impl.
const MISSING_CEILING: usize = 7;

/// Ceiling on [`GapKind::SignatureMismatch`] findings — natives that
/// exist but whose registered key doesn't exactly match the platform's
/// mangled FQN. Dispatch survives today via `find_by_prefix`, but Java
/// parity requires exact-FQN match.
///
/// **Baseline measured 2026-05-18 (post 2b second batch):** 2 Mismatch
/// after the bulk-alias commit aligned 17 keys. The remaining 2 are
/// the milestoning `getAll(Class, Date)` and `getAll(Class, Date, Date)`
/// overloads — `GetAll::execute` rejects Date args by arity, so an
/// alias would silently route to a runtime-error path. Those belong
/// with the milestoning implementation work, not the aliasing batch.
/// Ceiling sits at 5 — ~3 of headroom.
const MISMATCH_CEILING: usize = 5;

/// Severity for an audit finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GapKind {
    /// No native is registered for this simple-name prefix at all —
    /// any call to it raises `Cannot resolve function`. The headline
    /// number to drive down with Stream 2b.
    Missing,
    /// A native is registered for the simple name, but under a
    /// different mangled signature. The runtime currently dispatches
    /// via `NativeRegistry::find_by_prefix`, which silently absorbs
    /// the divergence. Java-parity requires exact-FQN matching, so
    /// these are latent bugs even when dispatch happens to work
    /// today — but they don't block the call path.
    SignatureMismatch,
}

/// One audit finding (Missing or SignatureMismatch).
#[derive(Debug, Clone)]
struct Gap {
    /// Pure-source FQN, e.g. `meta::pure::functions::math::pow`.
    user_path: String,
    /// Mangled signature key the platform declared, e.g.
    /// `pow_Number_1__Number_1__Number_1_` — what
    /// `NativeRegistry::get(...)` was queried with.
    mangled_fqn: String,
    /// `::`-joined package prefix used for grouping in the report,
    /// e.g. `meta::pure::functions::math`. Empty when the function
    /// lives at the root.
    package: String,
    /// Severity classification.
    kind: GapKind,
}

#[test]
fn pct_native_coverage_audit_under_ceiling() {
    use std::fmt::Write as _;

    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };
    // `discovered()` pulls in every `#[distributed_slice(RUNTIME_EXTENSIONS)]`
    // contribution alongside the platform-standard natives — same
    // composition the production `Evaluator::new_default(...)` uses, and
    // what `evaluator.evaluate(...)` actually dispatches against at
    // runtime. Using `standard()` would falsely flag the entire
    // relational-store extension as Missing.
    let registry = NativeRegistry::discovered();

    let mut gaps: Vec<Gap> = Vec::new();
    let mut total_natives = 0usize;

    for chunk in model.chunks.iter() {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Function(func) = element else {
                continue;
            };
            if !func.is_native {
                continue;
            }
            total_natives += 1;
            // `node.name` for native functions carries the already-
            // mangled key (`plus_Integer_MANY__Integer_1_`), matching
            // the shape used by `registry.register(name, fn)` call
            // sites. The runtime's dispatch path looks up the function
            // under this exact key, so the audit queries it directly.
            let node = chunk.nodes.get(local_idx);
            let mangled = node.name.clone();
            if registry.get(&mangled).is_some() {
                continue;
            }
            // Not an exact-key hit. Probe by simple-name prefix to
            // distinguish "no native at all" (Missing) from "registered
            // under a different signature" (SignatureMismatch).
            let simple_name = simple_name_of(&mangled);
            let kind = if registry.find_by_prefix(simple_name).is_some() {
                GapKind::SignatureMismatch
            } else {
                GapKind::Missing
            };
            let user_path = render_element_path(&model, &node.parent_package, &mangled);
            let package = render_package_path(&model, &node.parent_package);
            gaps.push(Gap {
                user_path,
                mangled_fqn: mangled.to_string(),
                package,
                kind,
            });
        }
    }

    let total_gaps = gaps.len();
    let missing_count = gaps.iter().filter(|g| g.kind == GapKind::Missing).count();
    let mismatch_count = gaps.iter().filter(|g| g.kind == GapKind::SignatureMismatch).count();
    let coverage_pct = if total_natives == 0 {
        100.0
    } else {
        let covered = (total_natives - total_gaps) as f64;
        100.0 * covered / total_natives as f64
    };

    // Group gaps by package; preserve alphabetical sort within each
    // group for deterministic report output.
    let mut by_package: BTreeMap<String, Vec<Gap>> = BTreeMap::new();
    for gap in &gaps {
        by_package
            .entry(gap.package.clone())
            .or_default()
            .push(gap.clone());
    }
    for group in by_package.values_mut() {
        group.sort_by(|a, b| {
            // Missing first within a group, then mismatch; alpha tiebreak.
            (a.kind == GapKind::SignatureMismatch, &a.user_path)
                .cmp(&(b.kind == GapKind::SignatureMismatch, &b.user_path))
        });
    }

    // ---- Render Markdown report ----------------------------------------
    let mut report = String::new();
    report.push_str("# PCT native coverage audit\n\n");
    writeln!(
        report,
        "**Platform `native function` declarations:** {total_natives}\n"
    )
    .ok();
    writeln!(
        report,
        "- Covered (exact-FQN match): {covered}",
        covered = total_natives - total_gaps
    )
    .ok();
    writeln!(
        report,
        "- **Missing** (no native at all for the simple name): {missing_count} \
         (ceiling {MISSING_CEILING})"
    )
    .ok();
    writeln!(
        report,
        "- SignatureMismatch (native exists, registered under a different key — \
         dispatch survives via `find_by_prefix`): {mismatch_count} (ceiling {MISMATCH_CEILING})"
    )
    .ok();
    writeln!(report, "- Coverage: {coverage_pct:.1}%\n").ok();

    if total_gaps == 0 {
        report.push_str("All platform native declarations are backed by registered ");
        report.push_str("`NativeFunction` impls under the exact mangled key — nothing to attack.\n");
    } else {
        report.push_str("## Gaps by package\n\n");
        report.push_str("Largest groups first. **Missing** rows (`M`) are the priority — ");
        report.push_str("the runtime cannot resolve those calls at all. **SignatureMismatch** ");
        report.push_str("rows (`S`) work today via `find_by_prefix` but diverge from Java's ");
        report.push_str("exact-FQN dispatch contract; fix the registration key to align.\n\n");

        // Sort groups by Missing count first (largest gaps first), then by
        // total gap count, then by package name for ties.
        let mut groups: Vec<(&String, &Vec<Gap>)> = by_package.iter().collect();
        groups.sort_by(|(an, av), (bn, bv)| {
            let am = av.iter().filter(|g| g.kind == GapKind::Missing).count();
            let bm = bv.iter().filter(|g| g.kind == GapKind::Missing).count();
            bm.cmp(&am)
                .then_with(|| bv.len().cmp(&av.len()))
                .then_with(|| an.cmp(bn))
        });

        for (package, group) in groups {
            let label = if package.is_empty() {
                "<root>".to_string()
            } else {
                package.clone()
            };
            let m = group.iter().filter(|g| g.kind == GapKind::Missing).count();
            let s = group.iter().filter(|g| g.kind == GapKind::SignatureMismatch).count();
            writeln!(report, "### `{label}` (M:{m} S:{s})").ok();
            report.push('\n');
            report.push_str("| Kind | Pure FQN | Mangled key |\n|---|---|---|\n");
            for gap in group {
                let kind_tag = match gap.kind {
                    GapKind::Missing => "M",
                    GapKind::SignatureMismatch => "S",
                };
                writeln!(
                    report,
                    "| {} | `{}` | `{}` |",
                    kind_tag, gap.user_path, gap.mangled_fqn
                )
                .ok();
            }
            report.push('\n');
        }
    }

    // ---- Write to disk ------------------------------------------------
    let out_path = report_path();
    if let Some(parent) = out_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&out_path, &report) {
        eprintln!("warning: failed to write PCT coverage report to {out_path:?}: {e}");
    } else {
        eprintln!("PCT native coverage report: {}", out_path.display());
    }

    eprintln!(
        "PCT native coverage: {covered}/{total_natives} covered ({coverage_pct:.1}%); \
         Missing: {missing_count} (≤{MISSING_CEILING}); SignatureMismatch: \
         {mismatch_count} (≤{MISMATCH_CEILING})",
        covered = total_natives - total_gaps,
    );

    assert!(
        missing_count <= MISSING_CEILING,
        "PCT native coverage regressed: {missing_count} Missing > ceiling {MISSING_CEILING}. \
         Report at {}.",
        out_path.display()
    );
    assert!(
        mismatch_count <= MISMATCH_CEILING,
        "PCT signature-mismatch count regressed: {mismatch_count} > ceiling {MISMATCH_CEILING}. \
         Report at {}.",
        out_path.display()
    );
}

/// Extract the simple function name from a mangled FQN. The mangled
/// format is `simpleName_ParamType_Mult__…__ReturnType_Mult_`, so the
/// simple name is everything before the FIRST single underscore that
/// isn't part of a `__` boundary. In practice it's everything before
/// the first `_` — Pure function names don't contain underscores in
/// their source form (they use `camelCase`).
fn simple_name_of(mangled: &str) -> &str {
    match mangled.find('_') {
        Some(i) => &mangled[..i],
        None => mangled,
    }
}

/// Compose a `::`-joined FQN from a package + simple name. Mirrors
/// `inference_drift_histogram::element_fqn` so report formats stay
/// consistent between audits.
fn render_element_path(
    model: &PureModel,
    package_id: &legend_pure_parser_pure::ids::PackageId,
    simple_name: &str,
) -> String {
    let mut segments = package_segments(model, package_id);
    segments.push(simple_name.to_string());
    segments.join("::")
}

/// `::`-joined package path with no trailing simple name, used to group
/// gaps in the report. Empty string for elements at the unnamed root.
fn render_package_path(
    model: &PureModel,
    package_id: &legend_pure_parser_pure::ids::PackageId,
) -> String {
    package_segments(model, package_id).join("::")
}

fn package_segments(
    model: &PureModel,
    package_id: &legend_pure_parser_pure::ids::PackageId,
) -> Vec<String> {
    let mut segments: Vec<String> = Vec::new();
    let mut id = Some(*package_id);
    while let Some(pid) = id {
        let pkg = model.get_package(pid);
        if pkg.name.is_empty() {
            break;
        }
        segments.push(pkg.name.to_string());
        id = pkg.parent;
    }
    segments.reverse();
    segments
}

fn report_path() -> PathBuf {
    // Walk up from CARGO_MANIFEST_DIR (`crates/runtime`) to the
    // workspace root, then into `target/test-output`.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest
        .parent() // crates/
        .and_then(|p| p.parent()) // legend-pure-rust/
        .map(PathBuf::from)
        .unwrap_or(manifest);
    workspace_root
        .join("target")
        .join("test-output")
        .join("pct_native_gaps.md")
}

