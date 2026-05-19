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

//! Phase A milestoning — declare-side validators.
//!
//! Locks the four validators wired in `hydrate_element_signature`
//! (A4.1 / A4.2 / A4.3) and the in-synthesis collision check (A4.4).
//! Each asserts the diagnostic `kind` (programmatic surface) plus a
//! message-substring (human surface).

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::pipeline::{compile_repo_slice, init_bootstrap_model};

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

fn compile_collect_errors(source: &str) -> Vec<CompilationError> {
    let mut model = fresh_model();
    let sf = parse(source, "fixture.pure");
    let (_r, errs) = compile_repo_slice(&mut model, &[sf], &[], &[]);
    errs
}

// -------------------------------------------------------------------------
// A4.1 — at-most-one temporal stereotype
// -------------------------------------------------------------------------

#[test]
fn a4_1_two_temporal_stereotypes_errors() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal, meta::pure::profiles::temporal.processingtemporal>> demo::Doomed
{
    label: String[1];
}
";
    let errors = compile_collect_errors(source);
    let conflict = errors
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::MilestoningStereotypeConflict { .. }
            )
        })
        .unwrap_or_else(|| panic!("expected MilestoningStereotypeConflict, got: {errors:?}"));
    if let CompilationErrorKind::MilestoningStereotypeConflict {
        class_name,
        stereotypes,
    } = &conflict.kind
    {
        assert_eq!(class_name, "Doomed");
        assert!(stereotypes.iter().any(|s| s == "businesstemporal"));
        assert!(stereotypes.iter().any(|s| s == "processingtemporal"));
    }
    assert!(
        conflict
            .message
            .contains("more than one temporal stereotype")
    );
}

// -------------------------------------------------------------------------
// A4.2 — reserved property names
// -------------------------------------------------------------------------

#[test]
fn a4_2_reserved_property_name_errors() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Reserved
{
    businessDate: String[1];
}
";
    let errors = compile_collect_errors(source);
    let reserved = errors
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::MilestoningReservedPropertyName { .. }
            )
        })
        .unwrap_or_else(|| panic!("expected MilestoningReservedPropertyName, got: {errors:?}"));
    if let CompilationErrorKind::MilestoningReservedPropertyName {
        class_name,
        property_name,
        stereotype,
    } = &reserved.kind
    {
        assert_eq!(class_name, "Reserved");
        assert_eq!(property_name, "businessDate");
        assert_eq!(stereotype, "businesstemporal");
    }
    assert!(reserved.message.contains("reserved by milestoning"));
}

#[test]
fn a4_2_milestoning_slot_name_is_reserved() {
    let source = "\
Class <<meta::pure::profiles::temporal.bitemporal>> demo::ReservedSlot
{
    milestoning: String[1];
}
";
    let errors = compile_collect_errors(source);
    let reserved = errors
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::MilestoningReservedPropertyName { .. }
            )
        })
        .expect("milestoning slot name should be flagged reserved");
    if let CompilationErrorKind::MilestoningReservedPropertyName { property_name, .. } =
        &reserved.kind
    {
        assert_eq!(property_name, "milestoning");
    }
}

// -------------------------------------------------------------------------
// A4.3 — hierarchy consistency
// -------------------------------------------------------------------------

#[test]
fn a4_3_subclass_with_different_stereotype_errors() {
    let source = "\
Class <<meta::pure::profiles::temporal.processingtemporal>> demo::Parent
{
    label: String[1];
}

Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Child extends demo::Parent
{
}
";
    let errors = compile_collect_errors(source);
    let mismatch = errors
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::MilestoningHierarchyMismatch { .. }
            )
        })
        .unwrap_or_else(|| panic!("expected MilestoningHierarchyMismatch, got: {errors:?}"));
    if let CompilationErrorKind::MilestoningHierarchyMismatch {
        class_name,
        own_stereotype,
        super_class_name,
        super_stereotype,
    } = &mismatch.kind
    {
        assert_eq!(class_name, "Child");
        assert_eq!(own_stereotype, "businesstemporal");
        assert_eq!(super_class_name, "Parent");
        assert_eq!(super_stereotype, "processingtemporal");
    }
}

// -------------------------------------------------------------------------
// A4.4 — edge-point name collision (in-synthesis seam)
// -------------------------------------------------------------------------

#[test]
fn a4_4_user_property_collides_with_synthesized_edge_point_name() {
    // Customer declares a user property `addressAllVersions` *and* a
    // milestoned-target property `address: Address[1]`. Synthesis would
    // otherwise produce a duplicate `addressAllVersions` — the validator
    // must surface a collision instead.
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Address
{
    line: String[1];
}

Class demo::Customer
{
    addressAllVersions: String[1];
    address: demo::Address[1];
}
";
    let errors = compile_collect_errors(source);
    let collision = errors
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::MilestoningEdgePointCollision { .. }
            )
        })
        .unwrap_or_else(|| panic!("expected MilestoningEdgePointCollision, got: {errors:?}"));
    if let CompilationErrorKind::MilestoningEdgePointCollision {
        class_name,
        user_property_name,
        synthesized_name,
    } = &collision.kind
    {
        assert_eq!(class_name, "Customer");
        assert_eq!(user_property_name, "addressAllVersions");
        assert_eq!(synthesized_name, "addressAllVersions");
    }
}

#[test]
fn a4_4_user_qp_collides_with_synthesized_range_name() {
    // Same shape but the user names a *qualified property* with the
    // synthesized range name. Synthesis must report the collision
    // instead of silently producing a duplicate QP.
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Address
{
    line: String[1];
}

Class demo::Customer
{
    address: demo::Address[1];
    addressAllVersionsInRange(){'shadow'} : String[1];
}
";
    let errors = compile_collect_errors(source);
    let collision = errors
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::MilestoningEdgePointCollision { .. }
            )
        })
        .unwrap_or_else(|| {
            panic!("expected MilestoningEdgePointCollision on range name, got: {errors:?}")
        });
    if let CompilationErrorKind::MilestoningEdgePointCollision {
        synthesized_name, ..
    } = &collision.kind
    {
        assert_eq!(synthesized_name, "addressAllVersionsInRange");
    }
}

#[test]
fn a4_3_same_stereotype_in_hierarchy_is_clean() {
    let source = "\
Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Parent {}

Class <<meta::pure::profiles::temporal.businesstemporal>> demo::Child extends demo::Parent {}
";
    let errors = compile_collect_errors(source);
    let has_mismatch = errors.iter().any(|e| {
        matches!(
            e.kind,
            CompilationErrorKind::MilestoningHierarchyMismatch { .. }
        )
    });
    assert!(
        !has_mismatch,
        "matching stereotypes should not trigger hierarchy mismatch; errors: {errors:?}"
    );
}
