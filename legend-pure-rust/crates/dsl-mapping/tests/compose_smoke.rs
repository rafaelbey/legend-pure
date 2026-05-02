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

//! Composer round-trip: parse → compose → parse must reproduce the
//! same `MappingDef` shape (modulo source_info offsets, which the
//! second parse will recompute against the new layout).

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef};
use legend_pure_dsl_mapping::compose::compose_mapping_section;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;

/// Strip `SourceInfo { … }` blocks from a Debug rendering so two
/// expressions parsed from physically different positions can be
/// compared structurally.
fn strip_source_info(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.char_indices().peekable();
    while let Some((i, _)) = chars.next() {
        if s[i..].starts_with("source_info: SourceInfo { ") {
            // Skip until matching closing brace at this nesting level.
            let mut depth = 0;
            for (j, c) in s[i..].char_indices() {
                if c == '{' {
                    depth += 1;
                } else if c == '}' {
                    depth -= 1;
                    if depth == 0 {
                        // Advance the outer iterator past `source_info … }`.
                        let end = i + j + 1;
                        while let Some(&(k, _)) = chars.peek() {
                            if k >= end {
                                break;
                            }
                            chars.next();
                        }
                        break;
                    }
                }
            }
        } else {
            out.push(s[i..].chars().next().unwrap());
        }
    }
    out
}

fn parse(name: &str, source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        name,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser)],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn collect_mappings(file: &SourceFile) -> Vec<MappingDef> {
    let mut out = Vec::new();
    for section in &file.sections {
        for elem in &section.elements {
            if let AstElement::DSLElement(boxed) = elem
                && let Some(m) = boxed.as_any().downcast_ref::<MappingDef>()
            {
                out.push(m.clone());
            }
        }
    }
    out
}

fn assert_round_trip(source: &str) {
    let file1 = parse("round_trip_1.pure", source);
    let mappings1 = collect_mappings(&file1);
    let composed = compose_mapping_section(&mappings1.iter().collect::<Vec<_>>());

    let file2 = parse("round_trip_2.pure", &composed);
    let mappings2 = collect_mappings(&file2);

    assert_eq!(
        mappings1.len(),
        mappings2.len(),
        "mapping count diverged after round-trip\nfirst:  {mappings1:#?}\nrecomp: {composed}\nsecond: {mappings2:#?}"
    );
    for (m1, m2) in mappings1.iter().zip(mappings2.iter()) {
        assert_eq!(m1.name.value, m2.name.value, "name diverged");
        assert_eq!(
            m1.includes.len(),
            m2.includes.len(),
            "include count diverged"
        );
        assert_eq!(
            m1.class_mappings.len(),
            m2.class_mappings.len(),
            "class-mapping count diverged"
        );
        for (cm1, cm2) in m1.class_mappings.iter().zip(m2.class_mappings.iter()) {
            assert_eq!(cm1.is_root, cm2.is_root, "is_root diverged");
            assert_eq!(cm1.class.name, cm2.class.name, "class name diverged");
            assert_eq!(cm1.id, cm2.id, "id diverged");
            assert_eq!(cm1.extends, cm2.extends, "extends diverged");
            let ClassMappingBody::Pure(b1) = &cm1.body else {
                unreachable!("Stage 2 only emits Pure bodies");
            };
            let ClassMappingBody::Pure(b2) = &cm2.body else {
                unreachable!("Stage 2 only emits Pure bodies");
            };
            assert_eq!(
                b1.src_class.as_ref().map(|p| &p.name),
                b2.src_class.as_ref().map(|p| &p.name),
                "~src diverged"
            );
            assert_eq!(
                b1.filter.is_some(),
                b2.filter.is_some(),
                "~filter presence diverged"
            );
            assert_eq!(
                b1.property_mappings.len(),
                b2.property_mappings.len(),
                "property mapping count diverged"
            );
            for (p1, p2) in b1.property_mappings.iter().zip(b2.property_mappings.iter()) {
                assert_eq!(p1.property_name, p2.property_name, "property name diverged");
                // Structural transform check: the AST debug
                // representation must match modulo source_info offsets.
                // This catches a composer that emits garbage Pure for
                // the transform body — it would produce a *different*
                // Expression on the second parse.
                let p1_debug = strip_source_info(&format!("{:?}", p1.transform));
                let p2_debug = strip_source_info(&format!("{:?}", p2.transform));
                assert_eq!(
                    p1_debug, p2_debug,
                    "transform expression diverged after round-trip for property '{}'",
                    p1.property_name
                );
            }
        }
    }
}

#[test]
fn round_trip_minimal_mapping() {
    assert_round_trip(indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            legalName : $src.name
          }
        )
    "});
}

#[test]
fn round_trip_with_filter_and_multiple_properties() {
    assert_round_trip(indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            ~filter $src.name == 'GS'
            legalName : $src.name,
            count : $src.employees
          }
        )
    "});
}

#[test]
fn round_trip_with_includes_and_root_marker() {
    assert_round_trip(indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          include pkg::Other

          *pkg::Firm[firm_id] : Pure
          {
            legalName : 'literal'
          }
        )
    "});
}
