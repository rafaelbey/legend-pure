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

//! Conversions between the Pure compiler's `SourceInfo` (1-indexed)
//! and `lsp_types::Range`/`Position` (0-indexed).
//!
//! The 1→0 shift is the only sharp edge in the LSP layer.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::error::Severity;
use tower_lsp_server::ls_types::{DiagnosticSeverity, Position, Range};

/// Convert a 1-indexed compiler position to a 0-indexed LSP position.
#[must_use]
pub fn position_from_1indexed(line: u32, column: u32) -> Position {
    Position {
        line: line.saturating_sub(1),
        character: column.saturating_sub(1),
    }
}

/// Convert an LSP `Position` (0-indexed) to a 1-indexed compiler
/// `(line, column)` pair.
#[must_use]
pub fn position_to_1indexed(p: Position) -> (u32, u32) {
    (p.line + 1, p.character + 1)
}

/// Convert a [`SourceInfo`] span to an LSP [`Range`].
#[must_use]
pub fn range_from_source_info(si: &SourceInfo) -> Range {
    Range {
        start: position_from_1indexed(si.start_line, si.start_column),
        end: position_from_1indexed(si.end_line, si.end_column),
    }
}

/// Map a Pure-compiler [`Severity`] to an LSP [`DiagnosticSeverity`].
#[must_use]
pub fn diagnostic_severity(s: Severity) -> DiagnosticSeverity {
    match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Info => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_indexes_shift_by_one() {
        let p = position_from_1indexed(3, 7);
        assert_eq!(p.line, 2);
        assert_eq!(p.character, 6);
        let (l, c) = position_to_1indexed(p);
        assert_eq!((l, c), (3, 7));
    }

    #[test]
    fn position_zero_columns_clamp_to_zero() {
        // Synthetic spans (e.g. compile errors with no real position)
        // arrive as `0` lines/columns. They must not underflow.
        let p = position_from_1indexed(0, 0);
        assert_eq!(p.line, 0);
        assert_eq!(p.character, 0);
    }

    #[test]
    fn severity_mapping_is_total() {
        for s in [
            Severity::Error,
            Severity::Warning,
            Severity::Info,
            Severity::Hint,
        ] {
            let _ = diagnostic_severity(s);
        }
    }
}
