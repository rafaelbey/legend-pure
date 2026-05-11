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

//! Parser smoke: minimal `###Mapping` blocks parse into the expected
//! AST shape via `parse_with_sections` + `MappingSectionParser`.

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef};
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;

fn parse(source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "parser_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn first_mapping(file: &SourceFile) -> &MappingDef {
    for section in &file.sections {
        for elem in &section.elements {
            if let AstElement::DSLElement(boxed) = elem
                && let Some(m) = boxed.as_any().downcast_ref::<MappingDef>()
            {
                return m;
            }
        }
    }
    panic!("no MappingDef found in parsed source");
}

#[test]
fn parses_minimal_mapping_with_pure_body() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            legalName : $src.name
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    assert_eq!(m.name.value.as_str(), "M");
    assert_eq!(m.class_mappings.len(), 1, "expected one class mapping");
    let cm = &m.class_mappings[0];
    assert_eq!(cm.class.name.as_str(), "Firm");
    let ClassMappingBody::Pure(body) = &cm.body else {
        panic!("expected Pure body variant");
    };
    assert!(body.src_class.is_some(), "expected ~src clause");
    assert_eq!(body.src_class.as_ref().unwrap().name.as_str(), "FirmSource");
    assert_eq!(body.property_mappings.len(), 1);
    assert_eq!(
        body.property_mappings[0].property_name.as_str(),
        "legalName"
    );
}

#[test]
fn parses_mapping_with_filter_and_multiple_properties() {
    let source = indoc! {r"
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
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let ClassMappingBody::Pure(body) = &cm.body else {
        panic!("expected Pure body variant");
    };
    assert!(body.filter.is_some(), "expected ~filter clause");
    assert_eq!(body.property_mappings.len(), 2);
    assert_eq!(body.property_mappings[1].property_name.as_str(), "count");
}

#[test]
fn parses_mapping_with_includes_and_root_marker() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          include pkg::Other

          *pkg::Firm[firm_id] : Pure
          {
            legalName : 'literal'
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    assert_eq!(m.includes.len(), 1);
    assert_eq!(m.includes[0].included.name.as_str(), "Other");
    let cm = &m.class_mappings[0];
    assert!(cm.is_root, "expected root marker (*)");
    assert_eq!(cm.id.as_deref(), Some("firm_id"));
}

#[test]
fn unknown_parser_name_errors_with_registration_hint() {
    // `MadeUpKind` is neither a built-in (Pure / Operation /
    // EnumerationMapping / AggregationAware / XStore / Relation) nor
    // a registered foreign body parser (Relational lives in
    // dsl-relational and isn't wired here). Exercise the dispatch
    // fallthrough that rejects unknown sub-parser names and points
    // at the registration mechanism.
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : MadeUpKind
          {
            anything
          }
        )
    "};
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        "unknown_subparser.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    let partial = result.expect_err("expected parse error for unsupported sub-parser");
    let msgs: Vec<String> = partial.errors.iter().map(ToString::to_string).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("MadeUpKind") && m.contains("with_body_parsers")),
        "expected error mentioning the unsupported sub-parser name and the registration \
         mechanism; got {msgs:?}"
    );
}

// ---------------------------------------------------------------------------
// Relation body kind (T2.2) — parser + composer round-trip
// ---------------------------------------------------------------------------

#[test]
fn parses_relation_body_with_single_property_mapping() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Relation
          {
            ~func pkg::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            legalName : name_col
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let legend_pure_dsl_mapping::ast::ClassMappingBody::RelationFunction(body) = &cm.body else {
        panic!("expected RelationFunction body");
    };
    assert_eq!(
        body.relation_function.name.as_str(),
        "myFn",
        "function FQN parsed cleanly"
    );
    assert!(
        body.function_signature_suffix.is_some(),
        "():Return[m] suffix captured for round-trip"
    );
    assert_eq!(body.property_mappings.len(), 1);
    let pm = &body.property_mappings[0];
    assert_eq!(pm.property_name.as_str(), "legalName");
    assert_eq!(pm.column.as_str(), "name_col");
    assert!(pm.binding_transformer.is_none());
    assert!(pm.local_mapping_property.is_none());
}

#[test]
fn parses_relation_body_with_multiple_property_mappings_comma_separated() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Relation
          {
            ~func pkg::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            firstName : fname_col,
            lastName : lname_col,
            age : age_col
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let legend_pure_dsl_mapping::ast::ClassMappingBody::RelationFunction(body) = &cm.body else {
        panic!("expected RelationFunction body");
    };
    assert_eq!(body.property_mappings.len(), 3);
    let names: Vec<&str> = body
        .property_mappings
        .iter()
        .map(|pm| pm.property_name.as_str())
        .collect();
    assert_eq!(names, vec!["firstName", "lastName", "age"]);
}

