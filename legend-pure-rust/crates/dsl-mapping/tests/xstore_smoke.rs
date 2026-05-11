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

//! Stage-7 XStore association-mapping coverage:
//!
//! Parsing:
//!   - parse_with_sections accepts `parserName == "XStore"` and
//!     constructs a `ClassMappingBody::XStore(...)` tree carrying
//!     `propName[srcId, tgtId]? : crossExpr` lines.
//!   - Both the `[srcId, tgtId]` and bare `propName : expr` forms
//!     parse cleanly.
//!   - Round-trip composer reproduces the parsed tree (counts +
//!     property names + ID brackets + expression token survival).
//!
//! Validation (positives + negatives):
//!   - Positive: an XStore association mapping bridging two
//!     pure-instance set implementations validates clean and the
//!     mapping registers in `MappingExtension`.
//!   - Negative: outer FQN resolves to a Class, not an Association.
//!   - Negative: property name doesn't exist on the association.
//!   - Negative: source/target set-impl ID is not declared in this
//!     mapping.

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef, XStoreClassMappingBody};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::compose::compose_mapping_section;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;

fn parse(name: &str, source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        name,
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
    panic!("no MappingDef in parsed source");
}

fn xstore_body(m: &MappingDef, idx: usize) -> &XStoreClassMappingBody {
    let cm = m
        .class_mappings
        .get(idx)
        .expect("expected at least one class mapping");
    let ClassMappingBody::XStore(body) = &cm.body else {
        panic!("class mapping {idx} is not an XStore body");
    };
    body
}

fn compile(
    sources: Vec<SourceFile>,
) -> (
    Vec<CompilationError>,
    MappingExtension,
    legend_pure_parser_pure::model::PureModel,
) {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    let (errors, model) = match result {
        Ok(model) => (Vec::new(), model),
        Err(p) => (p.errors, p.model),
    };
    (errors, extension, model)
}

// ----- Parser coverage --------------------------------------------------

#[test]
fn parses_xstore_with_set_impl_ids() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm_Person : XStore
          {
            firm[employee_set, firm_set]      : $this.firmId == $that.id,
            employees[firm_set, employee_set] : $this.id == $that.firmId
          }
        )
    "};
    let file = parse("xstore_two.pure", source);
    let body = xstore_body(first_mapping(&file), 0);
    assert_eq!(body.property_mappings.len(), 2);
    assert_eq!(body.property_mappings[0].property_name.as_str(), "firm");
    assert_eq!(
        body.property_mappings[0].source_set_impl_id.as_deref(),
        Some("employee_set")
    );
    assert_eq!(
        body.property_mappings[0].target_set_impl_id.as_deref(),
        Some("firm_set")
    );
    assert_eq!(
        body.property_mappings[1].property_name.as_str(),
        "employees"
    );
}

#[test]
fn parses_xstore_without_set_impl_ids() {
    // The `[srcId, tgtId]` annotation is optional in the Java
    // grammar (sourceAndTargetMappingId? in M3Parser.g4). Make sure
    // the bare form parses too.
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm_Person : XStore
          {
            firm : $this.firmId == $that.id
          }
        )
    "};
    let file = parse("xstore_bare.pure", source);
    let body = xstore_body(first_mapping(&file), 0);
    assert_eq!(body.property_mappings.len(), 1);
    assert!(body.property_mappings[0].source_set_impl_id.is_none());
    assert!(body.property_mappings[0].target_set_impl_id.is_none());
}

