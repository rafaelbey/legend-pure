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

    let fn_id = model
        .resolve_function_by_path(&["greet".into()])
        .expect("greet");
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

// Island-expression lowering rejection (the `#{…}#` grammar) lives in
// `crates/dsl-graph/tests/compiler_smoke.rs`. The pure crate carries
// no graph-fetch references.

// ---------------------------------------------------------------------------
// Invalid Decimal Literal (Error Case)
// ---------------------------------------------------------------------------

#[test]
fn invalid_decimal_literal() {
    let result = compile_one("function x(): Any[1] { 999999999999999999999999999999999999999n }");
    assert!(
        result.is_err(),
        "should result in some error due to skipped node"
    );
}

// ---------------------------------------------------------------------------
// Unresolved Type (Error Case)
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
// Duplicate Element — every element kind must produce DuplicateElement
// ---------------------------------------------------------------------------

/// Helper: compile `source`, expect failure, and return the
/// `DuplicateElement` errors filtered out of the result.
#[allow(clippy::result_large_err)]
fn duplicate_errors(source: &str) -> Vec<legend_pure_parser_pure::error::CompilationError> {
    let result = compile_one(source);
    assert!(result.is_err(), "duplicate element should fail");
    result
        .unwrap_err()
        .errors
        .into_iter()
        .filter(|e| {
            matches!(
                &e.kind,
                legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { .. }
            )
        })
        .collect()
}

/// Two functions with identical name + identical signature in the same
/// package are TRUE duplicates: their mangled FQNs collide because the
/// signature is fully encoded in the mangling. Distinct overloads have
/// distinct mangled names and never reach this check.
#[test]
fn duplicate_function_with_identical_signature() {
    let dupes = duplicate_errors(
        "function foo::bar(): Boolean[1] { true; }\n\
         function foo::bar(): Boolean[1] { false; }",
    );
    assert_eq!(dupes.len(), 1, "expected exactly 1 DuplicateElement");

    let err = &dupes[0];
    match &err.kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(
                name.as_str(),
                "foo::bar__Boolean_1_",
                "duplicate FQN should be the mangled function name"
            );
        }
        _ => unreachable!(),
    }
    assert_eq!(err.source_info.start_line, 2, "duplicate is the second def");
}

/// Real-world repro of the sqrt.pure / asin.pure copy-paste bug: two
/// functions identical down to type parameters, differing only in body.
#[test]
fn duplicate_function_with_type_parameters() {
    let dupes = duplicate_errors(
        "function <<PCT.test>> meta::demo::tst<Z|y>(f:Function<{->Z[y]}>[1]):Boolean[1] {\n\
            true;\n\
         }\n\
         function <<PCT.test>> meta::demo::tst<Z|y>(f:Function<{->Z[y]}>[1]):Boolean[1] {\n\
            false;\n\
         }",
    );
    assert_eq!(dupes.len(), 1, "expected exactly 1 DuplicateElement");
    // Second declaration starts at line 4 (line 1 = first def, line 2-3 = body+brace,
    // line 4 = second `function ...` head).
    assert_eq!(dupes[0].source_info.start_line, 4);
}

/// Negative control — distinct signatures must NOT be flagged as
/// duplicates. Function overloading is a load-bearing Pure feature.
#[test]
fn function_overloads_with_distinct_signatures_are_allowed() {
    let model = compile_one(
        "function foo::bar(x:Integer[1]):Boolean[1] { true; }\n\
         function foo::bar(x:String[1]):Boolean[1] { true; }",
    )
    .expect("distinct overloads must compile");

    // Both mangled FQNs must register as separate elements.
    let int_overload = model
        .resolve_by_path(&["foo".into(), "bar_Integer_1__Boolean_1_".into()])
        .expect("Integer overload");
    let str_overload = model
        .resolve_by_path(&["foo".into(), "bar_String_1__Boolean_1_".into()])
        .expect("String overload");
    assert_ne!(int_overload, str_overload);
}

/// Native functions also mangle via `f.mangled_name()`, so the same
/// duplicate-detection path applies.
#[test]
fn duplicate_native_function() {
    let dupes = duplicate_errors(
        "native function foo::bar(): Boolean[1];\n\
         native function foo::bar(): Boolean[1];",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::bar__Boolean_1_");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 2);
}

#[test]
fn duplicate_enumeration() {
    let dupes = duplicate_errors(
        "Enum foo::Color { RED, GREEN }\n\
         Enum foo::Color { BLUE }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::Color");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 2);
}

#[test]
fn duplicate_association() {
    let dupes = duplicate_errors(
        "Class A {}\n\
         Class B {}\n\
         Association foo::Link { left: A[1]; right: B[1]; }\n\
         Association foo::Link { left: A[1]; right: B[1]; }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::Link");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 4);
}

#[test]
fn duplicate_profile() {
    let dupes = duplicate_errors(
        "Profile foo::P { stereotypes: [s]; tags: [t]; }\n\
         Profile foo::P { stereotypes: [s]; tags: [t]; }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::P");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 2);
}

#[test]
fn duplicate_measure() {
    let dupes = duplicate_errors(
        "Measure foo::Distance {\n\
            *Meter: x -> $x;\n\
         }\n\
         Measure foo::Distance {\n\
            *Foot: x -> $x;\n\
         }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::Distance");
        }
        _ => unreachable!(),
    }
    // First measure: lines 1-3. Second measure starts at line 4.
    assert_eq!(dupes[0].source_info.start_line, 4);
}

#[test]
fn duplicate_class_then_enum() {
    let dupes = duplicate_errors(
        "Class foo::X {}\n\
         Enum foo::X { A }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::X");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 2);
}

#[test]
fn duplicate_class_then_association() {
    let dupes = duplicate_errors(
        "Class foo::X {}\n\
         Class A {}\n\
         Class B {}\n\
         Association foo::X { left: A[1]; right: B[1]; }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::X");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 4);
}

