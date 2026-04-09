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

//! End-to-end integration tests: parse → compile → assert model state.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::compile;
use legend_pure_parser_pure::model::Element;
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

/// Helper: parse a `.pure` string into a `SourceFile`.
fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, "test.pure").expect("parse failed")
}

/// Helper: compile a single Pure source string.
#[allow(clippy::result_large_err)]
fn compile_one(
    source: &str,
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sf = parse(source);
    compile!(&[sf])
}

// ---------------------------------------------------------------------------
// Class with Properties
// ---------------------------------------------------------------------------

#[test]
fn class_with_properties() {
    let model = compile_one("Class Person { name: String[1]; age: Integer[0..1]; }")
        .expect("should compile");

    // Find Person
    let person_id = model
        .resolve_by_path(&["Person".into()])
        .expect("Person should exist");
    let element = model.get_element(person_id);

    match element {
        Element::Class(class) => {
            assert_eq!(class.properties.len(), 2);

            // name: String[1]
            let name_prop = &class.properties[0];
            assert_eq!(name_prop.name.as_str(), "name");
            assert_eq!(name_prop.multiplicity, Multiplicity::PureOne);
            match &name_prop.type_expr {
                TypeExpr::Named { element, .. } => {
                    assert_eq!(*element, bootstrap::STRING_ID);
                }
                _ => panic!("expected Named type for name property"),
            }

            // age: Integer[0..1]
            let age_prop = &class.properties[1];
            assert_eq!(age_prop.name.as_str(), "age");
            assert_eq!(age_prop.multiplicity, Multiplicity::ZeroOrOne);
            match &age_prop.type_expr {
                TypeExpr::Named { element, .. } => {
                    assert_eq!(*element, bootstrap::INTEGER_ID);
                }
                _ => panic!("expected Named type for age property"),
            }
        }
        _ => panic!("expected Class element"),
    }
}

// ---------------------------------------------------------------------------
// Inheritance Chain + Specialization Index
// ---------------------------------------------------------------------------

#[test]
fn inheritance_chain_and_specializations() {
    let model = compile_one(
        "Class A extends B {}\n\
         Class B extends C {}\n\
         Class C {}",
    )
    .expect("should compile");

    let a_id = model.resolve_by_path(&["A".into()]).expect("A");
    let b_id = model.resolve_by_path(&["B".into()]).expect("B");
    let c_id = model.resolve_by_path(&["C".into()]).expect("C");

    // A extends B
    match model.get_element(a_id) {
        Element::Class(class) => {
            assert_eq!(class.super_types.len(), 1);
            match &class.super_types[0] {
                TypeExpr::Named { element, .. } => assert_eq!(*element, b_id),
                _ => panic!("expected Named"),
            }
        }
        _ => panic!("expected Class"),
    }

    // Specialization index: C has specialization B
    let c_specs = model.specializations(c_id);
    assert_eq!(c_specs.len(), 1);
    assert_eq!(c_specs[0], b_id);

    // Specialization index: B has specialization A
    let b_specs = model.specializations(b_id);
    assert_eq!(b_specs.len(), 1);
    assert_eq!(b_specs[0], a_id);

    // C is a root — no specialization of its own in the model
    let a_specs = model.specializations(a_id);
    assert!(a_specs.is_empty());
}

// ---------------------------------------------------------------------------
// Enumeration with Values
// ---------------------------------------------------------------------------

#[test]
fn enumeration_with_values() {
    let model = compile_one("Enum Color { RED, GREEN, BLUE }").expect("should compile");

    let color_id = model.resolve_by_path(&["Color".into()]).expect("Color");
    match model.get_element(color_id) {
        Element::Enumeration(e) => {
            assert_eq!(e.values.len(), 3);
            assert_eq!(e.values[0].name.as_str(), "RED");
            assert_eq!(e.values[1].name.as_str(), "GREEN");
            assert_eq!(e.values[2].name.as_str(), "BLUE");
        }
        _ => panic!("expected Enumeration"),
    }
}

