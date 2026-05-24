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

//! `legend build` — One-shot, incremental, dependency-ordered build
//! over a `legend-pure-classpath.toml`.
//!
//! For each filesystem (or embedded) repo in the classpath, in
//! topological dependency order: compute a content-hash fingerprint
//! of its inputs, compare against the cached stamp at
//! `<cache>/<repo>/cache.toml`, and either re-use the cached
//! `source.purem` or re-run parse + compile + serialize. Cached
//! `.purem` repos in the classpath pass through untouched (they're
//! already compiled). After all repos are resolved to `.purem` form,
//! the test surveyor runs over the unified model; the per-repo
//! `last_test_result` field on the stamp lets the next build skip
//! tests for repos that already passed.
//!
//! # Usage
//!
//! ```bash
//! legend build --classpath legend-pure-classpath.toml
//! legend build --classpath legend-pure-classpath.toml --skip-tests
//! legend build --classpath legend-pure-classpath.toml --clean
//! legend build --classpath legend-pure-classpath.toml --repo my_app
//! ```

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use legend_pure_core_platform::platform::PLATFORM_AUTO_IMPORTS;
use legend_pure_core_platform::repo::{self, Repo};
use legend_pure_core_platform::topo::topo_sort_repos;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::purem::header::SCHEMA_HASH;
use legend_pure_parser_pure::purem::{slice_by_repo, write_repo};
use owo_colors::OwoColorize;
use smol_str::SmolStr;

use super::build_cache::{
    CLI_VERSION, Fingerprint, Stamp, TestResult, clean_cache, compute_fingerprint,
    default_cache_dir, ensure_cache_dir, fingerprint_from_hex, fingerprint_to_hex, now_unix,
    read_stamp, write_stamp,
};
use super::test::run_build_tests;
use crate::diagnostics::CliError;

/// Output format for `legend build`.
#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum BuildFormat {
    /// Human-readable coloured summary on stderr.
    #[default]
    Pretty,
    /// One NDJSON object per repo on stdout + a final summary line.
    /// Suitable for IDE / CI integrations.
    Json,
}

/// `legend build` arguments.
#[derive(clap::Args, Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct BuildArgs {
    /// Remove the cache directory (after verifying its sentinel
    /// marker) before building. Forces every filesystem repo to
    /// recompile from scratch.
    #[arg(long)]
    pub clean: bool,

    /// Skip the test phase entirely. Parse + compile + emit purem
    /// still run. The stamp's `last_test_result` is set to `skipped`
    /// so the next build (without this flag) re-runs tests for every
    /// repo regardless of cache state.
    #[arg(long = "skip-tests")]
    pub skip_tests: bool,

    /// Override the cache directory. Defaults to
    /// `<classpath-dir>/target/legend/` (or
    /// `$XDG_CACHE_HOME/legend/` when the classpath has no on-disk
    /// source).
    #[arg(long, value_name = "DIR")]
    pub cache_dir: Option<PathBuf>,

    /// Restrict the build's test phase to a subset of repos. Build
    /// still compiles every repo (with auto-rebuild of stale
    /// dependencies), but only repos named here have their tests
    /// run. Repeat for multiple: `--repo a --repo b`.
    #[arg(long = "repo", value_name = "NAME")]
    pub repos: Vec<String>,

    /// Verify-only mode: parse + compile + (optionally) test, but do
    /// not materialize `.purem` artifacts to disk.
    #[arg(long = "no-write-purem")]
    pub no_write_purem: bool,

    /// Output format.
    #[arg(long, value_enum, default_value_t = BuildFormat::Pretty)]
    pub format: BuildFormat,
}

/// One repo's outcome in the build summary.
#[derive(Debug, Clone)]
struct RepoStatus {
    name: SmolStr,
    kind: StatusKind,
    /// Wall-clock duration of this repo's work (compile / cache load
    /// / passthrough). Zero for entries that didn't do meaningful
    /// work.
    elapsed_ms: u128,
}

