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

//! Stage-5 smoke tests for the Relational class-mapping body
//! sub-grammar. Every fixture parses a `###Mapping` source through
//! `MappingSectionParser::with_body_parsers` registered with
//! [`RelationalClassMappingBodyParser`] and checks the structured
//! body shape; the round-trip ones additionally compose the parsed
//! mapping back to source and re-parse to assert AST equality
//! modulo `source_info`.

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef};
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_relational::ast::{
    JoinColWithDbOrConstant, MappingElement, OpColumn, OpLiteral, RelationalClassMappingBody,
    ScopedMapping, SingleMappingLine,
};
use legend_pure_dsl_relational::parser::RelationalClassMappingBodyParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "class_mapping_body_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::with_body_parsers(vec![
            Box::new(RelationalClassMappingBodyParser),
        ]))],
    )
    .expect("source must parse")
}

fn first_relational_body(source: &str) -> RelationalClassMappingBody {
    let file = parse(source);
    let section = file
        .sections
        .iter()
        .find(|s| s.kind.as_str() == "Mapping")
        .expect("expected ###Mapping section");
    let AstElement::DSLElement(boxed) = section
        .elements
        .first()
        .expect("expected one mapping element")
    else {
        panic!("expected DSLElement");
    };
    let mapping = boxed
        .as_any()
        .downcast_ref::<MappingDef>()
        .expect("expected MappingDef");
    let class_mapping = mapping
        .class_mappings
        .first()
        .expect("expected at least one class mapping");
    let ClassMappingBody::Foreign(boxed) = &class_mapping.body else {
        panic!("expected Foreign body");
    };
    boxed
        .as_any()
        .downcast_ref::<RelationalClassMappingBody>()
        .expect("expected RelationalClassMappingBody")
        .clone()
}

// ---------------------------------------------------------------------------
// Bare `prop : col` mapping line
// ---------------------------------------------------------------------------

#[test]
fn parses_bare_property_mapping() {
    let body = first_relational_body(indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            (legalName : [pkg::FirmDb]FirmTable.legal_name)
          }
        )
    "});
    assert!(body.filter.is_none());
    assert!(!body.distinct);
    assert!(body.main_table.is_none());
    assert_eq!(body.mapping_elements.len(), 1);
    let MappingElement::Single(SingleMappingLine::NonePlus(line)) = &body.mapping_elements[0]
    else {
        panic!("expected bare mapping line");
    };
    assert_eq!(line.property.value.as_str(), "legalName");
    let col = line
        .mapping
        .value
        .column
        .as_ref()
        .expect("expected column terminal");
    let OpColumn::Aliased { alias, scope, .. } = col else {
        panic!("expected aliased column");
    };
    assert_eq!(alias.value.as_str(), "FirmTable");
    assert_eq!(scope.len(), 1);
    assert_eq!(scope[0].value.as_str(), "legal_name");
}

// ---------------------------------------------------------------------------
// scope() wrapper + ~distinct
// ---------------------------------------------------------------------------

#[test]
fn parses_scope_wrapper_and_distinct_flag() {
    let body = first_relational_body(indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            ~distinct
            (scope([pkg::FirmDb]FirmTable) (legalName : legal_name))
          }
        )
    "});
    assert!(body.distinct);
    assert_eq!(body.mapping_elements.len(), 1);
    let MappingElement::Scope(ScopedMapping {
        scope,
        mapping_lines,
        ..
    }) = &body.mapping_elements[0]
    else {
        panic!("expected scope wrapper");
    };
    let scope = scope.as_ref().expect("expected simpleScopeInfo");
    assert_eq!(scope.table.value.as_str(), "FirmTable");
    assert_eq!(mapping_lines.len(), 1);
}

// ---------------------------------------------------------------------------
// Headers ~mainTable + ~primaryKey + ~filter
// ---------------------------------------------------------------------------

#[test]
fn parses_main_table_primary_key_filter_headers() {
    let body = first_relational_body(indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            ~mainTable [pkg::FirmDb]FirmTable
            ~primaryKey([pkg::FirmDb]FirmTable.id)
            ~filter [pkg::FirmDb]activeOnly
            (legalName : [pkg::FirmDb]FirmTable.legal_name)
          }
        )
    "});
    let main_table = body.main_table.as_ref().expect("~mainTable required");
    assert_eq!(main_table.scope.table.value.as_str(), "FirmTable");
    let primary_key = body.primary_key.as_ref().expect("~primaryKey required");
    assert_eq!(primary_key.len(), 1);
    let filter = body.filter.as_ref().expect("~filter required");
    assert_eq!(filter.filter_name.value.as_str(), "activeOnly");
}

// ---------------------------------------------------------------------------
// Plus-form mapping line (`+ prop : Type[mult] : col`)
// ---------------------------------------------------------------------------

