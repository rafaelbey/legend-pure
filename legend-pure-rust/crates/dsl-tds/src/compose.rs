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

//! `IslandComposer` for `#TDS\n cols\n rows\n#`.
//!
//! Tag: `"TDS"`. Emits the canonical multi-line shape so the
//! parse → compose → parse round-trip is stable:
//!
//! ```text
//! #TDS
//!   col1, col2:Type, 'col 3':Type[1]
//!   1, 2, foo
//!   3, 4, bar
//! #
//! ```
//!
//! Plug-in callers register via [`default_island_composers`] when
//! calling [`legend_pure_parser_compose::island::compose_island_with`].

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_compose::island::IslandComposer;
use legend_pure_parser_compose::writer::IndentWriter;

use crate::ast::{TAG, TDSCell, TDSColumn, TDSExpr};

/// Composer for TDS islands. Multi-line emit with two-space indent
/// for the header and rows.
pub struct TDSIslandComposer;

impl IslandComposer for TDSIslandComposer {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn compose(&self, w: &mut IndentWriter, content: &dyn IslandContent) {
        if let Some(tds) = content.as_any().downcast_ref::<TDSExpr>() {
            compose_tds(w, tds);
        }
    }
}

/// Convenience helper — `vec![Box::new(TDSIslandComposer)]` for
/// callers that want the TDS DSL composer registered.
#[must_use]
pub fn default_island_composers() -> Vec<Box<dyn IslandComposer>> {
    vec![Box::new(TDSIslandComposer)]
}

fn compose_tds(w: &mut IndentWriter, tds: &TDSExpr) {
    w.write_line("#TDS");
    w.push_indent();

    // Header line. `IndentWriter::write` auto-emits indent at the
    // start of each line, so the first `write` after a `newline`
    // (or `write_line`) prepends the current indent.
    for (i, col) in tds.columns.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_column(w, col);
    }
    w.newline();

    // Row lines.
    for row in &tds.rows {
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            compose_cell(w, cell);
        }
        w.newline();
    }

    w.pop_indent();
    w.write("#");
}

fn compose_column(w: &mut IndentWriter, col: &TDSColumn) {
    if needs_quoting(col.name.as_str()) {
        w.write("'");
        w.write(col.name.as_str());
        w.write("'");
    } else {
        w.write(col.name.as_str());
    }

    if let Some(ty) = &col.type_ref {
        w.write(":");
        w.write(ty.name.as_str());
        if let Some(mult) = &ty.multiplicity {
            w.write("[");
            w.write(mult.as_str());
            w.write("]");
        }
    }
}

fn compose_cell(w: &mut IndentWriter, cell: &TDSCell) {
    w.write(cell.raw.as_str());
}

fn needs_quoting(name: &str) -> bool {
    // Quote if the name contains anything other than identifier chars.
    !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.chars().next().is_some_and(|c| c.is_ascii_digit())
}