#[test]
fn duplicate_class_then_profile() {
    let dupes = duplicate_errors(
        "Class foo::X {}\n\
         Profile foo::X { stereotypes: [s]; tags: [t]; }",
    );
    assert_eq!(dupes.len(), 1);
    match &dupes[0].kind {
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateElement { name } => {
            assert_eq!(name.as_str(), "foo::X");
        }
        _ => unreachable!(),
    }
    assert_eq!(dupes[0].source_info.start_line, 2);
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
            assert_eq!(node.name.as_str(), "Distance~Meter");

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
    // Import in section 1 does NOT apply to section 2.
    // BUT both sections define elements in `model::domain`, so `Address`
    // is visible in section 2 via same-package resolution (not imports).
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
    // Section 2 has no import, but `Person` and `Address` share the
    // `model::domain` package — same-package resolution succeeds.
    let result = compile_with_imports(&[source], &[]);
    assert!(
        result.is_ok(),
        "should succeed: same-package types are visible without imports"
    );
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

use legend_pure_parser_pure::types::{DateValue, ExprKind, FunctionCallData};

#[test]
fn function_body_integer_literal() {
    let source = "function test::intFunc(): Integer[1] { 42 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_function_by_path(&["test".into(), "intFunc".into()])
        .expect("intFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::IntegerLiteral(v) => assert_eq!(*v, 42),
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
        .resolve_function_by_path(&["test".into(), "strFunc".into()])
        .expect("strFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::StringLiteral(v) => assert_eq!(v.as_str(), "hello"),
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
        .resolve_function_by_path(&["test".into(), "boolFunc".into()])
        .expect("boolFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::BooleanLiteral(v) => assert!(*v),
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
        .resolve_function_by_path(&["test".into(), "floatFunc".into()])
        .expect("floatFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::FloatLiteral(v) => {
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
        .resolve_function_by_path(&["test".into(), "varFunc".into()])
        .expect("varFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::Variable { name } => assert_eq!(name.as_str(), "x"),
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
        .resolve_function_by_path(&["test".into(), "collFunc".into()])
        .expect("collFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::Collection { elements } => {
                    assert_eq!(elements.len(), 3);
                    assert!(matches!(&*elements[0].kind, ExprKind::IntegerLiteral(1)));
                    assert!(matches!(&*elements[1].kind, ExprKind::IntegerLiteral(2)));
                    assert!(matches!(&*elements[2].kind, ExprKind::IntegerLiteral(3)));
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
        .resolve_function_by_path(&["test".into(), "dateFunc".into()])
        .expect("dateFunc should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::DateLiteral(dv) => {
                    assert_eq!(
                        *dv,
                        DateValue::StrictDate {
                            year: 2024,
                            month: Some(1),
                            day: Some(15)
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
                &*c.constraints[0].function.kind,
                ExprKind::BooleanLiteral(true)
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
    // Declare a minimal `plus` so the resolver can pick an overload.
    // Operator lowering routes through `resolve_function_call` (same as
    // user `plus(x, y)` calls), so the test must provide a plus to
    // resolve to. Production tests run against the full platform where
    // every overload exists.
    let source = "\
        native function plus(ints: Integer[*]): Integer[1];\n\
        function test::f(): Integer[1] { 1 + 2 }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1, "body should have 1 expression");
            // `1 + 2` desugars to `plus([1, 2])` — a single-arg call
            // wrapping a Collection of the two operands. Matches the
            // Pure-level signature `plus(Number[*]):Number[1]` and
            // Java Pure's single-param dispatch.
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function,
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert!(
                        function.is_some(),
                        "operator should resolve to a model element via resolve_function_call"
                    );
                    assert_eq!(function_name.as_str(), "plus");
                    assert_eq!(arguments.len(), 1, "plus takes a single collection arg");
                    match &*arguments[0].kind {
                        ExprKind::Collection { elements } => {
                            assert_eq!(elements.len(), 2);
                            assert!(matches!(&*elements[0].kind, ExprKind::IntegerLiteral(1)));
                            assert!(matches!(&*elements[1].kind, ExprKind::IntegerLiteral(2)));
                        }
                        other => panic!("expected Collection arg, got {other:?}"),
                    }
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
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            // != desugars to not(equal(1, 2))
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "not");
                    assert_eq!(arguments.len(), 1);
                    match &*arguments[0].kind {
                        ExprKind::FunctionCall(FunctionCallData {
                            function_name: inner_name,
                            arguments: inner_args,
                            ..
                        }) => {
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
            .resolve_function_by_path(&["test".into(), "f".into()])
            .expect("f should exist");
        match model.get_element(id) {
            Element::Function(f) => {
                assert_eq!(f.body.len(), 1, "body should have 1 expression for {op}");
                match &*f.body[0].kind {
                    ExprKind::FunctionCall(FunctionCallData { function_name, .. }) => {
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
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "and");
                    assert_eq!(arguments.len(), 2);
                    assert!(matches!(
                        &*arguments[0].kind,
                        ExprKind::BooleanLiteral(true)
                    ));
                    assert!(matches!(
                        &*arguments[1].kind,
                        ExprKind::BooleanLiteral(false)
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
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "not");
                    assert_eq!(arguments.len(), 1);
                    assert!(matches!(
                        &*arguments[0].kind,
                        ExprKind::BooleanLiteral(true)
                    ));
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
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::PropertyCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "name");
                    assert_eq!(arguments.len(), 1, "Property kind carries one receiver arg");
                    assert!(matches!(
                        &*arguments[0].kind,
                        ExprKind::Variable { name, .. } if name == "p"
                    ));
                }
                other => panic!("expected FunctionCall(kind: Property), got {other:?}"),
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
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function,
                    function_name,
                    arguments,
                    ..
                }) => {
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

// ---------------------------------------------------------------------------
// Phase 3: Expression Lowering — Lambda, Let, NewInstance
// ---------------------------------------------------------------------------

#[test]
fn expression_lambda_lowering() {
    // Stub `filter` — will be removed once the standard library is loaded.
    let source = r"
        native function filter(col: Any[*], fn: Any[1]): Any[*];
        Class test::Person { name: String[1]; age: Integer[1]; }
        function test::f(people: test::Person[*]): test::Person[*] {
            $people->filter({p: test::Person[1] | $p.age > 18})
        }
    ";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            // body is: $people->filter(lambda)
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "filter");
                    assert_eq!(arguments.len(), 2, "filter takes target + lambda");
                    // Second arg should be the lambda
                    match &*arguments[1].kind {
                        ExprKind::Lambda {
                            parameters, body, ..
                        } => {
                            assert_eq!(parameters.len(), 1);
                            assert_eq!(parameters[0].name.as_str(), "p");
                            assert!(!body.is_empty());
                        }
                        other => panic!("expected Lambda, got {other:?}"),
                    }
                }
                other => panic!("expected FunctionCall, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn expression_let_desugars_to_let_function() {
    let source = "function test::f(): Integer[1] { let x = 42; $x; }";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert!(f.body.len() >= 2, "body should have let + reference");
            // First expression should be letFunction
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "letFunction");
                    assert_eq!(arguments.len(), 2);
                    // First arg is the variable name as a string literal
                    assert!(matches!(
                        &*arguments[0].kind,
                        ExprKind::StringLiteral(name) if name == "x"
                    ));
                    // Second arg is the value
                    assert!(matches!(&*arguments[1].kind, ExprKind::IntegerLiteral(42)));
                }
                other => panic!("expected letFunction, got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn expression_new_instance_desugars_to_new() {
    let source = r"
        Class test::Pair { first: String[1]; second: Integer[1]; }
        function test::f(): test::Pair[1] { ^test::Pair(first='hello', second=42) }
    ";
    let model = compile_one(source).expect("should compile");
    let id = model
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    match model.get_element(id) {
        Element::Function(f) => {
            assert_eq!(f.body.len(), 1);
            match &*f.body[0].kind {
                ExprKind::FunctionCall(FunctionCallData {
                    function_name,
                    arguments,
                    ..
                }) => {
                    assert_eq!(function_name.as_str(), "new");
                    // class_ref, class_name_string, [type_args],
                    // [type_var_values], (key1, val1, augmented1),
                    // (key2, val2, augmented2) — positions 2 and 3 are
                    // the parsed `<T1, T2>` and `(10, 'ok')` collections
                    // (both empty here); positions 4.. carry triples
                    // matching the runtime's `apply_key_value_triples`
                    // input shape (the augmented bool distinguishes `=`
                    // from `+=`, both `false` here).
                    assert_eq!(arguments.len(), 10);
                    // First arg is the class element ref
                    assert!(matches!(
                        &*arguments[0].kind,
                        ExprKind::PackageableElementRef { .. }
                    ));
                    // Second arg is the class name string
                    assert!(matches!(
                        &*arguments[1].kind,
                        ExprKind::StringLiteral(name) if name == "Pair"
                    ));
                    // Third arg is the (empty) type-argument collection
                    assert!(matches!(
                        &*arguments[2].kind,
                        ExprKind::Collection { elements } if elements.is_empty()
                    ));
                    // Fourth arg is the (empty) type-variable-values collection
                    assert!(matches!(
                        &*arguments[3].kind,
                        ExprKind::Collection { elements } if elements.is_empty()
                    ));
                    // Fifth arg is first key name
                    assert!(matches!(
                        &*arguments[4].kind,
                        ExprKind::StringLiteral(name) if name == "first"
                    ));
                    // Sixth arg is first value
                    assert!(matches!(
                        &*arguments[5].kind,
                        ExprKind::StringLiteral(val) if val == "hello"
                    ));
                    // Seventh arg is the augmented flag for the first pair
                    assert!(matches!(
                        &*arguments[6].kind,
                        ExprKind::BooleanLiteral(false)
                    ));
                    // Eighth arg is second key name
                    assert!(matches!(
                        &*arguments[7].kind,
                        ExprKind::StringLiteral(name) if name == "second"
                    ));
                    // Ninth arg is second value
                    assert!(matches!(&*arguments[8].kind, ExprKind::IntegerLiteral(42)));
                    // Tenth arg is the augmented flag for the second pair
                    assert!(matches!(
                        &*arguments[9].kind,
                        ExprKind::BooleanLiteral(false)
                    ));
                }
                other => panic!("expected new(), got {other:?}"),
            }
        }
        _ => panic!("expected Function"),
    }
}

#[test]
fn compile_multiple_let_same_variable() {
    let source = "function test::f(): Integer[1] { let x = 42; let x = 43; $x; }";
    let Err(partial) = compile_one(source) else {
        panic!("Should fail to compile shadowed variables");
    };
    assert_eq!(partial.errors.len(), 1);
    assert!(matches!(
        partial.errors[0].kind,
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateVariable { .. }
    ));
    assert_eq!(partial.errors[0].message, "'x' has already been defined!");
}

#[test]

fn compile_lambda_let_shadows_outer_let() {
    // Operator lowering needs `plus` in scope (variadic_op routes through
    // resolve_function_call); declare both natives the source uses.
    //
    // `map`'s second param is declared `Any[1]` — a deliberately-loose
    // synthetic signature for this test. That doesn't carry a
    // `Function<{T→V}>` shape for the orchestrator to flow expectations
    // into the lambda's `y`, so `y` would land at the type-hole guard
    // (`CannotInferLambdaParameterTypes`). Annotate `y` explicitly:
    // the test's purpose is verifying lambda-`let` shadowing of outer
    // `let x`, which the annotation doesn't change.
    let source = r"
        native function map(col: Any[*], fn: Any[1]): Any[*];
        native function plus(ints: Integer[*]): Integer[1];
        function test::f(): Integer[*] { let x = 42; [1, 2]->map(y: Integer[1] | let x = 43; $x + $y); }
    ";
    let Ok(_) = compile_one(source) else {
        let partial = compile_one(source).unwrap_err();
        panic!(
            "Lambda let should be allowed to shadow outer block let. Errors: {:#?}",
            partial.errors
        );
    };
}

// ---------------------------------------------------------------------------
// Function dispatch phases — targeted tests per phase
// ---------------------------------------------------------------------------

/// Resolve the single `FunctionCall` in `caller_fqn`'s body and return the
/// mangled name of the resolved callee (i.e. which overload dispatch picked).
fn dispatch_target(
    model: &legend_pure_parser_pure::model::PureModel,
    caller_fqn: &[&str],
) -> String {
    let path: Vec<smol_str::SmolStr> = caller_fqn.iter().map(smol_str::SmolStr::new).collect();
    let caller_id = model
        .resolve_by_path(&path)
        .unwrap_or_else(|| panic!("caller {caller_fqn:?} should exist"));
    let Element::Function(f) = model.get_element(caller_id) else {
        panic!("caller {caller_fqn:?} should be a Function");
    };
    // Walk the body looking for the first FunctionCall with a resolved target.
    // Skip `letFunction` calls, which are the implicit `let x = …` desugaring.
    for stmt in f.body.iter() {
        if let legend_pure_parser_pure::types::ExprKind::FunctionCall(FunctionCallData {
            function: Some(fid),
            function_name,
            ..
        }) = stmt.kind.as_ref()
        {
            if function_name.as_str() == "letFunction" {
                continue;
            }
            return model.get_node(*fid).name.to_string();
        }
    }
    panic!("caller body has no resolved FunctionCall");
}

#[test]
fn dispatch_phase1_incompatible_type_eliminated() {
    // f(String) vs f(Integer); call with Integer arg → only f(Integer) compatible.
    let source = r"
###Pure
function test::f(x: String[1]): Boolean[1] { true; }
function test::f(x: Integer[1]): Boolean[1] { false; }
function test::caller(): Boolean[1] { f(42); }
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let target = dispatch_target(&model, &["test", "caller__Boolean_1_"]);
    assert!(
        target.starts_with("f_Integer_1_"),
        "phase 1 should pick Integer overload, got {target}"
    );
}

#[test]
fn dispatch_phase2_exact_type_wins_over_subtype() {
    // f(Number) vs f(Integer); call with Integer → f(Integer) (exact +3) beats
    // f(Number) (subtype +1).
    let source = r"
###Pure
function test::f(x: Number[1]): Boolean[1] { true; }
function test::f(x: Integer[1]): Boolean[1] { false; }
function test::caller(): Boolean[1] { f(42); }
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let target = dispatch_target(&model, &["test", "caller__Boolean_1_"]);
    assert!(
        target.starts_with("f_Integer_1_"),
        "phase 2 should prefer exact Integer over subtype-via-Number, got {target}"
    );
}

#[test]
fn dispatch_phase2_exact_multiplicity_wins() {
    // f(String[1]) vs f(String[0..1]); call with [1] → exact wins.
    let source = r"
###Pure
function test::f(x: String[0..1]): Boolean[1] { true; }
function test::f(x: String[1]): Boolean[1] { false; }
function test::caller(): Boolean[1] { f('hi'); }
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let target = dispatch_target(&model, &["test", "caller__Boolean_1_"]);
    assert!(
        target.starts_with("f_String_1_"),
        "phase 2 should prefer exact [1] over [0..1], got {target}"
    );
}

#[test]
fn dispatch_phase1_mult_eliminates_too_many() {
    // [0..1] arg cannot fit into [1] param.
    let source = r"
###Pure
function test::f(x: String[1]): Boolean[1] { true; }
function test::f(x: String[0..1]): Boolean[1] { false; }
function test::caller(s: String[0..1]): Boolean[1] { f($s); }
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let target = dispatch_target(&model, &["test", "caller_String_$0_1$__Boolean_1_"]);
    assert!(
        target.starts_with("f_String_$0_1$_"),
        "phase 1 mult should eliminate [1] overload when arg is [0..1], got {target}"
    );
}

#[test]
fn dispatch_generic_substitution_cast() {
    // Declares a native `myCast<T>(src:Any[1], t:T[1]):T[1]` so the test
    // doesn't depend on platform sources. Substitution must bind T := Integer
    // from the `@Integer` arg and make `$c` report Integer (not generic T).
    let source = r"
###Pure
native function test::myCast<T>(src: Any[1], t: T[1]): T[1];
function test::g(x: Integer[1]): Boolean[1] { true; }
function test::g(x: String[1]): Boolean[1] { false; }
function test::caller(): Boolean[1] {
    let c = myCast(1, @Integer);
    g($c);
}
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let target = dispatch_target(&model, &["test", "caller__Boolean_1_"]);
    assert!(
        target.starts_with("g_Integer_1_"),
        "generic substitution should reveal $c's concrete type (Integer), got {target}"
    );
}

#[test]
fn dispatch_generic_mult_substitution_toone() {
    // `$x->myToOne()` substitutes [0..1] → [1]. Dispatch sees concrete [1]
    // and picks the f([1]) overload.
    let source = r"
###Pure
native function test::myToOne<T>(x: T[0..1]): T[1];
function test::f(x: String[1]): Boolean[1] { true; }
function test::f(x: String[0..1]): Boolean[1] { false; }
function test::caller(s: String[0..1]): Boolean[1] { f($s->myToOne()); }
";
    let model = compile_with_imports(&[source], &[]).expect("should compile");
    let target = dispatch_target(&model, &["test", "caller_String_$0_1$__Boolean_1_"]);
    assert!(
        target.starts_with("f_String_1_"),
        "myToOne should substitute [0..1] to [1], got {target}"
    );
}

#[test]
fn dispatch_root_identifier_resolves() {
    // `Root` should alias the root package. If it doesn't resolve, the
    // compile errors out. The compile succeeding is the assertion.
    let source = r"
###Pure
native function test::accept(p: Any[1]): Boolean[1];
function test::caller(): Boolean[1] { accept(Root); }
";
    let _model = compile_with_imports(&[source], &[]).expect("Root should resolve");
}

#[test]
fn compile_let_shadows_function_parameter() {
    // Should fail cleanly, just like a block let shadowing a block let
    let source = "function test::f(x: Integer[1]): Integer[1] { let x = 42; $x; }";
    let Err(partial) = compile_one(source) else {
        panic!("Should fail to compile because let shadows function parameter");
    };
    assert_eq!(partial.errors.len(), 1);
    assert!(matches!(
        partial.errors[0].kind,
        legend_pure_parser_pure::error::CompilationErrorKind::DuplicateVariable { .. }
    ));
    assert_eq!(partial.errors[0].message, "'x' has already been defined!");
}

// ---------------------------------------------------------------------------
// Property-access compile-time validation
// ---------------------------------------------------------------------------

#[test]
fn unknown_property_error() {
    use legend_pure_parser_pure::error::CompilationErrorKind;

    let source = r"
        Class test::P { first: Integer[1]; }
        function test::f(p: test::P[1]): Any[*] { $p.firstType }
    ";
    let result = compile_one(source);
    let partial = result.expect_err("missing property must produce a compile error");
    let unknown: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::UnknownProperty { .. }))
        .collect();
    assert_eq!(
        unknown.len(),
        1,
        "expected exactly one UnknownProperty error, got {:?}",
        partial.errors
    );
    match &unknown[0].kind {
        CompilationErrorKind::UnknownProperty {
            type_name,
            property_name,
        } => {
            assert_eq!(type_name.as_str(), "P");
            assert_eq!(property_name.as_str(), "firstType");
        }
        _ => unreachable!(),
    }
    assert!(
        unknown[0]
            .message
            .contains("'firstType' can't be found in the type 'P'"),
        "diagnostic message should mention property and type, got: {}",
        unknown[0].message
    );
}

#[test]
fn known_property_compiles_clean() {
    // Regression guard: the new UnknownProperty check must not fire on
    // valid property access. We deliberately don't assert on the
    // substituted concrete type — generic substitution for property
    // types is a separate latent bug; this test only locks the error
    // does NOT fire.
    let source = r"
        Class test::P { first: Integer[1]; }
        function test::f(p: test::P[1]): Integer[1] { $p.first }
    ";
    let model = compile_one(source).expect("valid property access must compile");
    let id = model
        .resolve_function_by_path(&["test".into(), "f".into()])
        .expect("f should exist");
    if let Element::Function(f) = model.get_element(id) {
        assert_eq!(f.body.len(), 1);
        assert!(
            f.body[0].type_info.is_some(),
            "type_info must be populated for valid property access"
        );
    } else {
        panic!("expected Function");
    }
}

#[test]
fn qualified_property_arity_mismatch_error() {
    use legend_pure_parser_pure::error::CompilationErrorKind;

    let source = r"
        Class test::C
        {
            qp(x: Integer[1], y: Integer[1]) { $x }: Integer[1];
        }
        function test::f(c: test::C[1]): Integer[1] { $c.qp(1) }
    ";
    let result = compile_one(source);
    let partial = result.expect_err("wrong QP arity must produce a compile error");
    let arity: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::QualifiedPropertyArityMismatch { .. }
            )
        })
        .collect();
    assert_eq!(
        arity.len(),
        1,
        "expected exactly one arity-mismatch error, got: {:?}",
        partial.errors
    );
    match &arity[0].kind {
        CompilationErrorKind::QualifiedPropertyArityMismatch {
            type_name,
            property_name,
            expected,
            actual,
        } => {
            assert_eq!(type_name.as_str(), "C");
            assert_eq!(property_name.as_str(), "qp");
            assert_eq!(*expected, 2);
            assert_eq!(*actual, 1);
        }
        _ => unreachable!(),
    }
}

#[test]
fn qualified_property_arg_type_mismatch_error() {
    use legend_pure_parser_pure::error::CompilationErrorKind;

    let source = r"
        Class test::C
        {
            qp(x: Integer[1], y: Integer[1]) { $x }: Integer[1];
        }
        function test::f(c: test::C[1]): Integer[1] { $c.qp(1, 'two') }
    ";
    let result = compile_one(source);
    let partial = result.expect_err("wrong QP arg type must produce a compile error");
    let mismatches: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::QualifiedPropertyArgTypeMismatch { .. }
            )
        })
        .collect();
    assert_eq!(
        mismatches.len(),
        1,
        "expected exactly one arg-type mismatch, got: {:?}",
        partial.errors
    );
    match &mismatches[0].kind {
        CompilationErrorKind::QualifiedPropertyArgTypeMismatch {
            type_name,
            property_name,
            param_index,
            param_name,
            ..
        } => {
            assert_eq!(type_name.as_str(), "C");
            assert_eq!(property_name.as_str(), "qp");
            assert_eq!(*param_index, 1);
            assert_eq!(param_name.as_str(), "y");
        }
        _ => unreachable!(),
    }
}

#[test]
fn qualified_property_arg_multiplicity_mismatch_error() {
    use legend_pure_parser_pure::error::CompilationErrorKind;

    let source = r"
        Class test::C
        {
            qp(x: Integer[1]) { $x }: Integer[1];
        }
        function test::f(c: test::C[1]): Integer[1] { $c.qp([1, 2, 3]) }
    ";
    let result = compile_one(source);
    let partial = result.expect_err("wrong QP arg multiplicity must produce a compile error");
    let mismatches: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::QualifiedPropertyArgTypeMismatch { .. }
            )
        })
        .collect();
    assert_eq!(
        mismatches.len(),
        1,
        "expected exactly one multiplicity mismatch, got: {:?}",
        partial.errors
    );
    match &mismatches[0].kind {
        CompilationErrorKind::QualifiedPropertyArgTypeMismatch { param_index, .. } => {
            assert_eq!(*param_index, 0);
        }
        _ => unreachable!(),
    }
}