#[derive(Debug, Clone)]
enum StatusKind {
    /// `.purem`-kind repo from the classpath — passed through as-is.
    Passthrough,
    /// Cache hit: loaded the cached purem instead of recompiling.
    /// `needs_tests` reflects whether the stamp's
    /// `last_test_result` warrants re-running tests.
    Cached { needs_tests: bool },
    /// Cache miss: recompiled from sources and wrote a fresh purem
    /// (unless `--no-write-purem`).
    Built,
    /// Compile failed for this repo. `errors` is the count of
    /// compilation errors reported by the loader.
    Failed { errors: usize },
    /// Skipped because an upstream dependency failed. `blocker` is
    /// the name of the first failing dependency reached in topo
    /// order.
    Blocked { blocker: SmolStr },
}

/// Run `legend build`.
///
/// # Errors
///
/// Returns [`CliError`] when classpath resolution, cache I/O, or any
/// compile / test phase fails. Compile errors and test failures show
/// up as non-zero exit code (via [`CliError::Custom`] at the end of
/// the function) rather than aborting mid-loop, so the user sees the
/// full per-repo summary.
#[allow(clippy::needless_pass_by_value)] // clap convention
#[allow(clippy::too_many_lines)]
pub fn run(args: BuildArgs, classpath: Option<&std::path::Path>) -> Result<(), CliError> {
    let cwd = std::env::current_dir().map_err(|e| CliError::Custom(format!("cwd: {e}")))?;
    let resolved = crate::classpath::resolve_classpath(classpath, &cwd)
        .map_err(|e| CliError::Custom(format!("classpath: {e}")))?;

    let cache_dir = args
        .cache_dir
        .clone()
        .unwrap_or_else(|| default_cache_dir(resolved.source.as_deref()));

    if args.clean {
        clean_cache(&cache_dir).map_err(|e| {
            CliError::Custom(format!(
                "--clean failed for {}: {e}",
                cache_dir.display()
            ))
        })?;
    }
    ensure_cache_dir(&cache_dir).map_err(|e| {
        CliError::Custom(format!(
            "creating cache dir {}: {e}",
            cache_dir.display()
        ))
    })?;

    // Resolve the auto-import set the same way `legend test` /
    // `legend snapshot` do: platform defaults plus whatever the
    // classpath TOML's `auto_imports` extends with.
    let mut auto_imports: Vec<SmolStr> = PLATFORM_AUTO_IMPORTS
        .iter()
        .map(|&s| SmolStr::new(s))
        .collect();
    auto_imports.extend(resolved.extra_auto_imports.iter().cloned());

    let sorted: Vec<&Repo> = topo_sort_repos(&resolved.repos)
        .map_err(|e| CliError::Custom(format!("repo topology: {e}")))?;

    // O(1) lookup of each sorted repo's index in the original
    // classpath slice so we can pair it with a stable order for the
    // summary printer.
    let topo_indices: Vec<usize> = sorted
        .iter()
        .filter_map(|sr| resolved.repos.iter().position(|r| std::ptr::eq(r, *sr)))
        .collect();

    let mut working_repos: Vec<Repo> = Vec::with_capacity(resolved.repos.len());
    let mut dep_fingerprints: HashMap<SmolStr, Fingerprint> = HashMap::new();
    let mut statuses: Vec<RepoStatus> = Vec::with_capacity(resolved.repos.len());
    let mut current_model: Option<PureModel> = None;
    let mut prev_chunk_count: usize = 0;
    let mut blocker: Option<SmolStr> = None;
    // After the loop runs, this maps repo name → fingerprint that
    // produced the cached purem in this build. Used by the test
    // phase to write fresh stamps reflecting test outcomes.
    let mut stamp_data: HashMap<SmolStr, StampData> = HashMap::new();

    for &i in &topo_indices {
        let repo = &resolved.repos[i];
        let Some(meta) = repo.meta() else {
            return Err(CliError::Custom(format!(
                "repo at classpath index {i} has no descriptor metadata; `legend build` requires it. \
                 (Repos built via `Repo::from_filesystem` carry no metadata; use a descriptor JSON.)"
            )));
        };
        let name = SmolStr::new(meta.name);

        // If an upstream repo already failed, mark this one Blocked
        // without touching the cache.
        if let Some(b) = &blocker {
            statuses.push(RepoStatus {
                name: name.clone(),
                kind: StatusKind::Blocked { blocker: b.clone() },
                elapsed_ms: 0,
            });
            continue;
        }

        let start = std::time::Instant::now();
        match repo {
            Repo::Purem { blob, .. } => {
                let fp_bytes = blake3::hash(blob);
                let fp: Fingerprint = *fp_bytes.as_bytes();
                working_repos.push(repo.clone());
                dep_fingerprints.insert(name.clone(), fp);
                statuses.push(RepoStatus {
                    name: name.clone(),
                    kind: StatusKind::Passthrough,
                    elapsed_ms: start.elapsed().as_millis(),
                });
            }
            Repo::Embedded { .. } | Repo::Filesystem { .. } => {
                let dep_subset: BTreeMap<&str, Fingerprint> = meta
                    .dependencies
                    .iter()
                    .filter_map(|d| dep_fingerprints.get(&SmolStr::new(*d)).map(|fp| (*d, *fp)))
                    .collect();
                let fp = compute_fingerprint(
                    repo,
                    &dep_subset,
                    CLI_VERSION,
                    SCHEMA_HASH,
                    &auto_imports,
                );

                let repo_cache_dir = cache_dir.join(meta.name);
                let stamp_path = repo_cache_dir.join("cache.toml");
                let purem_path = repo_cache_dir.join("source.purem");

                let prior_stamp = read_stamp(&stamp_path);
                let cache_hit = !args.clean
                    && prior_stamp
                        .as_ref()
                        .and_then(|s| fingerprint_from_hex(&s.fingerprint))
                        .is_some_and(|f| f == fp)
                    && purem_path.is_file();

                if cache_hit {
                    let cached_repo =
                        Repo::from_purem_file(&purem_path, format!("/{}", meta.name), *meta)
                            .map_err(|e| {
                                CliError::Custom(format!(
                                    "loading cached purem for {}: {e}",
                                    meta.name
                                ))
                            })?;
                    working_repos.push(cached_repo);
                    dep_fingerprints.insert(name.clone(), fp);
                    let needs_tests = !args.skip_tests
                        && prior_stamp
                            .as_ref()
                            .is_none_or(|s| s.last_test_result != TestResult::Pass);
                    stamp_data.insert(
                        name.clone(),
                        StampData {
                            fingerprint: fp,
                            dep_subset_owned: dep_subset
                                .iter()
                                .map(|(d, f)| ((*d).to_string(), *f))
                                .collect(),
                            // Default carries the prior outcome forward;
                            // overwritten below by the test phase if it
                            // runs.
                            test_result: prior_stamp
                                .as_ref()
                                .map_or(TestResult::Skipped, |s| s.last_test_result),
                        },
                    );
                    statuses.push(RepoStatus {
                        name: name.clone(),
                        kind: StatusKind::Cached { needs_tests },
                        elapsed_ms: start.elapsed().as_millis(),
                    });
                } else {
                    // Cache miss: compile `working_repos + this`
                    // from scratch. The previously compiled repos
                    // in `working_repos` are all `Repo::Purem` after
                    // the first miss, so their cost is merge-only.
                    let mut scratch = working_repos.clone();
                    scratch.push(repo.clone());
                    let model_result = repo::load(&scratch, &auto_imports);
                    match model_result {
                        Ok(model) => {
                            // Snapshot the new chunk range that
                            // belongs to this repo. With all prior
                            // entries already Purem (or the very
                            // first compile starting from
                            // bootstrap), the new chunks are
                            // appended at the tail.
                            let chunk_count = model.chunks.len();
                            let new_range = u16::try_from(prev_chunk_count).map_err(|_| {
                                CliError::Custom("chunk count overflow".into())
                            })?
                                ..u16::try_from(chunk_count).map_err(|_| {
                                    CliError::Custom("chunk count overflow".into())
                                })?;
                            let slice = slice_by_repo(&model, new_range);
                            let bytes = write_repo(&slice).map_err(|e| {
                                CliError::Custom(format!("write_repo for {}: {e}", meta.name))
                            })?;

                            if !args.no_write_purem {
                                std::fs::create_dir_all(&repo_cache_dir).map_err(|e| {
                                    CliError::Custom(format!(
                                        "mkdir {}: {e}",
                                        repo_cache_dir.display()
                                    ))
                                })?;
                                std::fs::write(&purem_path, &bytes).map_err(|e| {
                                    CliError::Custom(format!(
                                        "writing {}: {e}",
                                        purem_path.display()
                                    ))
                                })?;
                            }

                            // Substitute the freshly built repo's
                            // purem form into working_repos so
                            // subsequent compiles see it as Purem.
                            let cached_repo = Repo::from_purem_bytes(
                                format!("/{}", meta.name),
                                *meta,
                                Arc::from(bytes.into_boxed_slice()),
                            );
                            working_repos.push(cached_repo);
                            dep_fingerprints.insert(name.clone(), fp);
                            current_model = Some(model);
                            prev_chunk_count = chunk_count;
                            stamp_data.insert(
                                name.clone(),
                                StampData {
                                    fingerprint: fp,
                                    dep_subset_owned: dep_subset
                                        .iter()
                                        .map(|(d, f)| ((*d).to_string(), *f))
                                        .collect(),
                                    test_result: TestResult::Skipped,
                                },
                            );
                            statuses.push(RepoStatus {
                                name: name.clone(),
                                kind: StatusKind::Built,
                                elapsed_ms: start.elapsed().as_millis(),
                            });
                        }
                        Err(partial) => {
                            let count = partial.errors.len();
                            for err in &partial.errors {
                                eprintln!(
                                    "  {} {}: {}",
                                    "error:".red().bold(),
                                    err.source_info.source,
                                    err.message,
                                );
                            }
                            statuses.push(RepoStatus {
                                name: name.clone(),
                                kind: StatusKind::Failed { errors: count },
                                elapsed_ms: start.elapsed().as_millis(),
                            });
                            blocker = Some(name.clone());
                        }
                    }
                }

                // For cache hits and after a successful compile,
                // refresh the chunk-count snapshot so the next miss
                // can compute the right range. The cheapest way is
                // to do a load that uses every repo in
                // `working_repos`. On the very first cache hit
                // before any miss, `current_model` may still be
                // None; we lazily build it here.
                if matches!(statuses.last(), Some(s) if matches!(s.kind, StatusKind::Cached { .. }))
                {
                    match repo::load(&working_repos, &auto_imports) {
                        Ok(m) => {
                            prev_chunk_count = m.chunks.len();
                            current_model = Some(m);
                        }
                        Err(partial) => {
                            return Err(CliError::Custom(format!(
                                "loading cached repos failed unexpectedly with {} error(s)",
                                partial.errors.len()
                            )));
                        }
                    }
                }
            }
            // `Repo` is `#[non_exhaustive]`; future variants land
            // here so the match stays sound across upstream changes.
            _ => {
                return Err(CliError::Custom(format!(
                    "unsupported repo variant for {}; `legend build` accepts \
                     Embedded / Filesystem / Purem repos only",
                    meta.name,
                )));
            }
        }

        // Passthrough Purem entries also need the chunk-count
        // snapshot maintained so a later cache-miss compile gets
        // the right starting chunk.
        if matches!(statuses.last(), Some(s) if matches!(s.kind, StatusKind::Passthrough)) {
            match repo::load(&working_repos, &auto_imports) {
                Ok(m) => {
                    prev_chunk_count = m.chunks.len();
                    current_model = Some(m);
                }
                Err(partial) => {
                    return Err(CliError::Custom(format!(
                        "loading passthrough repos failed unexpectedly with {} error(s)",
                        partial.errors.len()
                    )));
                }
            }
        }
    }

    let any_failure = statuses
        .iter()
        .any(|s| matches!(s.kind, StatusKind::Failed { .. } | StatusKind::Blocked { .. }));

    // Test phase. We always test all built+needs-run repos
    // together: a single surveyor pass over the unified model.
    // Test results map back to per-repo stamps.
    let mut test_summary: Option<super::test::BuildTestSummary> = None;
    if !args.skip_tests && !any_failure {
        let needs_any_tests = statuses.iter().any(|s| match &s.kind {
            StatusKind::Built => true,
            StatusKind::Cached { needs_tests } => *needs_tests,
            _ => false,
        });
        if needs_any_tests {
            let model = match current_model {
                Some(m) => m,
                None => repo::load(&working_repos, &auto_imports).map_err(|partial| {
                    CliError::Custom(format!(
                        "final model load failed with {} error(s)",
                        partial.errors.len()
                    ))
                })?,
            };

            let summary = run_build_tests(
                &model,
                &resolved.extension_configs,
                "Root",
                "",
                match args.format {
                    BuildFormat::Pretty => super::test::TestFormat::Pretty,
                    BuildFormat::Json => super::test::TestFormat::Json,
                },
                false,
            )?;

            // Stamp the test outcome onto every repo that ran in
            // this build (Built or Cached-NeedsRun). For repos
            // explicitly listed via `--repo`, only those get their
            // test_result updated; the others keep their prior
            // outcome.
            let filter_set: HashSet<&str> =
                args.repos.iter().map(String::as_str).collect();
            let outcome = if summary.has_failures() {
                TestResult::Fail
            } else {
                TestResult::Pass
            };
            for s in &statuses {
                let in_filter = filter_set.is_empty() || filter_set.contains(s.name.as_str());
                let touched_by_tests = matches!(
                    s.kind,
                    StatusKind::Built | StatusKind::Cached { needs_tests: true }
                );
                if !touched_by_tests || !in_filter {
                    continue;
                }
                if let Some(data) = stamp_data.get_mut(&s.name) {
                    data.test_result = outcome;
                }
            }
            test_summary = Some(summary);
        }
    }

    // Persist stamps for everything we touched. For --skip-tests,
    // last_test_result is "skipped" so the next build re-runs them.
    if !args.no_write_purem {
        for s in &statuses {
            let Some(data) = stamp_data.get(&s.name) else {
                continue;
            };
            let stamp_path = cache_dir.join(s.name.as_str()).join("cache.toml");
            let dep_fps_str: BTreeMap<String, String> = data
                .dep_subset_owned
                .iter()
                .map(|(d, fp)| (d.clone(), fingerprint_to_hex(fp)))
                .collect();
            let stamp = Stamp {
                schema: super::build_cache::STAMP_SCHEMA,
                fingerprint: fingerprint_to_hex(&data.fingerprint),
                cli_version: CLI_VERSION.to_string(),
                purem_schema: format!("{SCHEMA_HASH:08x}"),
                generated_at: now_unix(),
                dep_fingerprints: dep_fps_str,
                last_test_result: data.test_result,
            };
            write_stamp(&stamp_path, &stamp).map_err(|e| {
                CliError::Custom(format!("writing stamp {}: {e}", stamp_path.display()))
            })?;
        }
    }

    print_summary(&statuses, test_summary.as_ref(), args.format);

    let test_failed = test_summary
        .as_ref()
        .is_some_and(|s| s.fail + s.error > 0);
    if any_failure || test_failed {
        return Err(CliError::Custom(format!(
            "build failed: {failed} repo(s) failed, {blocked} repo(s) blocked, {tf} test failure(s), {te} test error(s)",
            failed = statuses
                .iter()
                .filter(|s| matches!(s.kind, StatusKind::Failed { .. }))
                .count(),
            blocked = statuses
                .iter()
                .filter(|s| matches!(s.kind, StatusKind::Blocked { .. }))
                .count(),
            tf = test_summary.as_ref().map_or(0, |s| s.fail),
            te = test_summary.as_ref().map_or(0, |s| s.error),
        )));
    }

    Ok(())
}

