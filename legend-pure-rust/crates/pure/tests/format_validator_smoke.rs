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

//! Compile-time `format(...)` specifier validation (T-20260511-05).
//!
//! Java has no compile-time validator for `format` — this is a
//! strictly-stronger static check beyond Java parity, gated on both
//! the format string AND the args collection being literal. The
//! runtime fix in commit `d4326dc8291` covers the dynamic / non-
//! static cases (Java-parity exceptions: "Expected Integer, got: X").

use legend_pure_parser_ast::section::SourceFile;
use legend_pure_parser_pure::compile;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};

fn parse(source: &str, path: &str) -> SourceFile {
    legend_pure_parser_parser::parse(source, path).expect("parse should succeed")
}

#[allow(clippy::result_large_err)]
fn try_compile(
    source: &str,
) -> Result<
    legend_pure_parser_pure::model::PureModel,
    legend_pure_parser_pure::pipeline::PartialPureModel,
> {
    let sf = parse(source, "test.pure");
    compile!(&[sf])
}

/// Declares `meta::pure::functions::string::format` and any
/// supporting types so the validator can find a matching `ElementId`.
/// Bootstrap-only fixtures don't include the platform's `format`
/// declaration; the validator looks up by FQN and no-ops when absent.
const FORMAT_DECL: &str = r"
###Pure
native function
    meta::pure::functions::string::format(format: String[1], args: Any[*]): String[1];
";

fn type_mismatches(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::FormatSpecifierTypeMismatch { .. }
            )
        })
        .collect()
}

fn arity_mismatches(errors: &[CompilationError]) -> Vec<&CompilationError> {
    errors
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                CompilationErrorKind::FormatSpecifierArityMismatch { .. }
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. Bug repro from T-20260511-05 — `%d` on String at position 1
// ---------------------------------------------------------------------------

#[test]
fn bug_repro_string_on_d_at_position_1() {
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%s %d %d'->format(['hello', 'world', 4])
}}
"
    );
    let err = try_compile(&src).expect_err("must not compile");
    let hits = type_mismatches(&err.errors);
    let pos1: Vec<_> = hits
        .iter()
        .filter(|e| {
            matches!(
                &e.kind,
                CompilationErrorKind::FormatSpecifierTypeMismatch { arg_index: 1, .. }
            )
        })
        .collect();
    assert!(
        !pos1.is_empty(),
        "must surface position-1 mismatch ('world' on %d); got {:?}",
        err.errors
    );
    // Confirm the variant payload — specifier `%d`, expected Integer,
    // actual String.
    if let CompilationErrorKind::FormatSpecifierTypeMismatch {
        specifier,
        expected,
        actual,
        ..
    } = &pos1[0].kind
    {
        assert_eq!(specifier.as_str(), "%d");
        assert_eq!(expected.as_str(), "Integer");
        assert!(
            actual.contains("String"),
            "actual must mention String; got {actual}"
        );
    } else {
        unreachable!()
    }
}

// ---------------------------------------------------------------------------
// 2. Positive controls — all-correct arg types compile
// ---------------------------------------------------------------------------

#[test]
fn well_typed_format_compiles() {
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%s %d'->format(['x', 1])
}}
"
    );
    try_compile(&src).expect("well-typed format must compile");
}

#[test]
fn percent_s_accepts_any_value() {
    // %s and %r are Java-parity Any positions — no compile-time check.
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%s %r'->format([42, true])
}}
"
    );
    try_compile(&src).expect("%s and %r must accept any type at compile time");
}

// ---------------------------------------------------------------------------
// 3. Arity mismatches
// ---------------------------------------------------------------------------

#[test]
fn arity_too_few_args_errors() {
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%s %s'->format(['only-one'])
}}
"
    );
    let err = try_compile(&src).expect_err("must not compile");
    let arity = arity_mismatches(&err.errors);
    assert!(
        !arity.is_empty(),
        "expected FormatSpecifierArityMismatch; got {:?}",
        err.errors
    );
    if let CompilationErrorKind::FormatSpecifierArityMismatch { specifiers, args } = &arity[0].kind
    {
        assert_eq!(*specifiers, 2);
        assert_eq!(*args, 1);
    } else {
        unreachable!()
    }
}

#[test]
fn arity_extra_args_errors() {
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%s'->format(['x', 'y'])
}}
"
    );
    let err = try_compile(&src).expect_err("must not compile");
    let arity = arity_mismatches(&err.errors);
    assert!(
        !arity.is_empty(),
        "expected FormatSpecifierArityMismatch; got {:?}",
        err.errors
    );
}

// ---------------------------------------------------------------------------
// 4. Specifier-specific type checks
// ---------------------------------------------------------------------------

#[test]
fn percent_d_on_float_errors() {
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%d'->format([3.14])
}}
"
    );
    let err = try_compile(&src).expect_err("must not compile");
    let hits = type_mismatches(&err.errors);
    assert!(
        !hits.is_empty(),
        "%d on Float must error; got {:?}",
        err.errors
    );
}

#[test]
fn percent_f_on_integer_errors() {
    // Java parity: `%f` requires Float strictly — Integer doesn't
    // satisfy. (`Format.java:156` uses `instanceOf(arg, M3Paths.Float)`
    // — Integer is NOT a Float in Pure.)
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%f'->format([42])
}}
"
    );
    let err = try_compile(&src).expect_err("must not compile");
    let hits = type_mismatches(&err.errors);
    assert!(
        !hits.is_empty(),
        "%f on Integer must error (Java parity); got {:?}",
        err.errors
    );
}

// ---------------------------------------------------------------------------
// 5. `%%` doesn't consume an arg slot
// ---------------------------------------------------------------------------

#[test]
fn double_percent_does_not_consume_arg() {
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(): String[1] {{
    '%% %s'->format(['only-one-slot'])
}}
"
    );
    try_compile(&src).expect("%% must not consume a slot");
}

// ---------------------------------------------------------------------------
// 6. Skip cases — dynamic format string / dynamic args list
// ---------------------------------------------------------------------------

#[test]
fn non_literal_format_string_skipped() {
    // Dynamic format string — the validator can't statically know the
    // specifiers, so it silently defers to runtime.
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(fmt: String[1]): String[1] {{
    $fmt->format([1, 'x'])
}}
"
    );
    try_compile(&src).expect("non-literal format string must skip the validator");
}

#[test]
fn non_literal_args_collection_skipped() {
    // Dynamic args — the validator can't statically inspect the
    // element types, so it silently defers to runtime.
    let src = format!(
        "{FORMAT_DECL}

###Pure
import meta::pure::functions::string::*;

function abc::demo(args: Any[*]): String[1] {{
    '%d'->format($args)
}}
"
    );
    try_compile(&src).expect("non-literal args must skip the validator");
}

// ---------------------------------------------------------------------------
// 7. Format not loaded → validator is a no-op
// ---------------------------------------------------------------------------

#[test]
fn no_format_declaration_no_op() {
    // Compile fixture without the `format` declaration — the
    // validator can't find a target ElementId and silently returns.
    // (Verifies the bootstrap-only test fixtures elsewhere stay
    // unaffected even if they coincidentally write format-like
    // string-template helpers.)
    let src = r"
###Pure
Class abc::Holder { value: Integer[1]; }
";
    try_compile(src).expect("bootstrap-only fixture must compile cleanly");
}
