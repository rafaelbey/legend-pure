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

//! Phase A milestoning — class-level date synthesis.
//!
//! Each test compiles a milestoned class against a minimal inline declaration
//! of the `temporal` / `milestoning` profiles and the `*Milestoning` carrier
//! classes (the subset of `platform/pure/grammar/milestoning.pure` needed for
//! synthesis to populate the slot property). Asserts that the synthesis pass
//! adds the expected date properties + `milestoning` slot property + their
//! `<<milestoning.generatedmilestoningdateproperty>>` stereotype.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::nodes::class::{Class, Property};
use legend_pure_parser_pure::pipeline::{compile_repo_slice, init_bootstrap_model};
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

/// Minimal subset of `platform/pure/grammar/milestoning.pure` — just the
/// `temporal` + `milestoning` profiles and the three `*Milestoning` carrier
/// classes. Excludes `getAll(...)` / `getAllVersionsInRange` (Phase B).
const MILESTONING_PROFILE_SOURCE: &str = "\
Profile meta::pure::profiles::temporal
{
    stereotypes: [bitemporal, businesstemporal, processingtemporal];
}

Profile meta::pure::profiles::milestoning
{
    stereotypes: [generatedmilestoningproperty, generatedmilestoningdateproperty];
}

Class meta::pure::milestoning::DateMilestoning {}
Class meta::pure::milestoning::ProcessingDateMilestoning extends meta::pure::milestoning::DateMilestoning
{
   in  : Date[1];
   out : Date[1];
}
Class meta::pure::milestoning::BusinessDateMilestoning extends meta::pure::milestoning::DateMilestoning
{
   from : Date[1];
   thru : Date[1];
}
Class meta::pure::milestoning::BiTemporalMilestoning \
    extends meta::pure::milestoning::ProcessingDateMilestoning, meta::pure::milestoning::BusinessDateMilestoning
{
}
";

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

fn fresh_model_with_milestoning_metamodel() -> PureModel {
    let mut model = init_bootstrap_model();
    let sf = parse(MILESTONING_PROFILE_SOURCE, "milestoning_metamodel.pure");
    let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    assert!(
        errs.is_empty(),
        "milestoning metamodel compile errors: {errs:?}"
    );
    model
}

fn compile_and_resolve(model: &mut PureModel, source: &str, fqn: &[&str]) -> ElementId {
    let sf = parse(source, "fixture.pure");
    let (_r, errs) = compile_repo_slice(model, &[sf], &[], &[]);
    assert!(
        errs.is_empty(),
        "fixture compile errors (synthesis should not error on legal inputs): {errs:?}"
    );
    model
        .resolve_by_path(
            &fqn.iter()
                .map(|s| smol_str::SmolStr::new(*s))
                .collect::<Vec<_>>(),
        )
        .expect("class must resolve after compile")
}

fn cloned_class(model: &PureModel, id: ElementId) -> Class {
    match model.get_element(id) {
        Element::Class(c) => c.clone(),
        other => panic!("expected Class, got {other:?}"),
    }
}

fn assert_property(props: &[Property], name: &str, expected_mult: &Multiplicity) -> Property {
    let prop = props
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("missing synthesized property '{name}': have {props:?}"));
    assert_eq!(
        &prop.multiplicity, expected_mult,
        "property '{name}' multiplicity mismatch",
    );
    prop.clone()
}

fn assert_date_type(prop: &Property) {
    match &prop.type_expr {
        TypeExpr::Named { element, .. } => {
            assert_eq!(
                *element,
                legend_pure_parser_pure::bootstrap::DATE_ID,
                "expected Date type on property '{}', got element id {:?}",
                prop.name,
                element,
            );
        }
        other => panic!(
            "expected TypeExpr::Named(Date) on property '{}', got {:?}",
            prop.name, other
        ),
    }
}

fn assert_generated_date_stereotype(prop: &Property) {
    let has = prop
        .stereotypes
        .iter()
        .any(|s| s.value == "generatedmilestoningdateproperty");
    assert!(
        has,
        "expected <<milestoning.generatedmilestoningdateproperty>> on property '{}', got {:?}",
        prop.name, prop.stereotypes
    );
}

// -------------------------------------------------------------------------
// businesstemporal
// -------------------------------------------------------------------------