// ---------------------------------------------------------------------------
// Function with Parameters + Return Type
// ---------------------------------------------------------------------------

#[test]
fn function_with_params() {
    let model = compile_one("function greet(name: String[1]): String[1] { 'hello' }")
        .expect("should compile");

    let fn_id = model.resolve_by_path(&["greet".into()]).expect("greet");
    match model.get_element(fn_id) {
        Element::Function(f) => {
            // Parameters
            assert_eq!(f.parameters.len(), 1);
            assert_eq!(f.parameters[0].name.as_str(), "name");
            assert_eq!(f.parameters[0].multiplicity, Multiplicity::PureOne);
            match &f.parameters[0].type_expr {
                TypeExpr::Named { element, .. } => assert_eq!(*element, bootstrap::STRING_ID),
                _ => panic!("expected Named"),
            }

            // Return type
            assert_eq!(f.return_multiplicity, Multiplicity::PureOne);
            match &f.return_type {
                TypeExpr::Named { element, .. } => assert_eq!(*element, bootstrap::STRING_ID),
                _ => panic!("expected Named return type"),
            }
        }
        _ => panic!("expected Function"),
    }
}

// ---------------------------------------------------------------------------
// Association
// ---------------------------------------------------------------------------

#[test]
fn association_with_properties() {
    let model = compile_one(
        "Class Person {}\n\
         Class Firm {}\n\
         Association Person_Firm {\n\
           employee: Person[*];\n\
           employer: Firm[1];\n\
         }",
    )
    .expect("should compile");

    let person_id = model.resolve_by_path(&["Person".into()]).expect("Person");
    let firm_id = model.resolve_by_path(&["Firm".into()]).expect("Firm");
    let assoc_id = model
        .resolve_by_path(&["Person_Firm".into()])
        .expect("Person_Firm");

    match model.get_element(assoc_id) {
        Element::Association(assoc) => {
            assert_eq!(assoc.properties.len(), 2);

            // employee: Person[*]
            assert_eq!(assoc.properties[0].name.as_str(), "employee");
            assert_eq!(assoc.properties[0].multiplicity, Multiplicity::ZeroOrMany);
            match &assoc.properties[0].type_expr {
                TypeExpr::Named { element, .. } => assert_eq!(*element, person_id),
                _ => panic!("expected Named"),
            }

            // employer: Firm[1]
            assert_eq!(assoc.properties[1].name.as_str(), "employer");
            assert_eq!(assoc.properties[1].multiplicity, Multiplicity::PureOne);
            match &assoc.properties[1].type_expr {
                TypeExpr::Named { element, .. } => assert_eq!(*element, firm_id),
                _ => panic!("expected Named"),
            }
        }
        _ => panic!("expected Association"),
    }

    // Association property index: Person has association prop from Person_Firm
    let person_assoc_props = model.association_properties(person_id);
    assert_eq!(person_assoc_props.len(), 1);
    assert_eq!(person_assoc_props[0].0, assoc_id);

    // Firm also has one
    let firm_assoc_props = model.association_properties(firm_id);
    assert_eq!(firm_assoc_props.len(), 1);
    assert_eq!(firm_assoc_props[0].0, assoc_id);
}

// ---------------------------------------------------------------------------
// Cyclic Inheritance (Error Case)
// ---------------------------------------------------------------------------

#[test]
fn cyclic_inheritance_error() {
    let result = compile_one("Class A extends B {}\nClass B extends A {}");
    assert!(result.is_err(), "cyclic inheritance should fail");
    let errors = &result.unwrap_err().errors;

    let cycle_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                legend_pure_parser_pure::error::CompilationErrorKind::CyclicInheritance { .. }
            )
        })
        .collect();

    assert!(
        !cycle_errors.is_empty(),
        "should have CyclicInheritance error"
    );

    // Verify source info is present and points to the right file
    for err in &cycle_errors {
        assert_eq!(
            err.source_info.source, "test.pure",
            "error should reference the source file"
        );
        assert!(
            err.source_info.start_line >= 1,
            "error should have a valid line number, got {}",
            err.source_info.start_line
        );
        assert!(
            err.source_info.start_column >= 1,
            "error should have a valid column, got {}",
            err.source_info.start_column
        );
    }
}

