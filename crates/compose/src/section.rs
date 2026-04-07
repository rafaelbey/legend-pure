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

//! Section and source file composer.
//!
//! Renders the full source file with `###Section` headers and `import` statements.

use legend_pure_parser_ast::section::SourceFile;

use crate::element::compose_element;
use crate::expression::compose_package;
use crate::writer::IndentWriter;

/// Composes a full `SourceFile` to Pure grammar text.
///
/// This is the main entry point for the compose crate.
///
/// # Output Format
///
/// Single-section "Pure" files omit the `###Pure` header.
/// Multi-section files include `###SectionName` headers.
/// Elements within a section are separated by blank lines.
#[must_use]
pub fn compose_source_file(sf: &SourceFile) -> String {
    let mut w = IndentWriter::new();
    let is_single_pure_section =
        sf.sections.len() == 1 && sf.sections[0].kind == "Pure";

    for (si, section) in sf.sections.iter().enumerate() {
        // Section headers (e.g., `###Pure`)
        //
        // Single-section Pure files omit the header because Pure is the
        // default section type. Multi-section files and non-Pure sections
        // always emit the header.
        if !is_single_pure_section {
            if si > 0 {
                w.newline();
            }
            w.write("###");
            w.write(&section.kind);
            w.newline();
        }

        // Import statements
        for import in &section.imports {
            w.write("import ");
            compose_package(&mut w, &import.path);
            w.write_line("::*;");
        }

        // Elements
        for (ei, element) in section.elements.iter().enumerate() {
            compose_element(&mut w, element);

            // Blank line between elements (but not after the last)
            let is_last_element = ei == section.elements.len() - 1;
            let is_last_section = si == sf.sections.len() - 1;
            if !is_last_element || !is_last_section {
                w.newline();
            }
        }
    }

    w.finish()
}