#[test]
fn qualified_property_overload_resolves_correctly() {
    // Mirrors `MyClassWithTypeVariables.res()` / `.res('z')` from the
    // platform: two QP overloads with the same name, distinguished by
    // arity. Both forms must compile cleanly.
    let source = r"
        Class test::C
        {
            qp() { 'zero' }: String[1];
            qp(z: String[1]) { $z }: String[1];
        }
        function test::f(c: test::C[1]): String[1] { $c.qp() }
        function test::g(c: test::C[1]): String[1] { $c.qp('hi') }
    ";
    compile_one(source).expect("QP overloads must resolve cleanly");
}

#[test]
fn association_injected_property_resolves() {
    // Regression guard for the early-freeze pipeline change: the
    // association-injected property must be visible to the
    // property-access lookup at inference time. Before the freeze move
    // (`rebuild_derived_indexes` ran AFTER `pass_infer`), the
    // association-properties index was empty during inference and the
    // navigation property would fail with `UnknownProperty`.
    let source = r"
        Class test::Person { firstName: String[1]; }
        Class test::Firm { name: String[1]; }
        Association test::Employment { employer: test::Firm[1]; employee: test::Person[*]; }
        function test::f(p: test::Person[1]): test::Firm[1] { $p.employer }
    ";
    compile_one(source).expect("association-injected property must resolve");
}