// ---------------------------------------------------------------------------
// Unresolved Type (Error Case)
// ---------------------------------------------------------------------------

#[test]
fn unresolved_type_error() {
    let result = compile_one("Class Bad { x: NonExistent[1]; }");
    assert!(result.is_err(), "unresolved type should fail");
    let errors = &result.unwrap_err().errors;

    let unresolved: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { .. }
            )
        })
        .collect();

    assert_eq!(
        unresolved.len(),
        1,
        "should have exactly 1 UnresolvedElement error"
    );

    let err = &unresolved[0];

    // Verify the error kind carries the correct path
    match &err.kind {
        legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { path } => {
            assert_eq!(path.as_str(), "NonExistent");
        }
        _ => unreachable!(),
    }

    // Verify source info points to the type reference, not the element
    assert_eq!(err.source_info.source, "test.pure");
    assert_eq!(err.source_info.start_line, 1, "error should be on line 1");
    // Column should point to 'NonExistent' (col 16 in "Class Bad { x: NonExistent[1]; }")
    assert!(
        err.source_info.start_column > 1,
        "error column should point into the line, got {}",
        err.source_info.start_column
    );
}

// ---------------------------------------------------------------------------
// Duplicate Element (Error Case)
// ---------------------------------------------------------------------------

#[test]
fn duplicate_element_error() {
    let result = compile_one("Class Foo {}\nClass Foo {}");
    assert!(result.is_err(), "duplicate element should fail");
    let errors = &result.unwrap_err().errors;

    let dupes: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { .. }
            )
        })
        .collect();

    assert_eq!(
        dupes.len(),
        1,
        "should have exactly 1 DuplicateElement error"
    );

    let err = &dupes[0];
    match &err.kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "Foo");
        }
        _ => unreachable!(),
    }

    // Duplicate is the second declaration (line 2)
    assert_eq!(err.source_info.source, "test.pure");
    assert_eq!(
        err.source_info.start_line, 2,
        "duplicate should be on line 2"
    );
}

// ---------------------------------------------------------------------------
// Unresolved Profile in Annotation (Error Case)
// ---------------------------------------------------------------------------

#[test]
fn unresolved_profile_error() {
    let result = compile_one("Class <<nonexistent::profile.stereo>> Annotated {}");
    assert!(result.is_err(), "unresolved profile should fail");
    let errors = &result.unwrap_err().errors;

    let unresolved: Vec<_> = errors
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { .. }
            )
        })
        .collect();

    assert!(
        !unresolved.is_empty(),
        "should have UnresolvedElement for the profile"
    );

    // Verify source info is valid
    for err in &unresolved {
        assert_eq!(err.source_info.source, "test.pure");
        assert_eq!(err.source_info.start_line, 1);
    }
}

// ---------------------------------------------------------------------------
// Profile with Stereotypes and Tags
// ---------------------------------------------------------------------------

#[test]
fn profile_with_stereotypes_and_tags() {
    let model = compile_one("Profile doc { stereotypes: [deprecated]; tags: [description]; }")
        .expect("should compile");

    let profile_id = model.resolve_by_path(&["doc".into()]).expect("doc");
    match model.get_element(profile_id) {
        Element::Profile(p) => {
            assert_eq!(p.stereotypes.len(), 1);
            assert_eq!(p.stereotypes[0].as_str(), "deprecated");
            assert_eq!(p.tags.len(), 1);
            assert_eq!(p.tags[0].as_str(), "description");
        }
        _ => panic!("expected Profile"),
    }
}

// ---------------------------------------------------------------------------
// Class in Package
// ---------------------------------------------------------------------------

