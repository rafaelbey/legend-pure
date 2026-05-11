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

//! Phase-B' predicate `Boolean[1]` validation tests.
//!
//! Locks the narrow op-typer rule wired in
//! `RelationalExtension::validate`: Filter / Join / MultiGrainFilter
//! `op_operation` bodies must reduce to Boolean (or `Any` for
//! un-modeled DynaFunctions and unresolved column refs). Java parity
//! with `DatabaseProcessor`'s predicate-shape check, narrowed so we
//! don't false-positive while full DynaFunction lowering stays
//! deferred to RT-1 / INT-1.

use indoc::indoc;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;
use smol_str::SmolStr;

fn run_validator(source: &str) -> Vec<CompilationError> {
    let file = legend_pure_parser_parser::parse_with_sections(
        source,
        "predicate_typecheck_smoke.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    )
    .expect("source must parse");

    let files: [SourceFile; 1] = [file];
    let extension = RelationalExtension::new();
    let mut errors: Vec<CompilationError> = Vec::new();
    let mut bootstrap = legend_pure_parser_pure::pipeline::init_bootstrap_model();
    let auto_imports: Vec<SmolStr> = Vec::new();

    let mut declare_ctx = legend_pure_parser_pure::extension::DeclareCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.declare(&mut declare_ctx);
    let mut define_ctx = legend_pure_parser_pure::extension::DefineCtx {
        source_files: &files,
        model: &mut bootstrap,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.define_bodies(&mut define_ctx);
    let frozen = bootstrap;
    let mut validate_ctx = legend_pure_parser_pure::extension::ValidateCtx {
        model: &frozen,
        auto_imports: &auto_imports,
        errors: &mut errors,
    };
    extension.validate(&mut validate_ctx);
    errors
}

fn has_predicate_return_type_error(errors: &[CompilationError], substring: &str) -> bool {
    errors.iter().any(|e| {
        e.message.contains("predicate")
            && e.message.contains("must return Boolean[1]")
            && e.message.contains(substring)
    })
}

#[test]
fn filter_compare_predicate_passes() {
    // `tradeTable.id > 0` is a Compare → Boolean. Clean.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, qty FLOAT(10,2) )
          Filter active(tradeTable.id > 0)
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("active")),
        "no predicate-return-type error expected on Compare body; got: {errors:#?}"
    );
}

#[test]
fn join_with_target_alias_compare_passes() {
    // `mainCol = {target}.col` — Compare(Column(Aliased),
    // Column(Target)). Target column types `Any`; the compare
    // still folds to Boolean.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table customers ( id INTEGER PRIMARY KEY )
          Table orders    ( id INTEGER PRIMARY KEY, customer_id INTEGER )
          Join cust_orders(customers.id = {target}.customer_id)
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("cust_orders")),
        "no predicate-return-type error expected on Compare-with-target body; got: {errors:#?}"
    );
}

#[test]
fn multi_grain_filter_with_target_passes() {
    // `tradeTable.month = {target}.month` — Compare. Boolean. Clean.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, month INTEGER )
          MultiGrainFilter byMonth(tradeTable.month = {target}.month)
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("byMonth")),
        "no predicate-return-type error expected on MultiGrainFilter body; got: {errors:#?}"
    );
}

#[test]
fn filter_with_non_boolean_column_body_errors() {
    // Filter body is just a column ref: `tradeTable.qty` typed
    // FLOAT → Numeric. Must error.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, qty FLOAT(10,2) )
          Filter bug(tradeTable.qty)
        )
    "};
    let errors = run_validator(source);
    assert!(
        has_predicate_return_type_error(&errors, "Numeric")
            && errors
                .iter()
                .any(|e| e.message.contains("Filter") && e.message.contains("bug")),
        "expected Filter predicate-return-type error on Numeric column body; got: {errors:#?}"
    );
}

#[test]
fn filter_with_string_column_body_errors() {
    // Filter body is `tradeTable.name` typed VARCHAR → String.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, name VARCHAR(100) )
          Filter bug(tradeTable.name)
        )
    "};
    let errors = run_validator(source);
    assert!(
        has_predicate_return_type_error(&errors, "String"),
        "expected Filter predicate-return-type error on String column body; got: {errors:#?}"
    );
}

