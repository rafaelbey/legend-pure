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

//! Coverage report generation — LCOV output and `genhtml` invocation.
//!
//! This module converts a [`CoverageMap`] into standard LCOV tracefile
//! format using the [`lcov`] crate, optionally invokes `genhtml` for
//! HTML report generation, and prints a human-readable summary table.

use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use lcov::Record;

use super::coverage::CoverageMap;

/// Write coverage data to an LCOV tracefile.
///
/// Uses the [`lcov`] crate's typed [`Record`] enum for correct formatting.
/// The output can be consumed by `genhtml`, Codecov, Coveralls, etc.
///
/// `source_roots` maps virtual source paths (e.g. `/platform/pure/...`)
/// to real filesystem locations. Each root is tried in order; the first
/// root where the file exists on disk wins. Pass `&[]` to leave paths as-is.
///
/// # Errors
///
/// Returns an I/O error if the file cannot be created or written.
pub fn write_lcov(map: &CoverageMap, path: &Path, source_roots: &[PathBuf]) -> io::Result<()> {
    let mut file = File::create(path)?;
    let records = build_lcov_records(map, source_roots);
    for record in &records {
        writeln!(file, "{record}")?;
    }
    Ok(())
}

/// Merge one or more existing LCOV tracefiles into a single output file.
///
/// Reads each input file, concatenates all records, and writes to `output`.
/// `genhtml` and other tools naturally merge duplicate entries (summing
/// hit counts), so simple concatenation is correct.
///
/// # Errors
///
/// Returns an I/O error if any file cannot be read or written.
pub fn merge_lcov_files(inputs: &[PathBuf], output: &Path) -> io::Result<()> {
    let mut out = File::create(output)?;
    for input in inputs {
        let content = std::fs::read_to_string(input)?;
        out.write_all(content.as_bytes())?;
    }
    Ok(())
}

/// Build the list of LCOV records from a coverage map.
///
/// `source_roots` are tried in order to resolve virtual paths to real
/// filesystem paths. Pass `&[]` to leave paths unchanged.
///
/// Exposed for testing — callers typically use [`write_lcov`] directly.
#[must_use]
pub fn build_lcov_records(map: &CoverageMap, source_roots: &[PathBuf]) -> Vec<Record> {
    let mut records = Vec::new();

    for (source, file_cov) in map.files() {
        let resolved_path = resolve_source_path(source.as_str(), source_roots);

        records.push(Record::TestName {
            name: "Pure_Coverage".into(),
        });
        records.push(Record::SourceFile {
            path: resolved_path.into(),
        });

        let mut fn_found: u32 = 0;
        let mut fn_hit: u32 = 0;
        for (fqn, entry) in map.functions.functions_in_file(source.as_str()) {
            records.push(Record::FunctionName {
                name: fqn.to_string(),
                start_line: entry.source.start_line,
            });
            records.push(Record::FunctionData {
                name: fqn.to_string(),
                count: entry.hit_count,
            });
            fn_found += 1;
            if entry.hit_count > 0 {
                fn_hit += 1;
            }
        }
        records.push(Record::FunctionsFound { found: fn_found });
        records.push(Record::FunctionsHit { hit: fn_hit });

        let mut br_found: u32 = 0;
        let mut br_hit: u32 = 0;
        for (block_idx, point) in map.branches.points_in_file(source.as_str()).enumerate() {
            for (arm_idx, arm) in point.arms.iter().enumerate() {
                let taken = if arm.hit_count > 0 {
                    Some(arm.hit_count)
                } else {
                    None
                };
                records.push(Record::BranchData {
                    line: point.call_source.start_line,
                    block: u32::try_from(block_idx).unwrap_or(u32::MAX),
                    branch: u32::try_from(arm_idx).unwrap_or(u32::MAX),
                    taken,
                });
                br_found += 1;
                if arm.hit_count > 0 {
                    br_hit += 1;
                }
            }
        }
        records.push(Record::BranchesFound { found: br_found });
        records.push(Record::BranchesHit { hit: br_hit });

        let mut lf: u32 = 0;
        let mut lh: u32 = 0;
        for &line in &file_cov.coverable_lines {
            let count = file_cov.line_hits.get(&line).copied().unwrap_or(0);
            records.push(Record::LineData {
                line,
                count,
                checksum: None,
            });
            lf += 1;
            if count > 0 {
                lh += 1;
            }
        }
        records.push(Record::LinesFound { found: lf });
        records.push(Record::LinesHit { hit: lh });
        records.push(Record::EndOfRecord);
    }

    records
}