/// Fingerprint + per-dep snapshot used when writing the stamp at the
/// end of the build (after tests run). Kept owned so we don't have
/// to chase borrows through the test phase.
struct StampData {
    fingerprint: Fingerprint,
    dep_subset_owned: BTreeMap<String, Fingerprint>,
    test_result: TestResult,
}

fn print_summary(
    statuses: &[RepoStatus],
    test_summary: Option<&super::test::BuildTestSummary>,
    format: BuildFormat,
) {
    match format {
        BuildFormat::Pretty => print_pretty(statuses, test_summary),
        BuildFormat::Json => print_json(statuses, test_summary),
    }
}

fn print_pretty(statuses: &[RepoStatus], _test_summary: Option<&super::test::BuildTestSummary>) {
    // The test summary is rendered by `run_build_tests` itself (it
    // shares the same surveyor renderer as `legend test`), so this
    // function only prints the per-repo build status block.
    eprintln!();
    eprintln!("{}", "Build summary".bold());
    let name_width = statuses
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(0)
        .max(4);
    for s in statuses {
        let (mark, label) = match &s.kind {
            StatusKind::Passthrough => ("◌".dimmed().to_string(), "passthrough".dimmed().to_string()),
            StatusKind::Cached { needs_tests: false } => {
                ("✓".green().to_string(), "cached".green().to_string())
            }
            StatusKind::Cached { needs_tests: true } => {
                ("✓".green().to_string(), "cached (needs tests)".green().to_string())
            }
            StatusKind::Built => ("●".cyan().to_string(), "built".cyan().to_string()),
            StatusKind::Failed { errors } => (
                "✗".red().to_string(),
                format!("failed ({errors} error{})", if *errors == 1 { "" } else { "s" })
                    .red()
                    .to_string(),
            ),
            StatusKind::Blocked { blocker } => (
                "○".yellow().to_string(),
                format!("blocked by {blocker}").yellow().to_string(),
            ),
        };
        eprintln!(
            "  {mark} {name:<width$}  {label}  {timing}",
            name = s.name,
            width = name_width,
            timing = format!("({}ms)", s.elapsed_ms).dimmed(),
        );
    }
}