#[test]
fn round_trip_through_composer() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          pkg::Firm_Person : XStore
          {
            firm[employee_set, firm_set]      : $this.firmId == $that.id,
            employees[firm_set, employee_set] : $this.id == $that.firmId
          }
        )
    "};
    let file1 = parse("rt1.pure", source);
    let m1 = first_mapping(&file1);
    let composed = compose_mapping_section(&[m1]);

    // Defence against silent-content corruption: assert specific
    // expression tokens from the input survive into the composed
    // string before the round-trip parse — without these a composer
    // that silently dropped or rewrote a cross-expression body
    // could pass the count-only structural assertions below.
    for needle in [
        "$this.firmId",
        "$that.id",
        "$this.id",
        "$that.firmId",
        "[employee_set, firm_set]",
        "[firm_set, employee_set]",
    ] {
        assert!(
            composed.contains(needle),
            "composed XStore body lost token '{needle}'; composed:\n{composed}"
        );
    }

    let file2 = parse("rt2.pure", &composed);
    let m2 = first_mapping(&file2);
    let b1 = xstore_body(m1, 0);
    let b2 = xstore_body(m2, 0);
    assert_eq!(b1.property_mappings.len(), b2.property_mappings.len());
    for (p1, p2) in b1.property_mappings.iter().zip(b2.property_mappings.iter()) {
        assert_eq!(p1.property_name, p2.property_name);
        assert_eq!(p1.source_set_impl_id, p2.source_set_impl_id);
        assert_eq!(p1.target_set_impl_id, p2.target_set_impl_id);
    }
}

// ----- Validator coverage -----------------------------------------------

#[test]
fn xstore_bridging_two_pure_instances_validates_clean() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          id : String[1];
        }

        Class my::test::Person
        {
          firmId : String[1];
        }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::FirmPersonMapping
        (
          *my::test::Firm[firm_set] : Pure
          {
            ~src my::test::FirmSrc
            id : $src.id
          }

          *my::test::Person[employee_set] : Pure
          {
            ~src my::test::PersonSrc
            firmId : $src.firmId
          }

          my::test::Firm_Person : XStore
          {
            firm[employee_set, firm_set]      : $this.firmId == $that.id,
            employees[firm_set, employee_set] : $this.id == $that.firmId
          }
        )
    "};
    let file = parse("xstore_clean.pure", source);
    let (errors, _ext, model) = compile(vec![file]);
    let xs_errors: Vec<_> = errors
        .iter()
        .filter(|e| e.message.contains("XStore") || e.message.contains("Association "))
        .collect();
    assert!(
        xs_errors.is_empty(),
        "expected no XStore-validator errors; got: {:#?}",
        xs_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(
        MappingExtension::mappings_from_model(&model)
            .iter()
            .any(|(fqn, _)| fqn.as_str() == "my::test::FirmPersonMapping")
    );
}

#[test]
fn target_resolving_to_class_not_association_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::NotAnAssociation
        {
          n : String[1];
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::NotAnAssociation : XStore
          {
            n : $this.n == $that.n
          }
        )
    "};
    let file = parse("xstore_target_class.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("XStore mapping target")
                && e.message.contains("NotAnAssociation")
                && e.message.contains("Association")),
        "expected target-not-an-Association error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_property_on_association_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { id : String[1]; }
        Class my::test::Person { firmId : String[1]; }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure { ~src my::test::FirmSrc id : $src.id }
          *my::test::Person[employee_set] : Pure { ~src my::test::PersonSrc firmId : $src.firmId }

          my::test::Firm_Person : XStore
          {
            doesNotExist[employee_set, firm_set] : $this.firmId == $that.id
          }
        )
    "};
    let file = parse("xstore_unknown_prop.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("Association")
            && e.message.contains("Firm_Person")
            && e.message.contains("doesNotExist")),
        "expected unknown-property error mentioning 'doesNotExist'; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn bare_property_mapping_without_ids_errors_at_validate_time() {
    // Parser-permissive + validator-strict, mirroring Java's split:
    // M3 grammar accepts `propName : crossExpr` syntactically
    // (sourceAndTargetMappingId is optional in M3Parser.g4), but
    // Java's XStoreProcessor calls MappingValidator.validateId on
    // both ids and throws when null. Make sure our validator does
    // the equivalent — without the ids the crossExpression has
    // nothing to bind $this/$that to.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { id : String[1]; }
        Class my::test::Person { firmId : String[1]; }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure { ~src my::test::FirmSrc id : $src.id }
          *my::test::Person[employee_set] : Pure { ~src my::test::PersonSrc firmId : $src.firmId }

          my::test::Firm_Person : XStore
          {
            firm : $this.firmId == $that.id
          }
        )
    "};
    let file = parse("xstore_bare_validate.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message.contains("XStore property mapping")
                && e.message
                    .contains("requires source and target set-implementation IDs")
        }),
        "expected missing-set-impl-ids error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_set_impl_id_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm { id : String[1]; }
        Class my::test::Person { firmId : String[1]; }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure { ~src my::test::FirmSrc id : $src.id }
          *my::test::Person[employee_set] : Pure { ~src my::test::PersonSrc firmId : $src.firmId }

          my::test::Firm_Person : XStore
          {
            firm[notARealSetImpl, firm_set] : $this.firmId == $that.id
          }
        )
    "};
    let file = parse("xstore_unknown_setimpl.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("XStore source set-implementation")
                && e.message.contains("notARealSetImpl")),
        "expected unresolved source-set-impl error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

