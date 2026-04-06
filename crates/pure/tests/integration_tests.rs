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
use legend_pure_parser_pure::model::Element;
use legend_pure_parser_pure::pipeline::compile;
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

/// Helper: parse a `.pure` string into a `SourceFile`.
fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, "test.pure").expect("parse failed")
}

/// Helper: compile a single Pure source string.
fn compile_one(source: &str) -> Result<legend_pure_parser_pure::model::PureModel, Vec<legend_pure_parser_pure::error::CompilationError>> {
    let sf = parse(source);
    compile(&[sf])
}

// ---------------------------------------------------------------------------
// Class with Properties
// ---------------------------------------------------------------------------

#[test]
fn class_with_properties() {
    let model = compile_one(
        "Class Person { name: String[1]; age: Integer[0..1]; }"
    ).expect("should compile");

    // Find Person
    let person_id = model.resolve_by_path(&["Person".into()]).expect("Person should exist");
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
         Class C {}"
    ).expect("should compile");

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
    let model = compile_one(
        "Enum Color { RED, GREEN, BLUE }"
    ).expect("should compile");

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
    let model = compile_one(
        "function greet(name: String[1]): String[1] { 'hello' }"
    ).expect("should compile");

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
         }"
    ).expect("should compile");

    let person_id = model.resolve_by_path(&["Person".into()]).expect("Person");
    let firm_id = model.resolve_by_path(&["Firm".into()]).expect("Firm");
    let assoc_id = model.resolve_by_path(&["Person_Firm".into()]).expect("Person_Firm");

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
    let result = compile_one(
        "Class A extends B {}\nClass B extends A {}"
    );
    assert!(result.is_err(), "cyclic inheritance should fail");
    let errors = result.unwrap_err();

    let cycle_errors: Vec<_> = errors.iter()
        .filter(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::CyclicInheritance { .. }
        ))
        .collect();

    assert!(!cycle_errors.is_empty(), "should have CyclicInheritance error");

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
    let result = compile_one(
        "Class Bad { x: NonExistent[1]; }"
    );
    assert!(result.is_err(), "unresolved type should fail");
    let errors = result.unwrap_err();

    let unresolved: Vec<_> = errors.iter()
        .filter(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { .. }
        ))
        .collect();

    assert_eq!(unresolved.len(), 1, "should have exactly 1 UnresolvedElement error");

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
    assert_eq!(
        err.source_info.start_line, 1,
        "error should be on line 1"
    );
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
    let result = compile_one(
        "Class Foo {}\nClass Foo {}"
    );
    assert!(result.is_err(), "duplicate element should fail");
    let errors = result.unwrap_err();

    let dupes: Vec<_> = errors.iter()
        .filter(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { .. }
        ))
        .collect();

    assert_eq!(dupes.len(), 1, "should have exactly 1 DuplicateElement error");

    let err = &dupes[0];
    match &err.kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "Foo");
        }
        _ => unreachable!(),
    }

    // Duplicate is the second declaration (line 2)
    assert_eq!(err.source_info.source, "test.pure");
    assert_eq!(err.source_info.start_line, 2, "duplicate should be on line 2");
}

// ---------------------------------------------------------------------------
// Unresolved Profile in Annotation (Error Case)
// ---------------------------------------------------------------------------

#[test]
fn unresolved_profile_error() {
    let result = compile_one(
        "Class <<nonexistent::profile.stereo>> Annotated {}"
    );
    assert!(result.is_err(), "unresolved profile should fail");
    let errors = result.unwrap_err();

    let unresolved: Vec<_> = errors.iter()
        .filter(|e| matches!(
            &e.kind,
            legend_pure_parser_pure::error::CompilationErrorKind::UnresolvedElement { .. }
        ))
        .collect();

    assert!(!unresolved.is_empty(), "should have UnresolvedElement for the profile");

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
    let model = compile_one(
        "Profile doc { stereotypes: [deprecated]; tags: [description]; }"
    ).expect("should compile");

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
    let model = compile_one(
        "Class model::domain::Person { name: String[1]; }"
    ).expect("should compile");

    let person_id = model.resolve_by_path(
        &["model".into(), "domain".into(), "Person".into()]
    ).expect("model::domain::Person should exist");

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