#[test]
fn parses_plus_form_mapping_line_with_local_property() {
    let body = first_relational_body(indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            (+ legalName : pkg::String[1] : [pkg::FirmDb]FirmTable.legal_name)
          }
        )
    "});
    let MappingElement::Single(SingleMappingLine::Plus(plus)) = &body.mapping_elements[0] else {
        panic!("expected plus-form line");
    };
    assert_eq!(plus.property.value.as_str(), "legalName");
    assert_eq!(plus.local.type_path.name.as_str(), "String");
    assert_eq!(plus.local.mult_lower.value.as_str(), "1");
    assert!(plus.local.mult_upper.is_none());
}

// ---------------------------------------------------------------------------
// joinSequence in a mapping value
// ---------------------------------------------------------------------------

#[test]
fn parses_join_sequence_in_mapping_value() {
    let body = first_relational_body(indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            (productName : [pkg::FirmDb]@firmProduct | productTable.name)
          }
        )
    "});
    let MappingElement::Single(SingleMappingLine::NonePlus(line)) = &body.mapping_elements[0]
    else {
        panic!("expected bare mapping line");
    };
    let JoinColWithDbOrConstant { join, column, .. } = &line.mapping.value;
    let join = join.as_ref().expect("expected join sequence");
    assert_eq!(join.head.name.value.as_str(), "firmProduct");
    let col = column.as_ref().expect("expected | op_column tail");
    let OpColumn::Aliased { alias, scope, .. } = col else {
        panic!("expected aliased column tail");
    };
    assert_eq!(alias.value.as_str(), "productTable");
    assert_eq!(scope[0].value.as_str(), "name");
}

// ---------------------------------------------------------------------------
// Constant literal in a mapping value
// ---------------------------------------------------------------------------

#[test]
fn parses_constant_literal_in_mapping_value() {
    let body = first_relational_body(indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            (status : 'ACTIVE')
          }
        )
    "});
    let MappingElement::Single(SingleMappingLine::NonePlus(line)) = &body.mapping_elements[0]
    else {
        panic!("expected bare mapping line");
    };
    let lit = line
        .mapping
        .value
        .literal
        .as_ref()
        .expect("expected literal");
    assert!(matches!(lit, OpLiteral::String { value, .. } if value.as_str() == "ACTIVE"));
}

// ---------------------------------------------------------------------------
// Round-trip: parse → compose → re-parse → AST equal
// ---------------------------------------------------------------------------

#[test]
fn round_trips_full_class_mapping_body() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::FirmMapping
        (
          Firm : Relational
          {
            ~mainTable [pkg::FirmDb]FirmTable
            ~primaryKey([pkg::FirmDb]FirmTable.id)
            ~filter [pkg::FirmDb]activeOnly
            ~distinct
            (legalName : [pkg::FirmDb]FirmTable.legal_name, productCount : 0)
          }
        )
    "};
    let file1 = parse(source);
    let body1 = first_relational_body(source);

    // Compose the mapping back.
    let composed =
        legend_pure_dsl_mapping::compose::compose_mapping_section(&collect_mappings(&file1));
    let body2 = first_relational_body(&composed);

    let strip = |s: &str| {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.char_indices().peekable();
        while let Some((i, _)) = chars.peek().copied() {
            if s[i..].starts_with("source_info:") {
                let after_ident = i + "source_info:".len();
                let Some(brace) = s[after_ident..].find('{') else {
                    break;
                };
                out.push_str("source_info: <stripped>");
                let mut depth = 1usize;
                let mut idx = after_ident + brace + 1;
                while depth > 0 && idx < s.len() {
                    let c = s.as_bytes()[idx] as char;
                    if c == '{' {
                        depth += 1;
                    } else if c == '}' {
                        depth -= 1;
                    }
                    idx += 1;
                }
                while let Some(&(j, _)) = chars.peek() {
                    if j >= idx {
                        break;
                    }
                    chars.next();
                }
                continue;
            }
            let (_, c) = chars.next().expect("checked above");
            out.push(c);
        }
        out
    };
    assert_eq!(
        strip(&format!("{body1:#?}")),
        strip(&format!("{body2:#?}")),
        "AST diverged after round-trip\nsource:\n{source}\ncomposed:\n{composed}"
    );
}

fn collect_mappings(file: &SourceFile) -> Vec<&MappingDef> {
    let mut out = Vec::new();
    for section in &file.sections {
        if section.kind.as_str() != "Mapping" {
            continue;
        }
        for elem in &section.elements {
            if let AstElement::DSLElement(boxed) = elem {
                if let Some(m) = boxed.as_any().downcast_ref::<MappingDef>() {
                    out.push(m);
                }
            }
        }
    }
    out
}