#[test]
fn class_in_package() {
    let model =
        compile_one("Class model::domain::Person { name: String[1]; }").expect("should compile");

    let person_id = model
        .resolve_by_path(&["model".into(), "domain".into(), "Person".into()])
        .expect("model::domain::Person should exist");

    let node = model.get_node(person_id);
    assert_eq!(node.name.as_str(), "Person");

    match model.get_element(person_id) {
        Element::Class(class) => {
            assert_eq!(class.properties.len(), 1);
            assert_eq!(class.properties[0].name.as_str(), "name");
        }
        _ => panic!("expected Class"),
    }
}

// ===========================================================================
// Phase 4: Validation Tests
// ===========================================================================

use legend_pure_parser_pure::error::CompilationErrorKind;

// ---------------------------------------------------------------------------
// Measure with Units (Positive)
// ---------------------------------------------------------------------------

#[test]
fn measure_with_units() {
    let model = compile_one(
        "Measure Distance {\n\
           *Meter: x -> $x;\n\
           Kilometer: x -> $x * 1000;\n\
           Mile: x -> $x * 1609.344;\n\
         }",
    )
    .expect("should compile");

    let measure_id = model
        .resolve_by_path(&["Distance".into()])
        .expect("Distance");
    match model.get_element(measure_id) {
        Element::Measure(m) => {
            assert!(m.canonical_unit.is_some(), "should have canonical unit");
            assert_eq!(
                m.non_canonical_units.len(),
                2,
                "should have 2 non-canonical units"
            );

            // Verify each unit references back to its parent measure
            let canon_id = m.canonical_unit.unwrap();
            match model.get_element(canon_id) {
                Element::Unit(u) => {
                    assert_eq!(
                        u.measure, measure_id,
                        "canonical unit should reference parent"
                    );
                }
                _ => panic!("expected Unit element for canonical"),
            }
            let node = model.get_node(canon_id);
            assert_eq!(node.name.as_str(), "Meter");

            for &non_canon_id in &m.non_canonical_units {
                match model.get_element(non_canon_id) {
                    Element::Unit(u) => {
                        assert_eq!(
                            u.measure, measure_id,
                            "non-canonical unit should reference parent"
                        );
                    }
                    _ => panic!("expected Unit element for non-canonical"),
                }
            }
        }
        _ => panic!("expected Measure"),
    }
}

// ---------------------------------------------------------------------------
// Annotation Validation: Stereotype Exists (Positive)
// ---------------------------------------------------------------------------

#[test]
fn valid_stereotype_compiles() {
    let model = compile_one(
        "Profile doc { stereotypes: [deprecated, internal]; tags: [description]; }\n\
         Class <<doc.deprecated>> {doc.description = 'A thing'} OldThing {}",
    )
    .expect("valid annotation should compile");

    let old_id = model
        .resolve_by_path(&["OldThing".into()])
        .expect("OldThing");
    match model.get_element(old_id) {
        Element::Class(class) => {
            assert_eq!(class.stereotypes.len(), 1, "should have 1 stereotype");
            assert_eq!(class.stereotypes[0].value.as_str(), "deprecated");
            assert_eq!(class.tagged_values.len(), 1, "should have 1 tagged value");
            assert_eq!(class.tagged_values[0].tag.as_str(), "description");
            assert_eq!(class.tagged_values[0].value, "A thing");
        }
        _ => panic!("expected Class"),
    }
    drop(model);
}

// ---------------------------------------------------------------------------
// Association Cardinality Error
// ---------------------------------------------------------------------------

#[test]
fn association_wrong_cardinality() {
    // 1 property instead of 2
    let result = compile_one(
        "Class A {}\n\
         Association Bad { a: A[1]; }",
    );
    assert!(result.is_err(), "association with 1 property should fail");
    let errors = &result.unwrap_err().errors;
    let assoc_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, CompilationErrorKind::InvalidAssociation { .. }))
        .collect();
    assert!(
        !assoc_errors.is_empty(),
        "should have InvalidAssociation error"
    );
}

// ---------------------------------------------------------------------------
// Association Property Not Class Error
// ---------------------------------------------------------------------------

