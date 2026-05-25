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
//
//! Per-expression inference-drift histogram across the embedded
//! platform model.
//!
//! Complements `inference_precision_sweep.rs` (whole-function-body
//! Any check) with **per-expression resolution**: for every
//! `ValueSpec` in every concrete function body, count surviving
//! `Generic(name)` and `Variable(name)` markers in the inferred
//! `type_info.type_expr`. These are unsubstituted type / multiplicity
//! parameters — places where dispatch couldn't resolve a binding,
//! either because no concrete arg supplied one (genuine drift) or
//! because the parameter is in scope at the surrounding fn (legitimate
//! pass-through).
//!
//! Output: a sorted Markdown report at
//! `target/test-output/inference_drift.md`.
//!
//! The test asserts a hard ceiling so future regressions in the
//! dispatch / generic-substitution / lambda-inference pipeline trip
//! a CI failure. The ceiling is set to whatever the platform measured
//! at first run (Item 3 audit, May 2026); follow-up work that lowers
//! drift bumps the ceiling down.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use legend_pure_core_platform::platform;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::{
    ExprKind, FunctionCallData, Multiplicity, TypeExpr, ValueSpec,
};

/// Hard ceiling for total surviving Generic / Variable markers across
/// all platform expressions.
///
/// Baseline measured during the Item 3 audit (May 2026): **5931**
/// (4380 type + 1551 multiplicity, across 563 functions with non-zero
/// drift).
///
/// **Z-propagation fix (May 2026-05-18)**: substituting `ty_auth`
/// bindings into the param type before pass-2's FunctionType check
/// (`resolve::infer_generic_bindings`) plus using
/// `infer_typeexpr_from_valuespec` (instead of bare-element
/// `infer_type_from_valuespec`) in
/// `inference::lambda::bind_from_lambda_body` propagates inner
/// generics through PCT-runner-style call shapes. New post-fix
/// measurement: **4531** (2977 type + 1554 multiplicity, across 567
/// functions). Ceiling lowered from 6500 to 4800 — leaves ~270
/// headroom for natural micro-drift and trips on real regressions.
/// Lower this when a chain-inference fix lands. **Do not raise** —
/// a regression is the alarm this ceiling exists to surface.
const DRIFT_CEILING: usize = 4800;

/// Per-function tally.
#[derive(Default, Debug, Clone)]
struct FnDrift {
    fqn: String,
    type_drift: usize,
    mult_drift: usize,
    /// Drift count by expression kind (FunctionCall / PropertyCall / etc.).
    by_kind: BTreeMap<&'static str, usize>,
}

/// Test entry — runs the sweep, writes the Markdown report, and
/// asserts the ceiling.
#[test]
fn inference_drift_histogram_under_ceiling() {
    use std::fmt::Write as _;

    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    let mut per_fn: Vec<FnDrift> = Vec::new();
    let mut total_type = 0usize;
    let mut total_mult = 0usize;
    let mut total_kind: BTreeMap<&'static str, usize> = BTreeMap::new();

    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Function(func) = element else {
                continue;
            };
            if func.is_native {
                continue;
            }
            let node = chunk.nodes.get(local_idx);
            let fqn = element_fqn(&model, &node.parent_package, node.name.as_str());

            let mut tally = FnDrift {
                fqn,
                ..Default::default()
            };
            for spec in func.body.iter() {
                walk_value_spec(spec, &mut tally);
            }
            if tally.type_drift > 0 || tally.mult_drift > 0 {
                total_type += tally.type_drift;
                total_mult += tally.mult_drift;
                for (k, v) in &tally.by_kind {
                    *total_kind.entry(k).or_insert(0) += v;
                }
                per_fn.push(tally);
            }
        }
    }

    let total = total_type + total_mult;

    // ---- Render Markdown report ----------------------------------------
    let mut report = String::new();
    report.push_str("# Inference-drift histogram\n\n");
    writeln!(
        report,
        "**Total surviving Generic + Variable markers:** {total}\n"
    )
    .ok();
    writeln!(report, "- Generic (type): {total_type}").ok();
    writeln!(report, "- Variable (multiplicity): {total_mult}").ok();
    writeln!(report, "- Functions with drift: {}", per_fn.len()).ok();
    writeln!(report, "- Ceiling: {DRIFT_CEILING}\n").ok();

    report.push_str("## Drift by expression kind\n\n");
    report.push_str("| Kind | Count |\n|---|---|\n");
    let mut by_kind_sorted: Vec<(&&str, &usize)> = total_kind.iter().collect();
    by_kind_sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (kind, count) in by_kind_sorted {
        writeln!(report, "| {kind} | {count} |").ok();
    }
    report.push('\n');

    report.push_str("## Drift bucket histogram\n\n");
    let buckets = drift_buckets(&per_fn);
    report.push_str("| Bucket | Functions |\n|---|---|\n");
    for (label, n) in &buckets {
        writeln!(report, "| {label} | {n} |").ok();
    }
    report.push('\n');

    report.push_str("## Top 20 worst offenders\n\n");
    report.push_str("| FQN | Type drift | Mult drift | Total |\n|---|---|---|---|\n");
    let mut sorted = per_fn.clone();
    sorted.sort_by(|a, b| {
        let total_a = a.type_drift + a.mult_drift;
        let total_b = b.type_drift + b.mult_drift;
        total_b.cmp(&total_a)
    });
    for fd in sorted.iter().take(20) {
        writeln!(
            report,
            "| `{}` | {} | {} | {} |",
            fd.fqn,
            fd.type_drift,
            fd.mult_drift,
            fd.type_drift + fd.mult_drift
        )
        .ok();
    }
    report.push('\n');

    // ---- Write to disk ----
    let out_path = report_path();
    if let Some(parent) = out_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&out_path, &report) {
        eprintln!("warning: failed to write drift histogram to {out_path:?}: {e}");
    } else {
        eprintln!("inference-drift histogram: {}", out_path.display());
    }

    // ---- Always print a one-line summary so the test log shows it ----
    eprintln!(
        "inference-drift total: {total} (type {total_type} + mult {total_mult}); \
         ceiling {DRIFT_CEILING}; functions-with-drift {}",
        per_fn.len(),
    );

    assert!(
        total <= DRIFT_CEILING,
        "Inference drift regressed: total {total} > ceiling {DRIFT_CEILING}. \
         Report at {}.",
        out_path.display()
    );
}