fn print_json(statuses: &[RepoStatus], _test_summary: Option<&super::test::BuildTestSummary>) {
    // Test results (per-test NDJSON + final `{"type":"summary"}` for
    // tests) are emitted by `run_build_tests` directly. This printer
    // emits a `{"type":"repo"}` line per repo and a closing
    // `{"type":"buildSummary"}` aggregate. The two `type` values are
    // distinct so a JSON consumer can demux on `type` without
    // ambiguity.
    for s in statuses {
        let (kind, extra): (&str, serde_json::Value) = match &s.kind {
            StatusKind::Passthrough => ("passthrough", serde_json::json!({})),
            StatusKind::Cached { needs_tests } => {
                ("cached", serde_json::json!({"needsTests": needs_tests}))
            }
            StatusKind::Built => ("built", serde_json::json!({})),
            StatusKind::Failed { errors } => ("failed", serde_json::json!({"errors": errors})),
            StatusKind::Blocked { blocker } => {
                ("blocked", serde_json::json!({"blocker": blocker.as_str()}))
            }
        };
        let payload = serde_json::json!({
            "type":      "repo",
            "name":      s.name.as_str(),
            "status":    kind,
            "elapsedMs": s.elapsed_ms,
            "extra":     extra,
        });
        println!("{payload}");
    }
    let summary = serde_json::json!({
        "type":    "buildSummary",
        "repos":   statuses.len(),
        "passthrough": statuses.iter().filter(|s| matches!(s.kind, StatusKind::Passthrough)).count(),
        "cached":  statuses.iter().filter(|s| matches!(s.kind, StatusKind::Cached { .. })).count(),
        "built":   statuses.iter().filter(|s| matches!(s.kind, StatusKind::Built)).count(),
        "failed":  statuses.iter().filter(|s| matches!(s.kind, StatusKind::Failed { .. })).count(),
        "blocked": statuses.iter().filter(|s| matches!(s.kind, StatusKind::Blocked { .. })).count(),
    });
    println!("{summary}");
}