#[test]
fn association_property_not_class() {
    let result = compile_one(
        "Class A {}\n\
         Enum Status { Active, Inactive }\n\
         Association Bad { a: A[1]; s: Status[1]; }",
    );
    assert!(
        result.is_err(),
        "association property pointing to Enum should fail"
    );
    let errors = &result.unwrap_err().errors;
    let assoc_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, CompilationErrorKind::InvalidAssociation { .. }))
        .collect();
    assert!(
        !assoc_errors.is_empty(),
        "should have InvalidAssociation for non-Class property"
    );
}

// ---------------------------------------------------------------------------
// Super-type Not a Class Error
// ---------------------------------------------------------------------------

#[test]
fn super_type_not_class() {
    let result = compile_one(
        "Enum Color { RED, GREEN }\n\
         Class Bad extends Color {}",
    );
    assert!(result.is_err(), "extending an Enum should fail");
    let errors = &result.unwrap_err().errors;
    let super_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, CompilationErrorKind::InvalidSuperType { .. }))
        .collect();
    assert!(
        !super_errors.is_empty(),
        "should have InvalidSuperType error"
    );

    // Verify error carries class and super names
    match &super_errors[0].kind {
        CompilationErrorKind::InvalidSuperType {
            class_name,
            super_name,
        } => {
            assert_eq!(class_name.as_str(), "Bad");
            assert_eq!(super_name.as_str(), "Color");
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Stereotype Not in Profile Error
// ---------------------------------------------------------------------------

#[test]
fn stereotype_not_in_profile() {
    let result = compile_one(
        "Profile doc { stereotypes: [deprecated]; tags: [desc]; }\n\
         Class <<doc.nonexistent>> Bad {}",
    );
    assert!(
        result.is_err(),
        "referencing nonexistent stereotype should fail"
    );
    let errors = &result.unwrap_err().errors;
    let anno_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, CompilationErrorKind::InvalidAnnotation { .. }))
        .collect();
    assert!(
        !anno_errors.is_empty(),
        "should have InvalidAnnotation error"
    );
    assert!(
        errors[0].message.contains("nonexistent"),
        "error message should mention the missing stereotype name"
    );
}

// ---------------------------------------------------------------------------
// Tag Not in Profile Error
// ---------------------------------------------------------------------------

#[test]
fn tag_not_in_profile() {
    let result = compile_one(
        "Profile doc { stereotypes: [deprecated]; tags: [description]; }\n\
         Class {doc.missingTag = 'val'} Bad {}",
    );
    assert!(result.is_err(), "referencing nonexistent tag should fail");
    let errors = &result.unwrap_err().errors;
    let anno_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, CompilationErrorKind::InvalidAnnotation { .. }))
        .collect();
    assert!(
        !anno_errors.is_empty(),
        "should have InvalidAnnotation error"
    );
    assert!(
        errors[0].message.contains("missingTag"),
        "error message should mention the missing tag name"
    );
}

// ---------------------------------------------------------------------------
// Duplicate Property Names Error
// ---------------------------------------------------------------------------

#[test]
fn duplicate_property_names() {
    let result = compile_one("Class Bad { name: String[1]; name: Integer[1]; }");
    assert!(result.is_err(), "duplicate properties should fail");
    let errors = &result.unwrap_err().errors;
    let dupe_errors: Vec<_> = errors
        .iter()
        .filter(|e| matches!(&e.kind, CompilationErrorKind::DuplicateProperty { .. }))
        .collect();
    assert_eq!(
        dupe_errors.len(),
        1,
        "should have exactly 1 DuplicateProperty error"
    );

    match &dupe_errors[0].kind {
        CompilationErrorKind::DuplicateProperty {
            class_name,
            property_name,
        } => {
            assert_eq!(class_name.as_str(), "Bad");
            assert_eq!(property_name.as_str(), "name");
        }
        _ => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// Import Resolution
// ---------------------------------------------------------------------------

/// Helper: compile multiple Pure source strings together with auto-imports.
#[allow(clippy::result_large_err)]
fn compile_with_imports(
    sources: &[&str],
    auto_imports: &[&str],
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sfs: Vec<SourceFile> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            legend_pure_parser_parser::parse(s, &format!("test{i}.pure")).expect("parse failed")
        })
        .collect();
    let imports: Vec<smol_str::SmolStr> = auto_imports.iter().map(smol_str::SmolStr::new).collect();
    legend_pure_parser_pure::pipeline::compile(&sfs, &imports)
}