#[test]
fn filter_with_known_boolean_dynafunction_passes() {
    // DynaFunction `equal(...)` is in
    // `KNOWN_BOOLEAN_DYNAFUNCTIONS` → typed Boolean. No
    // predicate-return-type error.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, status VARCHAR(10) )
          Filter active(equal(tradeTable.status, 'A'))
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("active")),
        "no predicate-return-type error expected on equal(...) body; got: {errors:#?}"
    );
}

#[test]
fn filter_with_unknown_dynafunction_does_not_emit_predicate_error() {
    // Unknown DynaFunction `madeUpFn(...)` types as `Any` →
    // accepted by the validator (no false positive while full
    // DynaFunction lowering stays deferred).
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY )
          Filter mystery(madeUpFn(tradeTable.id))
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("mystery")),
        "no predicate-return-type error expected on unknown-DynaFunction body; \
         got: {errors:#?}"
    );
}

// ---------------------------------------------------------------------------
// Java-parity pin on the boolean-shape DynaFunction allow-list (T3.1 audit)
//
// Java's canonical classifier
// `meta::relational::functions::sqlQueryToString::isBooleanOperation`
// lives at `legend-engine-xt-relationalStore-core-pure/.../sqlQueryToString/
// dbExtension.pure:803`. The set below is its verbatim membership. Any
// drift between Java's source-of-truth and our `KNOWN_BOOLEAN_DYNAFUNCTIONS`
// is a parity regression — predicates that Java accepts would either
// error or silently degrade to `Any` on our side.
// ---------------------------------------------------------------------------

#[test]
fn known_boolean_dynafunctions_match_java_isbooleanoperation_set() {
    use legend_pure_dsl_relational::op_typer::KNOWN_BOOLEAN_DYNAFUNCTIONS;
    use std::collections::HashSet;

    // Verbatim copy of Java's `isBooleanOperation` list (dbExtension.pure:803).
    let java_canonical: HashSet<&str> = [
        "or",
        "and",
        "lessThan",
        "lessThanEqual",
        "greaterThan",
        "greaterThanEqual",
        "equal",
        "notEqual",
        "notEqualAnsi",
        "startsWith",
        "endsWith",
        "contains",
        "isEmpty",
        "isNotEmpty",
        "isNull",
        "isNotNull",
        "isAlphaNumeric",
        "exists",
        "not",
        "in",
        "isNumeric",
        "matches",
        "isDistinct",
    ]
    .into_iter()
    .collect();
    let ours: HashSet<&str> = KNOWN_BOOLEAN_DYNAFUNCTIONS.iter().copied().collect();

    let missing: Vec<&&str> = java_canonical.difference(&ours).collect();
    let extras: Vec<&&str> = ours.difference(&java_canonical).collect();

    assert!(
        missing.is_empty(),
        "Java's isBooleanOperation accepts these names but ours rejects them: {missing:?}. \
         Java accepting a predicate we reject is a parity regression; widen \
         KNOWN_BOOLEAN_DYNAFUNCTIONS in op_typer.rs."
    );
    assert!(
        extras.is_empty(),
        "Ours has names Java's isBooleanOperation never accepts: {extras:?}. \
         These cannot come from Java-emitted protocol; if they're meaningful for \
         a Rust-only path, document why — otherwise drop them to keep parity tight."
    );
}

#[test]
fn filter_with_starts_with_dynafunction_passes() {
    // `startsWith` is in Java's isBooleanOperation set. Pin it
    // separately from the parity test so the SQL-emission work
    // landing later (T1.4) trips on any accidental removal.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, status VARCHAR(10) )
          Filter prefixed(startsWith(tradeTable.status, 'A'))
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("prefixed")),
        "no predicate-return-type error expected on startsWith(...) body; got: {errors:#?}"
    );
}

#[test]
fn filter_with_is_empty_dynafunction_passes() {
    // `isEmpty` is Java's null-check shape — must type Boolean.
    let source = indoc! {r"
        ###Relational
        Database pkg::Db
        (
          Table tradeTable ( id INTEGER PRIMARY KEY, status VARCHAR(10) )
          Filter blanks(isEmpty(tradeTable.status))
        )
    "};
    let errors = run_validator(source);
    assert!(
        !errors
            .iter()
            .any(|e| e.message.contains("predicate") && e.message.contains("blanks")),
        "no predicate-return-type error expected on isEmpty(...) body; got: {errors:#?}"
    );
}