#[test]
fn parses_relation_body_with_binding_transformer() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Relation
          {
            ~func pkg::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            data : Binding pkg::TextBinding : text_col
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let legend_pure_dsl_mapping::ast::ClassMappingBody::RelationFunction(body) = &cm.body else {
        panic!("expected RelationFunction body");
    };
    let pm = &body.property_mappings[0];
    assert_eq!(pm.property_name.as_str(), "data");
    let bt = pm
        .binding_transformer
        .as_ref()
        .expect("binding transformer must be captured");
    assert_eq!(bt.binding.name.as_str(), "TextBinding");
    assert_eq!(pm.column.as_str(), "text_col");
}

#[test]
fn parses_relation_body_with_local_mapping_property() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Relation
          {
            ~func pkg::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            +computed : String[1] : composite_col
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let legend_pure_dsl_mapping::ast::ClassMappingBody::RelationFunction(body) = &cm.body else {
        panic!("expected RelationFunction body");
    };
    let pm = &body.property_mappings[0];
    assert_eq!(pm.property_name.as_str(), "computed");
    let local = pm
        .local_mapping_property
        .as_ref()
        .expect("local mapping property must be captured");
    assert_eq!(local.type_ref.name.as_str(), "String");
    assert_eq!(pm.column.as_str(), "composite_col");
}

#[test]
fn relation_body_round_trips_through_composer() {
    use legend_pure_dsl_mapping::compose::compose_mapping_section;

    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Relation
          {
            ~func pkg::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            firstName : fname_col,
            data : Binding pkg::TextBinding : text_col,
            +computed : String[1] : composite_col
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let composed = compose_mapping_section(&[m]);
    // Token-survival check: every distinct token in the source body
    // must appear in the composed text.
    for token in [
        "~func",
        "myFn",
        "Relation<Any>",
        "[1]",
        "firstName",
        "fname_col",
        "Binding",
        "TextBinding",
        "text_col",
        "+computed",
        "String[1]",
        "composite_col",
    ] {
        assert!(
            composed.contains(token),
            "composer dropped token {token:?}; composed:\n{composed}"
        );
    }
    // Round-trip parse to confirm structural fidelity.
    let file2 = parse(&composed);
    let m2 = first_mapping(&file2);
    let cm2 = &m2.class_mappings[0];
    let legend_pure_dsl_mapping::ast::ClassMappingBody::RelationFunction(body2) = &cm2.body else {
        panic!("expected RelationFunction body after round-trip");
    };
    assert_eq!(body2.property_mappings.len(), 3);
    assert!(body2.property_mappings[1].binding_transformer.is_some());
    assert!(body2.property_mappings[2].local_mapping_property.is_some());
}

// ---------------------------------------------------------------------------
// `*` explode marker on Pure property mappings (Java M3CoreParser.g4:84
// `mappingLine: ... STAR? COLON ...`).
// ---------------------------------------------------------------------------

#[test]
fn parses_pure_property_mapping_with_explode_marker() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            aliases *: $src.aliasList,
            name : $src.name
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let ClassMappingBody::Pure(body) = &cm.body else {
        panic!("expected Pure body variant");
    };
    assert_eq!(body.property_mappings.len(), 2);
    assert_eq!(body.property_mappings[0].property_name.as_str(), "aliases");
    assert!(
        body.property_mappings[0].explode,
        "expected explode=true on `aliases *: …`"
    );
    assert_eq!(body.property_mappings[1].property_name.as_str(), "name");
    assert!(
        !body.property_mappings[1].explode,
        "expected explode=false on regular `name : …`"
    );
}

#[test]
fn pure_property_mapping_without_star_keeps_explode_false() {
    // Pin against accidental wiring that would set explode=true on
    // every property mapping. The parser may have read its way past
    // a `*` token earlier in the source (none here) — verify the
    // bool stays false for plain `name : transform`.
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            legalName : $src.name
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let ClassMappingBody::Pure(body) = &cm.body else {
        panic!("expected Pure body");
    };
    assert!(!body.property_mappings[0].explode);
}

#[test]
fn explode_marker_round_trips_through_composer() {
    use legend_pure_dsl_mapping::compose::compose_mapping_section;

    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm : Pure
          {
            ~src pkg::FirmSource
            aliases *: $src.aliasList
          }
        )
    "};
    let file = parse(source);
    let m = first_mapping(&file);
    let composed = compose_mapping_section(&[m]);
    // The composer is allowed to insert spaces around the `*`
    // explode marker. Assert the token is present after the
    // property name; structural round-trip (next step) is the
    // load-bearing fidelity check.
    assert!(
        composed.contains("aliases *"),
        "composer must replay the `*` explode marker after the property name; composed:\n{composed}"
    );

    // Round-trip parse to confirm structural fidelity.
    let file2 = parse(&composed);
    let m2 = first_mapping(&file2);
    let cm = &m2.class_mappings[0];
    let ClassMappingBody::Pure(body) = &cm.body else {
        panic!("expected Pure body after round-trip");
    };
    assert_eq!(body.property_mappings.len(), 1);
    assert!(
        body.property_mappings[0].explode,
        "explode flag must survive parse → compose → parse"
    );
}