#[test]
fn compile_with_import_resolves_unqualified() {
    // Section with explicit import — `Instrument` resolves without FQN
    let source = r"
###Pure
import model::trading::*;

Class model::trading::Instrument {
  ticker: String[1];
}

Class model::trading::Trade {
  instrument: Instrument[1];
}
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let trade_id = model
        .resolve_by_path(&["model".into(), "trading".into(), "Trade".into()])
        .expect("Trade should exist");
    let instr_id = model
        .resolve_by_path(&["model".into(), "trading".into(), "Instrument".into()])
        .expect("Instrument should exist");

    match model.get_element(trade_id) {
        Element::Class(c) => {
            assert_eq!(c.properties.len(), 1);
            match &c.properties[0].type_expr {
                TypeExpr::Named { element, .. } => assert_eq!(*element, instr_id),
                _ => panic!("expected Named type"),
            }
        }
        _ => panic!("Trade should be a Class"),
    }
}

#[test]
fn compile_import_isolation_across_sections() {
    // Import in section 1 does NOT apply to section 2
    let source = r"
###Pure
import model::domain::*;

Class model::domain::Address {
  city: String[1];
}

###Pure

Class model::domain::Person {
  home: Address[1];
}
";
    // Section 2 has no import — `Address` is unqualified and should fail
    let result = compile_with_imports(&[source], &[]);
    assert!(
        result.is_err(),
        "should fail because section 2 has no import for Address"
    );
    let errors = &result.unwrap_err().errors;
    assert!(errors.iter().any(|e| e.message.contains("Address")));
}

#[test]
fn compile_unresolved_with_wrong_import() {
    let source = r"
###Pure
import wrong::pkg::*;

Class model::domain::Person {
  name: SomeType[1];
}
";
    let result = compile_with_imports(&[source], &[]);
    assert!(result.is_err());
    let errors = &result.unwrap_err().errors;
    assert!(errors.iter().any(|e| e.message.contains("SomeType")));
}

#[test]
fn compile_ambiguous_import() {
    // Two imports both provide `Foo` — should produce AmbiguousImport error
    let source = r"
###Pure
import pkg_a::*;
import pkg_b::*;

Class pkg_a::Foo {
  x: String[1];
}

Class pkg_b::Foo {
  y: String[1];
}

Class test::Bar {
  f: Foo[1];
}
";
    let result = compile_with_imports(&[source], &[]);
    assert!(result.is_err(), "should fail with ambiguous import");
    let errors = &result.unwrap_err().errors;
    let ambig = errors
        .iter()
        .find(|e| matches!(&e.kind, CompilationErrorKind::AmbiguousImport { .. }));
    assert!(
        ambig.is_some(),
        "should have AmbiguousImport error, got: {errors:?}"
    );
}

#[test]
fn compile_import_for_annotations() {
    // Import resolves profile references in stereotypes
    let source = r"
###Pure
import model::meta::*;

Profile model::meta::doc {
  stereotypes: [deprecated];
  tags: [description];
}

Class <<doc.deprecated>> {model::meta::doc.description = 'old'} model::domain::LegacyThing {
  name: String[1];
}
";
    let model = compile_with_imports(&[source], &[]).expect("should compile with imported profile");
    let legacy_id = model
        .resolve_by_path(&["model".into(), "domain".into(), "LegacyThing".into()])
        .expect("LegacyThing should exist");
    match model.get_element(legacy_id) {
        Element::Class(c) => {
            assert_eq!(c.stereotypes.len(), 1);
            assert_eq!(c.stereotypes[0].value.as_str(), "deprecated");
            assert_eq!(c.tagged_values.len(), 1);
            assert_eq!(c.tagged_values[0].tag.as_str(), "description");
        }
        _ => panic!("LegacyThing should be a Class"),
    }
}