/// Bucket count by drift size.
fn drift_buckets(fns: &[FnDrift]) -> Vec<(&'static str, usize)> {
    let mut clean = 0usize;
    let mut minor = 0usize;
    let mut moderate = 0usize;
    let mut significant = 0usize;
    for fd in fns {
        let total = fd.type_drift + fd.mult_drift;
        match total {
            0 => clean += 1,
            1..=2 => minor += 1,
            3..=10 => moderate += 1,
            _ => significant += 1,
        }
    }
    vec![
        ("0 (clean)", clean),
        ("1-2 (minor)", minor),
        ("3-10 (moderate)", moderate),
        ("11+ (significant)", significant),
    ]
}

/// Recursively walk a `ValueSpec`, tallying drift on its inferred
/// type/multiplicity and recursing into sub-expressions.
fn walk_value_spec(spec: &ValueSpec, tally: &mut FnDrift) {
    let kind_name: &'static str = match spec.kind.as_ref() {
        ExprKind::IntegerLiteral(_) => "IntegerLiteral",
        ExprKind::FloatLiteral(_) => "FloatLiteral",
        ExprKind::DecimalLiteral(_) => "DecimalLiteral",
        ExprKind::StringLiteral(_) => "StringLiteral",
        ExprKind::BooleanLiteral(_) => "BooleanLiteral",
        ExprKind::DateLiteral(_) => "DateLiteral",
        ExprKind::Variable { .. } => "Variable",
        ExprKind::FunctionCall(_) => "FunctionCall",
        ExprKind::PropertyCall(_) => "PropertyCall",
        ExprKind::QualifiedPropertyCall(_) => "QualifiedPropertyCall",
        ExprKind::EnumValue { .. } => "EnumValue",
        ExprKind::Lambda { .. } => "Lambda",
        ExprKind::Collection { .. } => "Collection",
        ExprKind::TypeReference { .. } => "TypeReference",
        ExprKind::PackageableElementRef { .. } => "PackageableElementRef",
        _ => "Other",
    };

    if let Some(rt) = spec.type_info.as_ref() {
        let ty_drift = count_generic(&rt.type_expr);
        let mult_drift = count_variable(&rt.multiplicity);
        if ty_drift > 0 {
            tally.type_drift += ty_drift;
            *tally.by_kind.entry(kind_name).or_insert(0) += ty_drift;
        }
        if mult_drift > 0 {
            tally.mult_drift += mult_drift;
            *tally.by_kind.entry(kind_name).or_insert(0) += mult_drift;
        }
    }

    // Recurse into sub-expressions.
    match spec.kind.as_ref() {
        ExprKind::FunctionCall(FunctionCallData { arguments, .. })
        | ExprKind::PropertyCall(FunctionCallData { arguments, .. })
        | ExprKind::QualifiedPropertyCall(FunctionCallData { arguments, .. }) => {
            for a in arguments {
                walk_value_spec(a, tally);
            }
        }
        ExprKind::Lambda { body, .. } => {
            for s in body {
                walk_value_spec(s, tally);
            }
        }
        ExprKind::Collection { elements } => {
            for s in elements {
                walk_value_spec(s, tally);
            }
        }
        _ => {}
    }
}

/// Count surviving `Generic(_)` markers in a `TypeExpr`. Recurses
/// into nested `Named.type_arguments`, `FunctionType` shapes, and
/// algebraic unions.
fn count_generic(t: &TypeExpr) -> usize {
    match t {
        TypeExpr::Generic(_) => 1,
        TypeExpr::Named { type_arguments, .. } => type_arguments.iter().map(count_generic).sum(),
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            let p: usize = parameters.iter().map(|(t, _)| count_generic(t)).sum();
            p + count_generic(return_type)
        }
        TypeExpr::Relation(cols) => cols.iter().map(|c| count_generic(&c.type_expr)).sum(),
        TypeExpr::GenericTypeOperation {
            left: a, right: b, ..
        } => count_generic(a) + count_generic(b),
        TypeExpr::Unresolved => 0,
    }
}

/// Count surviving `Variable(_)` markers in a `Multiplicity`. Range
/// multiplicities have no Variable; only the explicit `Variable`
/// constructor counts.
fn count_variable(m: &Multiplicity) -> usize {
    match m {
        Multiplicity::Variable(_) => 1,
        _ => 0,
    }
}

/// Build a slash-separated FQN from package + simple name.
fn element_fqn(
    model: &PureModel,
    package_id: &legend_pure_parser_pure::ids::PackageId,
    simple_name: &str,
) -> String {
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
    segments.push(simple_name.to_string());
    segments.join("::")
}

fn report_path() -> PathBuf {
    // Walk up from CARGO_MANIFEST_DIR (crates/core-platform-pure) to
    // the workspace root, then into target/test-output.
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest
        .parent() // crates/
        .and_then(|p| p.parent()) // legend-pure-rust/
        .map(PathBuf::from)
        .unwrap_or(manifest);
    workspace_root
        .join("target")
        .join("test-output")
        .join("inference_drift.md")
}
