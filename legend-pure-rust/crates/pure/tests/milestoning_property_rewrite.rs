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

//! Phase A milestoning — property-rewrite synthesis.
//!
//! For each declared property `p: T[mult]` where `T` is a milestoned class,
//! the synthesis pass:
//!
//! - Moves the original `p` into `Class.original_milestoned_properties`.
//! - Adds an edge-point property `pAllVersions: T[*]` (lower-bound preserved)
//!   to `Class.properties`.
//! - Adds qualified-property signatures `p(td: Date[1]): T[mult]` and
//!   `pAllVersionsInRange(start: Date[1], end: Date[1]): T[mult]`.
//! - For bitemporal targets, additionally adds `p(pd: Date[1], bd: Date[1])`.
//!
//! Bodies are intentionally empty in Phase A (deferred to Phase B with the
//! `getAll(...)` natives + date propagation). The signatures alone are what
//! lets dispatch / IDE / codegen see the correct surface.

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::nodes::class::{Class, QualifiedProperty};
use legend_pure_parser_pure::pipeline::{compile_repo_slice, init_bootstrap_model};
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};

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

fn fresh_model() -> PureModel {
    let mut model = init_bootstrap_model();
    let sf = parse(MILESTONING_PROFILE_SOURCE, "milestoning_metamodel.pure");
    let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    assert!(errs.is_empty(), "metamodel errors: {errs:?}");
    model
}

fn compile_fixture(model: &mut PureModel, source: &str) {
    let sf = parse(source, "fixture.pure");
    let (_r, errs) = compile_repo_slice(model, &[sf], &[], &[]);
    assert!(errs.is_empty(), "fixture errors: {errs:?}");
}

fn resolve(model: &PureModel, fqn: &[&str]) -> ElementId {
    model
        .resolve_by_path(
            &fqn.iter()
                .map(|s| smol_str::SmolStr::new(*s))
                .collect::<Vec<_>>(),
        )
        .expect("FQN must resolve")
}

fn cloned_class(model: &PureModel, id: ElementId) -> Class {
    match model.get_element(id) {
        Element::Class(c) => c.clone(),
        other => panic!("expected Class, got {other:?}"),
    }
}

fn find_qp<'a>(qps: &'a [QualifiedProperty], name: &str, arity: usize) -> &'a QualifiedProperty {
    qps.iter()
        .find(|q| q.name == name && q.parameters.len() == arity)
        .unwrap_or_else(|| panic!("no QP '{name}'/{arity} in: {qps:?}"))
}

// -------------------------------------------------------------------------
// Non-milestoned owner with a businesstemporal target
// -------------------------------------------------------------------------

#[test]
fn property_to_businesstemporal_target_synthesizes_edgepoint_and_two_qps() {
    let mut model = fresh_model();
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Address
{
    line: String[1];
}

Class demo::Customer
{
    address: demo::Address[1];
}
";
    compile_fixture(&mut model, source);

    let customer = cloned_class(&model, resolve(&model, &["demo", "Customer"]));

    // Original moved aside.
    assert_eq!(customer.original_milestoned_properties.len(), 1);
    assert_eq!(customer.original_milestoned_properties[0].name, "address");

    // Edge-point present with `[1..*]` multiplicity (widened from `[1]`).
    let edge = customer
        .properties
        .iter()
        .find(|p| p.name == "addressAllVersions")
        .expect("addressAllVersions edge-point");
    assert_eq!(edge.multiplicity, Multiplicity::OneOrMany);
    assert!(matches!(
        &edge.type_expr,
        TypeExpr::Named { element, .. }
            if model.element_name(*element) == "Address"
    ));

    // Original `address` is GONE from class.properties.
    assert!(!customer.properties.iter().any(|p| p.name == "address"));

    // Two QPs: `address(td: Date[1])` and `addressAllVersionsInRange(start, end)`.
    let one_arg = find_qp(&customer.qualified_properties, "address", 1);
    assert_eq!(one_arg.parameters[0].name, "td");
    assert_eq!(one_arg.return_multiplicity, Multiplicity::PureOne);

    let range = find_qp(
        &customer.qualified_properties,
        "addressAllVersionsInRange",
        2,
    );
    assert_eq!(range.parameters[0].name, "start");
    assert_eq!(range.parameters[1].name, "end");
    assert_eq!(range.return_multiplicity, Multiplicity::PureOne);

    // No zero-arg variant — owner is not milestoned.
    assert!(
        customer
            .qualified_properties
            .iter()
            .all(|q| !(q.name == "address" && q.parameters.is_empty())),
        "non-milestoned owner should not get a zero-arg variant",
    );
}

// -------------------------------------------------------------------------
// Milestoned-on-milestoned target: zero-arg `p()` is added when same kind
// -------------------------------------------------------------------------

#[test]
fn milestoned_owner_with_same_kind_target_adds_no_arg_variant() {
    let mut model = fresh_model();
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Address
{
    line: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Customer
{
    address: demo::Address[1];
}
";
    compile_fixture(&mut model, source);

    let customer = cloned_class(&model, resolve(&model, &["demo", "Customer"]));

    // The no-arg variant should be present.
    let _no_arg = find_qp(&customer.qualified_properties, "address", 0);
    // Plus the single-date variant.
    let _one_arg = find_qp(&customer.qualified_properties, "address", 1);
    // Plus the range variant.
    let _range = find_qp(
        &customer.qualified_properties,
        "addressAllVersionsInRange",
        2,
    );
}

// -------------------------------------------------------------------------
// Bitemporal target: two-arg `p(pd, bd)` variant present
// -------------------------------------------------------------------------

#[test]
fn bitemporal_target_synthesizes_two_parameter_variant() {
    let mut model = fresh_model();
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> demo::Address
{
    line: String[1];
}

Class demo::Customer
{
    address: demo::Address[1];
}
";
    compile_fixture(&mut model, source);

    let customer = cloned_class(&model, resolve(&model, &["demo", "Customer"]));

    let two_arg = find_qp(&customer.qualified_properties, "address", 2);
    assert_eq!(two_arg.parameters[0].name, "pd");
    assert_eq!(two_arg.parameters[1].name, "bd");

    let _one_arg = find_qp(&customer.qualified_properties, "address", 1);
    let _range = find_qp(
        &customer.qualified_properties,
        "addressAllVersionsInRange",
        2,
    );
}

// -------------------------------------------------------------------------
// Multiplicity widening preserves lower bound
// -------------------------------------------------------------------------

#[test]
fn many_target_widens_to_zero_or_many() {
    let mut model = fresh_model();
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Address
{
    line: String[1];
}

Class demo::Customer
{
    addresses: demo::Address[*];
}
";
    compile_fixture(&mut model, source);

    let customer = cloned_class(&model, resolve(&model, &["demo", "Customer"]));

    let edge = customer
        .properties
        .iter()
        .find(|p| p.name == "addressesAllVersions")
        .expect("addressesAllVersions edge-point");
    assert_eq!(edge.multiplicity, Multiplicity::ZeroOrMany);
}