// ---------------------------------------------------------------------------
// Expression Lowering — Phase 1
// ---------------------------------------------------------------------------

use legend_pure_parser_pure::types::{DateValue, Expression};

#[test]
fn function_body_integer_literal() {
    let source = "function test::intFunc(): Integer[1] { 42 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "intFunc".into()])
        .expect("intFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::IntegerLiteral(v, _) => assert_eq!(*v, 42),
                other => panic!("expected IntegerLiteral, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn function_body_string_literal() {
    let source = "function test::strFunc(): String[1] { 'hello' }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "strFunc".into()])
        .expect("strFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::StringLiteral(v, _) => assert_eq!(v.as_str(), "hello"),
                other => panic!("expected StringLiteral, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn function_body_boolean_literal() {
    let source = "function test::boolFunc(): Boolean[1] { true }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "boolFunc".into()])
        .expect("boolFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::BooleanLiteral(v, _) => assert!(*v),
                other => panic!("expected BooleanLiteral, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn function_body_float_literal() {
    let source = "function test::floatFunc(): Float[1] { 1.5 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "floatFunc".into()])
        .expect("floatFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::FloatLiteral(v, _) => {
                    assert!((v - 1.5).abs() < f64::EPSILON);
                }
                other => panic!("expected FloatLiteral, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn function_body_variable_ref() {
    let source = "function test::varFunc(x: String[1]): String[1] { $x }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "varFunc".into()])
        .expect("varFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::Variable { name, .. } => assert_eq!(name.as_str(), "x"),
                other => panic!("expected Variable, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn function_body_collection() {
    let source = "function test::collFunc(): Integer[*] { [1, 2, 3] }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "collFunc".into()])
        .expect("collFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::Collection { elements, .. } => {
                    assert_eq!(elements.len(), 3);
                    assert!(matches!(&elements[0], Expression::IntegerLiteral(1, _)));
                    assert!(matches!(&elements[1], Expression::IntegerLiteral(2, _)));
                    assert!(matches!(&elements[2], Expression::IntegerLiteral(3, _)));
                }
                other => panic!("expected Collection, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn function_body_date_literal() {
    let source = "function test::dateFunc(): StrictDate[1] { %2024-01-15 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "dateFunc".into()])
        .expect("dateFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::DateLiteral(dv, _) => {
                    assert_eq!(
                        *dv,
                        DateValue::StrictDate {
                            year: 2024,
                            month: 1,
                            day: 15
                        }
                    );
                }
                other => panic!("expected DateLiteral, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn constraint_expression_compiled() {
    let source = "Class test::Foo [ c: true ] { name: String[1]; }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "Foo".into()])
        .expect("Foo should exist");
    match model.get_element(id) {
        Element::Class(c) => {
            assert_eq!(c.constraints.len(), 1);
            assert!(matches!(
                &c.constraints[0].function,
                Expression::BooleanLiteral(true, _)
            ));
        }
        _ => panic!("expected Class"),
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Expression Lowering — Operators
// ---------------------------------------------------------------------------

#[test]
fn expression_arithmetic_desugars_to_function_call() {
    let source = "function test::f(): Integer[1] { 1 + 2 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1, "body should have 1 expression");
            match &f.body[0] {
                Expression::FunctionCall {
                    function,
                    function_name,
                    arguments,
                    ..
                } => {
                    assert!(function.is_none(), "built-in operator has no element ID");
                    assert_eq!(function_name.as_str(), "plus");
                    assert_eq!(arguments.len(), 2);
                    assert!(matches!(&arguments[0], Expression::IntegerLiteral(1, _)));
                    assert!(matches!(&arguments[1], Expression::IntegerLiteral(2, _)));
                }
                other => panic!("expected FunctionCall, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn expression_not_equal_desugars_to_not_equal() {
    let source = "function test::f(): Boolean[1] { 1 != 2 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            // != desugars to not(equal(1, 2))
            match &f.body[0] {
                Expression::FunctionCall {
                    function_name,
                    arguments,
                    ..
                } => {
                    assert_eq!(function_name.as_str(), "not");
                    assert_eq!(arguments.len(), 1);
                    match &arguments[0] {
                        Expression::FunctionCall {
                            function_name: inner_name,
                            arguments: inner_args,
                            ..
                        } => {
                            assert_eq!(inner_name.as_str(), "equal");
                            assert_eq!(inner_args.len(), 2);
                        }
                        other => panic!("expected inner equal(), got {other:?}"),
                    }
                }
                other => panic!("expected not(), got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn expression_comparison_operators() {
    // Test all comparison operators desugar correctly
    for (op, expected_name) in &[
        ("<", "lessThan"),
        ("<=", "lessThanEqual"),
        (">", "greaterThan"),
        (">=", "greaterThanEqual"),
        ("==", "equal"),
    ] {
        let source = format!("function test::f(): Boolean[1] {{ 1 {op} 2 }}");
        let model = compile_one(&source).expect("should compile");
        let id = model
            .resolve_by_path(&["test".into(), "f".into()])
            .expect("f should exist");
        match model.get_element(id) {
            Element::Function(f) => {
                assert_eq!(f.body.len(), 1, "body should have 1 expression for {op}");
                match &f.body[0] {
                    Expression::FunctionCall { function_name, .. } => {
                        assert_eq!(
                            function_name.as_str(),
                            *expected_name,
                            "operator {op} should map to {expected_name}"
                        );
                    }
                    other => panic!("expected FunctionCall for {op}, got {other:?}"),
                }
            }
            _ => panic!("expected Function"),
        }
    }
}

#[test]
fn expression_logical_operators() {
    let source = "function test::f(): Boolean[1] { true && false }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::FunctionCall {
                    function_name,
                    arguments,
                    ..
                } => {
                    assert_eq!(function_name.as_str(), "and");
                    assert_eq!(arguments.len(), 2);
                    assert!(matches!(&arguments[0], Expression::BooleanLiteral(true, _)));
                    assert!(matches!(
                        &arguments[1],
                        Expression::BooleanLiteral(false, _)
                    ));
                }
                other => panic!("expected FunctionCall, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn expression_unary_not() {
    let source = "function test::f(): Boolean[1] { !true }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::FunctionCall {
                    function_name,
                    arguments,
                    ..
                } => {
                    assert_eq!(function_name.as_str(), "not");
                    assert_eq!(arguments.len(), 1);
                    assert!(matches!(&arguments[0], Expression::BooleanLiteral(true, _)));
                }
                other => panic!("expected FunctionCall, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Expression Lowering — Member Access
// ---------------------------------------------------------------------------

#[test]
fn expression_property_access() {
    let source = r"
        Class test::Person { name: String[1]; }
        function test::f(p: test::Person[1]): String[1] { $p.name }
    ";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::PropertyAccess {
                    target, property, ..
                } => {
                    assert_eq!(property.as_str(), "name");
                    assert!(matches!(
                        target.as_ref(),
                        Expression::Variable { name, .. } if name == "p"
                    ));
                }
                other => panic!("expected PropertyAccess, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

// ---------------------------------------------------------------------------
// Phase 2: Expression Lowering — Function Application
// ---------------------------------------------------------------------------

#[test]
fn expression_function_call_resolved() {
    let source = r"
        function test::helper(): String[1] { 'hi' }
        function test::f(): String[1] { test::helper() }
    ";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &f.body[0] {
                Expression::FunctionCall {
                    function,
                    function_name,
                    arguments,
                    ..
                } => {
                    assert!(
                        function.is_some(),
                        "resolved function should have element ID"
                    );
                    assert_eq!(function_name.as_str(), "helper");
                    assert!(arguments.is_empty());
                }
                other => panic!("expected FunctionCall, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}
