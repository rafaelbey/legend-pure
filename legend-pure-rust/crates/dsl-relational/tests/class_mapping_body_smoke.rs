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
    JoinColWithDbOrConstant, MappingElement, NonePlusMappingValue, OpColumn, OpLiteral,
    RelationalClassMappingBody, ScopedMapping, SingleMappingLine,
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
    let NonePlusMappingValue::Relational(rm) = &line.value else {
        panic!("expected relational mapping value");
    };
    let col = rm.value.column.as_ref().expect("expected column terminal");
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
    let NonePlusMappingValue::Relational(rm) = &line.value else {
        panic!("expected relational mapping value");
    };
    let JoinColWithDbOrConstant { join, column, .. } = &rm.value;
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
    let NonePlusMappingValue::Relational(rm) = &line.value else {
        panic!("expected relational mapping value");
    };
    let lit = rm.value.literal.as_ref().expect("expected literal");
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
            if let AstElement::DSLElement(boxed) = elem
                && let Some(m) = boxed.as_any().downcast_ref::<MappingDef>()
            {
                out.push(m);
            }
        }
    }
    out
}

// ===========================================================================
// Stage 6: embedded / inline / otherwise property mappings
// ===========================================================================

mod embedded {
    use super::first_relational_body;
    use indoc::indoc;
    use legend_pure_dsl_relational::ast::{
        EmbeddedMappingTrailer, MappingElement, NonePlusMappingValue, SingleMappingLine,
    };

    fn embedded_value(
        body: &legend_pure_dsl_relational::ast::RelationalClassMappingBody,
    ) -> &legend_pure_dsl_relational::ast::EmbeddedMapping {
        let MappingElement::Single(SingleMappingLine::NonePlus(line)) = &body.mapping_elements[0]
        else {
            panic!("expected bare mapping line");
        };
        let NonePlusMappingValue::Embedded(e) = &line.value else {
            panic!("expected embedded mapping; got {:?}", line.value);
        };
        e
    }

    #[test]
    fn parses_simple_embedded_mapping() {
        let body = first_relational_body(indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm : Relational
              {
                (details (taxLocation : [pkg::FirmDb]FirmTable.tax_location))
              }
            )
        "});
        let e = embedded_value(&body);
        assert!(e.primary_key.is_none());
        assert!(e.trailer.is_none());
        assert_eq!(e.mapping_lines.len(), 1);
    }

    #[test]
    fn parses_embedded_with_primary_key() {
        let body = first_relational_body(indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm : Relational
              {
                (details (
                  ~primaryKey([pkg::FirmDb]FirmTable.id)
                  taxLocation : [pkg::FirmDb]FirmTable.tax_location
                ))
              }
            )
        "});
        let e = embedded_value(&body);
        let pk = e.primary_key.as_ref().expect("~primaryKey required");
        assert_eq!(pk.len(), 1);
    }

    #[test]
    fn parses_embedded_inline_trailer() {
        let body = first_relational_body(indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm : Relational
              {
                (details () Inline[employeeMapping])
              }
            )
        "});
        let e = embedded_value(&body);
        let Some(EmbeddedMappingTrailer::Inline(r)) = &e.trailer else {
            panic!("expected Inline trailer; got {:?}", e.trailer);
        };
        assert_eq!(r.id.value.as_str(), "employeeMapping");
    }

    #[test]
    fn parses_embedded_otherwise_trailer() {
        let body = first_relational_body(indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm : Relational
              {
                (details () Otherwise([taxLocation] : [pkg::FirmDb]@firmDetails))
              }
            )
        "});
        let e = embedded_value(&body);
        let Some(EmbeddedMappingTrailer::Otherwise(maps)) = &e.trailer else {
            panic!("expected Otherwise trailer; got {:?}", e.trailer);
        };
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].property.value.as_str(), "taxLocation");
    }

    #[test]
    fn round_trips_embedded_with_primary_key_and_otherwise() {
        let source = indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm : Relational
              {
                (details (
                  ~primaryKey([pkg::FirmDb]FirmTable.id)
                  taxLocation : [pkg::FirmDb]FirmTable.tax_location
                ) Otherwise([taxLocation] : [pkg::FirmDb]@firmDetails))
              }
            )
        "};
        let body1 = first_relational_body(source);
        let composed = legend_pure_dsl_mapping::compose::compose_mapping_section(
            &super::collect_mappings(&super::parse(source)),
        );
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
            "embedded round-trip diverged\nsource:\n{source}\ncomposed:\n{composed}"
        );
    }
}

// ===========================================================================
// Stage 7: relational AssociationMapping
// ===========================================================================

mod association {
    use super::first_relational_body;
    use indoc::indoc;
    use legend_pure_dsl_relational::ast::SingleMappingLine;

    #[test]
    fn parses_association_mapping_body() {
        let body = first_relational_body(indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm_Person : Relational
              {
                AssociationMapping
                (
                  employees[fir1, per1] : [pkg::FirmDb]@firmJoin,
                  firm[per1, fir1] : [pkg::FirmDb]@firmJoin
                )
              }
            )
        "});
        let lines = body
            .association_mapping
            .as_ref()
            .expect("expected AssociationMapping body");
        assert_eq!(lines.len(), 2);
        let SingleMappingLine::NonePlus(employees) = &lines[0] else {
            panic!("expected bare line");
        };
        assert_eq!(employees.property.value.as_str(), "employees");
        assert_eq!(
            employees
                .source_id
                .as_ref()
                .expect("expected srcId")
                .value
                .as_str(),
            "fir1"
        );
        assert_eq!(
            employees
                .target_id
                .as_ref()
                .expect("expected targetId")
                .value
                .as_str(),
            "per1"
        );
        // The other body fields stay empty for an association mapping.
        assert!(body.main_table.is_none());
        assert!(body.mapping_elements.is_empty());
    }

    #[test]
    fn round_trips_association_mapping() {
        let source = indoc! {r"
            ###Mapping
            Mapping pkg::FirmMapping
            (
              Firm_Person : Relational
              {
                AssociationMapping
                (
                  employees[fir1, per1] : [pkg::FirmDb]@firmJoin,
                  firm[per1, fir1] : [pkg::FirmDb]@firmJoin
                )
              }
            )
        "};
        let body1 = first_relational_body(source);
        let composed = legend_pure_dsl_mapping::compose::compose_mapping_section(
            &super::collect_mappings(&super::parse(source)),
        );
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
            "association round-trip diverged\nsource:\n{source}\ncomposed:\n{composed}"
        );
    }
}