// ----- crossExpression lowering: Boolean[1] type-check -----------------

#[test]
fn cross_expression_with_boolean_return_validates_clean() {
    // Pins the new XStore lowering's negative side: when $this/$that
    // resolve and the cross-expression returns Boolean[1], no
    // XStoreCrossExpressionReturnType diagnostic is emitted.
    // Subset of `xstore_bridging_two_pure_instances_validates_clean`
    // with an exact error-kind assertion so future regressions in
    // `is_boolean_one` surface here.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          id : String[1];
        }

        Class my::test::Person
        {
          firmId : String[1];
        }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure
          {
            ~src my::test::FirmSrc
            id : $src.id
          }
          *my::test::Person[employee_set] : Pure
          {
            ~src my::test::PersonSrc
            firmId : $src.firmId
          }
          my::test::Firm_Person : XStore
          {
            firm[employee_set, firm_set] : $this.firmId == $that.id
          }
        )
    "};
    let file = parse("xstore_clean_bool.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    let return_type_errors: Vec<&str> = errors
        .iter()
        .filter(|e| {
            e.message.contains("XStore crossExpression on")
                && e.message.contains("must return Boolean[1]")
        })
        .map(|e| e.message.as_str())
        .collect();
    assert!(
        return_type_errors.is_empty(),
        "no XStoreCrossExpressionReturnType error expected on clean cross-expression; got: {:#?}",
        return_type_errors
    );
}

#[test]
fn cross_expression_non_boolean_return_errors() {
    // Cross-expression `$this.firmId` lowers to String[1] — not
    // Boolean[1]. The new validator must emit a pointed error
    // message naming the association property and including the
    // actual inferred type spelling.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          id : String[1];
        }

        Class my::test::Person
        {
          firmId : String[1];
        }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure
          {
            ~src my::test::FirmSrc
            id : $src.id
          }
          *my::test::Person[employee_set] : Pure
          {
            ~src my::test::PersonSrc
            firmId : $src.firmId
          }
          my::test::Firm_Person : XStore
          {
            firm[employee_set, firm_set] : $this.firmId
          }
        )
    "};
    let file = parse("xstore_non_bool.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| {
            e.message
                .contains("XStore crossExpression on 'my::test::Firm_Person.firm'")
                && e.message.contains("must return Boolean[1]")
                && e.message.contains("String")
        }),
        "expected XStoreCrossExpressionReturnType error mentioning String; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn cross_expression_unknown_property_via_this_binding_propagates() {
    // Locks the `$this` binding: if `$this` is bound to the
    // source-set-impl's class (employee_set → Person), then
    // accessing `$this.notAField` must emit an UnknownProperty
    // error on Person — proving the lowering wires the binding,
    // not just builds a synthetic any-typed `$this`.
    let source = indoc! {r"
        ###Pure
        Class my::test::Firm
        {
          id : String[1];
        }

        Class my::test::Person
        {
          firmId : String[1];
        }

        Association my::test::Firm_Person
        {
          firm : my::test::Firm[1];
          employees : my::test::Person[*];
        }

        Class my::test::FirmSrc { id : String[1]; }
        Class my::test::PersonSrc { firmId : String[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[firm_set] : Pure
          {
            ~src my::test::FirmSrc
            id : $src.id
          }
          *my::test::Person[employee_set] : Pure
          {
            ~src my::test::PersonSrc
            firmId : $src.firmId
          }
          my::test::Firm_Person : XStore
          {
            firm[employee_set, firm_set] : $this.notAField == $that.id
          }
        )
    "};
    let file = parse("xstore_unknown_prop.pure", source);
    let (errors, _ext, _model) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("notAField")),
        "expected unknown-property error mentioning `notAField`; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