#[test]
fn businesstemporal_class_gets_businessdate_and_milestoning_slot() {
    let mut model = fresh_model_with_milestoning_metamodel();
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Customer
{
    name: String[1];
}
";
    let id = compile_and_resolve(&mut model, source, &["demo", "Customer"]);
    let class = cloned_class(&model, id);

    let business_date = assert_property(&class.properties, "businessDate", &Multiplicity::PureOne);
    assert_date_type(&business_date);
    assert_generated_date_stereotype(&business_date);

    let milestoning_slot =
        assert_property(&class.properties, "milestoning", &Multiplicity::ZeroOrOne);
    match &milestoning_slot.type_expr {
        TypeExpr::Named { element, .. } => {
            assert_eq!(model.element_name(*element), "BusinessDateMilestoning");
        }
        other => panic!("milestoning slot type unexpected: {other:?}"),
    }
    assert_generated_date_stereotype(&milestoning_slot);

    assert!(
        class.properties.iter().any(|p| p.name == "name"),
        "user property 'name' should remain on milestoned class",
    );
    assert!(class.original_milestoned_properties.is_empty());
}

// -------------------------------------------------------------------------
// processingtemporal
// -------------------------------------------------------------------------

#[test]
fn processingtemporal_class_gets_processingdate_and_milestoning_slot() {
    let mut model = fresh_model_with_milestoning_metamodel();
    let source = "\
Class <<meta::pure::profiles::temporal.processingtemporal>> demo::Stamp
{
    label: String[1];
}
";
    let id = compile_and_resolve(&mut model, source, &["demo", "Stamp"]);
    let class = cloned_class(&model, id);

    let processing_date =
        assert_property(&class.properties, "processingDate", &Multiplicity::PureOne);
    assert_date_type(&processing_date);
    assert_generated_date_stereotype(&processing_date);

    let milestoning_slot =
        assert_property(&class.properties, "milestoning", &Multiplicity::ZeroOrOne);
    match &milestoning_slot.type_expr {
        TypeExpr::Named { element, .. } => {
            assert_eq!(model.element_name(*element), "ProcessingDateMilestoning");
        }
        other => panic!("milestoning slot type unexpected: {other:?}"),
    }

    assert!(
        !class.properties.iter().any(|p| p.name == "businessDate"),
        "businessDate should not be synthesized on a processingtemporal class",
    );
}

// -------------------------------------------------------------------------
// bitemporal
// -------------------------------------------------------------------------

#[test]
fn bitemporal_class_gets_both_dates_and_bitemporal_slot() {
    let mut model = fresh_model_with_milestoning_metamodel();
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> demo::Position
{
    qty: Integer[1];
}
";
    let id = compile_and_resolve(&mut model, source, &["demo", "Position"]);
    let class = cloned_class(&model, id);

    let processing_date =
        assert_property(&class.properties, "processingDate", &Multiplicity::PureOne);
    assert_date_type(&processing_date);
    let business_date = assert_property(&class.properties, "businessDate", &Multiplicity::PureOne);
    assert_date_type(&business_date);

    let milestoning_slot =
        assert_property(&class.properties, "milestoning", &Multiplicity::ZeroOrOne);
    match &milestoning_slot.type_expr {
        TypeExpr::Named { element, .. } => {
            assert_eq!(model.element_name(*element), "BiTemporalMilestoning");
        }
        other => panic!("milestoning slot type unexpected: {other:?}"),
    }
}

// -------------------------------------------------------------------------
// Non-milestoned class is untouched
// -------------------------------------------------------------------------

#[test]
fn non_milestoned_class_is_not_modified() {
    let mut model = fresh_model_with_milestoning_metamodel();
    let source = "\
Class demo::Plain
{
    label: String[1];
}
";
    let id = compile_and_resolve(&mut model, source, &["demo", "Plain"]);
    let class = cloned_class(&model, id);

    assert_eq!(
        class.properties.len(),
        1,
        "expected one property, got {:?}",
        class.properties
    );
    assert_eq!(class.properties[0].name, "label");
    assert!(class.original_milestoned_properties.is_empty());
    assert!(
        class.properties.iter().all(|p| p.name != "milestoning"),
        "milestoning slot should not be on non-milestoned class",
    );
}