/// Resolve a virtual source path to a real filesystem path.
///
/// Tries each root in order. The first root where
/// `<root>/<trimmed_virtual_path>` exists on disk is used.
/// Falls back to the original virtual path if no root matches.
fn resolve_source_path(virtual_path: &str, source_roots: &[PathBuf]) -> String {
    if source_roots.is_empty() {
        return virtual_path.to_string();
    }
    let trimmed = virtual_path.strip_prefix('/').unwrap_or(virtual_path);
    for root in source_roots {
        let candidate = root.join(trimmed);
        if candidate.exists() {
            return candidate.to_string_lossy().into_owned();
        }
    }
    // No root matched — use the first root as a best-effort prefix.
    source_roots[0].join(trimmed).to_string_lossy().into_owned()
}

/// Shell out to `genhtml` to produce an HTML coverage report.
///
/// `genhtml` is part of the `lcov` system package (installed via
/// `brew install lcov` / `apt install lcov`).
///
/// Returns `Ok(())` if `genhtml` succeeds, or a descriptive error string
/// if the binary is not found or fails.
///
/// # Errors
///
/// Returns an error string if `genhtml` is not installed, exits with a
/// non-zero status, or cannot be spawned.
pub fn generate_html(lcov_path: &Path, output_dir: &Path) -> Result<(), String> {
    let status = std::process::Command::new("genhtml")
        .arg(lcov_path)
        .arg("--output-directory")
        .arg(output_dir)
        .arg("--branch-coverage")
        .arg("--function-coverage")
        .arg("--synthesize-missing")
        .arg("--ignore-errors")
        .arg("format,source,category")
        .status();

    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("genhtml exited with {s}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err("genhtml not found. Install the lcov package: \
             brew install lcov (macOS) / apt install lcov (Linux)"
                .into())
        }
        Err(e) => Err(format!("Failed to run genhtml: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use smol_str::SmolStr;

    use super::*;

    #[test]
    fn build_lcov_records_empty_map() {
        let map = CoverageMap::new();
        let records = build_lcov_records(&map, &[]);
        assert!(records.is_empty());
    }

    #[test]
    fn build_lcov_records_basic() {
        let mut map = CoverageMap::new();
        let src = SmolStr::new("test.pure");

        map.mark_coverable(&src, 1);
        map.mark_coverable(&src, 2);
        map.mark_coverable(&src, 3);
        map.record_hit(&src, 1);
        map.record_hit(&src, 2);

        let records = build_lcov_records(&map, &[]);

        // Should contain: TN, SF, FNF, FNH, BRF, BRH, DA×3, LF, LH, end_of_record
        assert!(records.iter().any(|r| matches!(r, Record::TestName { .. })));
        assert!(
            records
                .iter()
                .any(|r| matches!(r, Record::SourceFile { .. }))
        );
        assert!(records.iter().any(|r| matches!(r, Record::EndOfRecord)));

        // Verify line data.
        let line_data: Vec<_> = records
            .iter()
            .filter_map(|r| {
                if let Record::LineData { line, count, .. } = r {
                    Some((*line, *count))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(line_data.len(), 3);
        assert!(line_data.contains(&(1, 1)));
        assert!(line_data.contains(&(2, 1)));
        assert!(line_data.contains(&(3, 0)));

        // Verify summary records.
        assert!(
            records
                .iter()
                .any(|r| matches!(r, Record::LinesFound { found: 3 }))
        );
        assert!(
            records
                .iter()
                .any(|r| matches!(r, Record::LinesHit { hit: 2 }))
        );
    }

    #[test]
    fn lcov_roundtrip_format() {
        let mut map = CoverageMap::new();
        let src = SmolStr::new("test.pure");
        map.mark_coverable(&src, 1);
        map.record_hit(&src, 1);

        let records = build_lcov_records(&map, &[]);
        let output: String = records.iter().map(|r| r.to_string() + "\n").collect();

        // Verify the output can be parsed back.
        let reader = lcov::Reader::new(output.as_bytes());
        let parsed: Vec<_> = reader.collect::<Result<Vec<_>, _>>().unwrap();
        assert!(!parsed.is_empty());

        // Verify TN record is first.
        assert!(matches!(parsed[0], Record::TestName { .. }));
    }
}