// ---------------------------------------------------------------------------
// Generic type / multiplicity substitution — closes the BACKLOG P1
// "Generic params treated as Any; causes false matches" entry.
// Each test pins one substitution gap. See plan
// `~/.claude/plans/do-we-have-enought-quiet-swing.md` for context.
// ---------------------------------------------------------------------------

#[test]
fn generic_subst_head_on_typed_collection_flows_element_type_clean() {
    // Positive baseline: `head<T>(T[*]):T[0..1]` called with `Integer[*]`
    // must bind T:=Integer and produce Integer[0..1]. The function header
    // declares Integer[0..1] return — substitution + body-return-signature
    // check together keep the compile clean.
    let source = r"
###Pure
native function test::head<T>(c: T[*]): T[0..1];
function test::caller(): Integer[0..1] { [1, 2, 3]->head() }
";
    compile_with_imports(&[source], &[]).expect("head on Integer[*] must yield Integer[0..1]");
}

#[test]
fn generic_subst_head_on_typed_collection_mismatched_decl_errors() {
    // Negative twin: declare String[0..1] return on a body that produces
    // Integer[0..1]. Inference must surface a body-return-signature error.
    let source = r"
###Pure
native function test::head<T>(c: T[*]): T[0..1];
function test::caller(): String[0..1] { [1, 2, 3]->head() }
";
    let result = compile_with_imports(&[source], &[]);
    let partial = result.expect_err(
        "head on Integer[*] returns Integer — declaring String[0..1] must error",
    );
    assert!(
        partial.errors.iter().any(|e| {
            e.message.contains("return") || e.message.contains("Argument")
        }),
        "expected return-type or argument error, got: {:?}",
        partial.errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn generic_subst_head_on_empty_collection_treats_t_as_nil() {
    // Plan decision (b): empty `[]` binds T:=Nil so head returns
    // Nil[0..1]. Nil is subtype of every type, so this declaration
    // compiles cleanly regardless of declared return type.
    let source = r"
###Pure
native function test::head<T>(c: T[*]): T[0..1];
function test::caller(): String[0..1] { []->head() }
";
    compile_with_imports(&[source], &[]).expect(
        "head on empty collection should bind T:=Nil and accept any \
         declared return type (Nil <: T for all T)",
    );
}

#[test]
fn generic_subst_two_arg_homogeneous_returns_concrete_t() {
    // f<T>(T,T):T called with (Integer, Integer) → T:=Integer.
    let source = r"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Integer[1] { pick(1, 2) }
";
    compile_with_imports(&[source], &[])
        .expect("homogeneous Integer args must bind T:=Integer");
}

#[test]
fn generic_subst_two_arg_subtype_lubs_to_supertype() {
    // f<T>(T,T):T called with (Integer, Float) → T:=Number (LUB walks the
    // numeric hierarchy). Declaring Number[1] return must compile clean.
    let source = r"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Number[1] { pick(1, 1.5) }
";
    compile_with_imports(&[source], &[])
        .expect("Integer + Float must LUB to Number, not stay at Integer");
}

#[test]
fn generic_subst_two_arg_subtype_lub_decl_integer_errors() {
    // Negative twin of the LUB test: with (Integer, Float) the bound T is
    // Number, so declaring Integer[1] return must error.
    let source = r"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Integer[1] { pick(1, 1.5) }
";
    let result = compile_with_imports(&[source], &[]);
    let partial = result.expect_err(
        "T:=Number from Integer+Float LUB is incompatible with declared Integer[1]",
    );
    assert!(
        !partial.errors.is_empty(),
        "expected at least one return/type error"
    );
}

#[test]
fn generic_subst_two_arg_unrelated_lubs_to_any() {
    // f<T>(T,T):T called with (Integer, String) → T:=Any. Declaring
    // Any[1] return compiles clean (consistent with Collection LUB:
    // [1, 'x'] is Collection<Any>[*]).
    let source = r"
###Pure
native function test::pick<T>(a: T[1], b: T[1]): T[1];
function test::caller(): Any[1] { pick(1, 'x') }
";
    compile_with_imports(&[source], &[])
        .expect("Integer + String must LUB to Any (mirror of Collection LUB)");
}

#[test]
fn generic_subst_multiplicity_var_threads_through_return() {
    // f<T|m>(T[m]):T[m] — multiplicity threads from arg to return. Called
    // with Integer[2..2] (literal collection of two), declared
    // Integer[*]: must compile cleanly because Integer[2..2] ⊆ Integer[*].
    let source = r"
###Pure
native function test::ident<T|m>(p: T[m]): T[m];
function test::caller(): Integer[*] { ident([1, 2]) }
";
    compile_with_imports(&[source], &[])
        .expect("ident<T|m> must thread Integer[2..2] through to Integer[2..2]");
}

#[test]
fn generic_subst_multiplicity_var_decl_pure_one_errors() {
    // ident([1,2]) returns Integer[2..2], not Integer[1]. Declaring [1]
    // must surface a multiplicity error.
    let source = r"
###Pure
native function test::ident<T|m>(p: T[m]): T[m];
function test::caller(): Integer[1] { ident([1, 2]) }
";
    let result = compile_with_imports(&[source], &[]);
    let partial = result.expect_err(
        "Integer[2..2] from ident([1,2]) cannot satisfy declared Integer[1]",
    );
    assert!(!partial.errors.is_empty());
}

#[test]
fn generic_subst_property_chain_through_generic_class() {
    // Property access through a generic class: the receiver's type-args
    // (`Box<String>`) must flow into property `value`'s declared `T`.
    // Today `extract_receiver_type_args` only handles `ExprKind::Variable`,
    // so direct-variable receivers work but chained receivers
    // (`func()->prop`) lose the bindings. Validate the variable case is
    // green; the chained case lives in a follow-up test.
    let source = r"
###Pure
Class test::Box<T> { value: T[1]; }
function test::caller(b: test::Box<String>[1]): String[1] { $b.value }
";
    compile_with_imports(&[source], &[]).expect(
        "$b.value on Box<String> must produce String[1] via class-generic substitution",
    );
}

#[test]
fn generic_subst_property_chain_via_function_receiver() {
    // Chained receiver: factory()->value. The receiver isn't a bare
    // Variable; today `extract_receiver_type_args` returns vec![] for
    // anything other than ExprKind::Variable, so the property's `T` stays
    // as Generic and degrades downstream. Plan Step 2 fixes this by
    // delegating to `infer_typeexpr_from_valuespec` (which already
    // handles arbitrary receivers).
    let source = r"
###Pure
Class test::Box<T> { value: T[1]; }
native function test::makeBox(): test::Box<String>[1];
function test::caller(): String[1] { makeBox().value }
";
    compile_with_imports(&[source], &[]).expect(
        "Chained receiver makeBox().value must thread Box<String>'s T:=String through",
    );
}

#[test]
fn generic_subst_property_chain_via_function_receiver_decl_integer_errors() {
    // Sensitivity twin for the chain test: if substitution actually
    // resolved `value` to `String[1]`, declaring Integer[1] return must
    // error. If the gap leaves `value` as `T[1]` / `Any[1]`, the
    // body-return-signature check skips on Any and the test silently
    // passes — so this asserts the substitution genuinely happens.
    let source = r"
###Pure
Class test::Box<T> { value: T[1]; }
native function test::makeBox(): test::Box<String>[1];
function test::caller(): Integer[1] { makeBox().value }
";
    let result = compile_with_imports(&[source], &[]);
    let partial = result.expect_err(
        "Box<String>.value resolves to String[1]; declaring Integer[1] must error",
    );
    assert!(
        !partial.errors.is_empty(),
        "expected at least one error from chained generic substitution"
    );
}

#[test]
fn generic_subst_property_chain_with_mult_variable() {
    // Class-level multiplicity parameter on a property: receiver
    // `Holder<String|m>` carries `m`, property `items: T[m]`. Today
    // `multiplicity_product` bails to receiver-only when the property's
    // multiplicity is `Variable`. Plan Step 3 plumbs class-mult
    // substitution through.
    let source = r"
###Pure
Class test::Holder<T|m> { items: T[m]; }
function test::caller(h: test::Holder<String|*>[1]): String[*] { $h.items }
";
    compile_with_imports(&[source], &[]).expect(
        "$h.items on Holder<String|*> must flow String[*] through class-mult substitution",
    );
}

#[test]
fn generic_subst_property_chain_with_mult_variable_decl_one_errors() {
    // Sensitivity twin for the mult-variable test: declare String[1]
    // return on a body whose actual multiplicity is `[*]`. If
    // substitution works, the body's String[*] cannot fit declared [1]
    // → error. If multiplicity_product silently bails to receiver-only
    // ([1]), the test passes silently and the gap is undetected.
    let source = r"
###Pure
Class test::Holder<T|m> { items: T[m]; }
function test::caller(h: test::Holder<String|*>[1]): String[1] { $h.items }
";
    let result = compile_with_imports(&[source], &[]);
    let partial = result.expect_err(
        "items on Holder<String|*> resolves to String[*]; declaring String[1] must error",
    );
    assert!(
        !partial.errors.is_empty(),
        "expected multiplicity error from class-mult substitution"
    );
}

// ---------------------------------------------------------------------------
// M3 property types — diagnostic & lock for the BACKLOG P1 ⚠️ Partial item
// ---------------------------------------------------------------------------

#[test]
fn m3_property_parametric_types_preserved() {
    // Regression lock for the M3 property type-args fix: any property
    // whose declared rawType in `m3.pure` is a generic class
    // (`Property<U,V>`, `Class<T>`, …) must carry non-empty
    // `type_arguments` after `resolve_m3_supertypes`. Before the fix
    // these stripped to bare `Named { Property, type_arguments: [] }`,
    // dropping anchoring info for downstream reflection chains.
    use legend_pure_parser_pure::ids::ElementId;
    use legend_pure_parser_pure::model::Element;
    use legend_pure_parser_pure::types::TypeExpr;

    let model = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let chunk = &model.chunks[0];
    let mut stripped: Vec<String> = Vec::new();
    for local_idx in 0..chunk.elements.len() {
        let id = ElementId::InstanceId {
            chunk_id: 0,
            local_idx,
        };
        let node = chunk.nodes.get(id.local_idx());
        let elem = chunk.elements.get(id.local_idx());
        if let Element::Class(c) = elem {
            for p in &c.properties {
                if let TypeExpr::Named {
                    element,
                    type_arguments,
                    ..
                } = &p.type_expr
                {
                    let name = model.element_name(*element);
                    if type_arguments.is_empty()
                        && let Some(Element::Class(target)) = model.try_get_element(*element)
                        && !target.type_parameters.is_empty()
                    {
                        stripped.push(format!(
                            "{}.{}: {}<{}>",
                            node.name,
                            p.name,
                            name,
                            target.type_parameters.join(", ")
                        ));
                    }
                }
            }
        }
    }
    assert!(
        stripped.is_empty(),
        "M3 properties with stripped parametric types ({}):\n  {}",
        stripped.len(),
        stripped.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Lambda parameter inference from `Function<{T->X}>` shape
// ---------------------------------------------------------------------------

#[test]
fn lambda_param_inferred_from_filter_signature() {
    // `filter<T>(coll: T[*], pred: Function<{T[1]->Boolean[1]}>[1]): T[*]`
    // Called with `[1, 2, 3]->filter(x | $x->greaterThan(0))`.
    // T binds Integer from coll → lambda's `x` should be Integer[1] → the
    // body's `$x->greaterThan(0)` dispatches the Integer-typed overload
    // unambiguously and the compile is clean.
    let source = r"
###Pure
native function test::filter<T>(coll: T[*], pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]): T[*];
native function test::greaterThan(a: Integer[1], b: Integer[1]): Boolean[1];
function test::caller(): Integer[*] { [1, 2, 3]->filter(x | $x->greaterThan(0)) }
";
    compile_with_imports(&[source], &[])
        .expect("filter must thread T:=Integer through to lambda param x");
}

#[test]
fn lambda_param_inferred_via_chained_generic_class() {
    // Lambda inside a chain whose receiver is a generic class.
    // `Holder<T> { items: T[*]; }; func<T>(h:Holder<T>[1], pred:Function<{T[1]->Boolean[1]}>[1]):T[*]`
    // Receiver `Holder<String>[1]` binds T:=String → pred's x is String[1]
    // → body `$x->equalString('')` resolves to the String overload.
    let source = r"
###Pure
Class test::Holder<T> { items: T[*]; }
native function test::pickItems<T>(h: test::Holder<T>[1], pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]): T[*];
native function test::isEmpty(s: String[1]): Boolean[1];
function test::caller(h: test::Holder<String>[1]): String[*] { pickItems($h, x | $x->isEmpty()) }
";
    compile_with_imports(&[source], &[])
        .expect("pickItems must bind T:=String from Holder<String> and type lambda param x as String[1]");
}

#[test]
fn lambda_param_inferred_for_t_to_t_signature() {
    // `mutate<T>(coll: T[*], f: Function<{T[1]->T[1]}>[1]): T[*]`
    // Lambda's input AND output are T. T binds from coll → lambda's x and
    // its return are both Integer.
    let source = r"
###Pure
native function test::mutate<T>(coll: T[*], f: meta::pure::metamodel::function::Function<{T[1]->T[1]}>[1]): T[*];
native function test::identity(x: Integer[1]): Integer[1];
function test::caller(): Integer[*] { [1, 2, 3]->mutate(x | $x->identity()) }
";
    compile_with_imports(&[source], &[])
        .expect("mutate must bind T:=Integer for both arg and return positions of the lambda");
}

#[test]
fn lambda_param_typed_arg_dispatches_specific_overload() {
    // Sensitivity check: the lambda's body uses `$x` against an
    // overloaded function, where dispatch can pick the wrong overload
    // if `x`'s type is `Any` or `Unresolved`. Filter binds T:=Integer
    // → x:Integer[1] → dispatch picks the Integer overload of `pickOne`.
    let source = r"
###Pure
native function test::filter<T>(coll: T[*], pred: meta::pure::metamodel::function::Function<{T[1]->Boolean[1]}>[1]): T[*];
native function test::pickOne(x: Integer[1]): Boolean[1];
native function test::pickOne(x: String[1]): Boolean[1];
function test::caller(): Integer[*] { [1, 2, 3]->filter(x | $x->pickOne()) }
";
    compile_with_imports(&[source], &[])
        .expect("filter must thread T:=Integer so pickOne(Integer) wins dispatch");
}

#[test]
fn lambda_multi_param_inferred_from_function_shape() {
    // Two-param lambda `{x, y | ...}` passed to
    // `Function<{T[1], U[1]->Boolean[1]}>[1]`. Both T and U bind from
    // sibling args (or coll-style). x and y must each get their typed
    // expectations.
    let source = r"
###Pure
native function test::zipPred<T, U>(
  a: T[*],
  b: U[*],
  pred: meta::pure::metamodel::function::Function<{T[1], U[1]->Boolean[1]}>[1]
): Boolean[1];
native function test::eqInt(a: Integer[1], b: Integer[1]): Boolean[1];
function test::caller(): Boolean[1] { zipPred([1, 2], [3, 4], {x, y | eqInt($x, $y)}) }
";
    compile_with_imports(&[source], &[])
        .expect("zipPred must bind T:=Integer, U:=Integer and type both x, y as Integer[1]");
}
