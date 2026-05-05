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

//! End-to-end coverage for stereotype / tagged-value profile
//! resolution.
//!
//! These tests exercise the *full* parse → compile pipeline on
//! fixtures that previously tripped a panic in
//! `validate.rs::validate_stereotypes` when an unresolvable
//! stereotype profile name accidentally collided with a package
//! name in scope.
//!
//! The fix lives in `resolve.rs::resolve_stereotypes` (and the
//! parallel tagged-value path): when the generic
//! `resolve_element_ptr` falls back to a Package — legitimate for
//! expression positions like `elementPath(meta)`, never legitimate
//! for a stereotype profile — the stereotype resolver intercepts
//! that, re-emits the `UnresolvedElement` error the fallback
//! popped, and skips the StereotypeRef.

use legend_pure_parser_parser::parse;
use legend_pure_parser_pure::error::CompilationErrorKind;
use legend_pure_parser_pure::pipeline;

/// `<<test.Test>>` references a profile named `test`. Without the
/// platform's `meta::pure::profiles::test` Profile loaded, the
/// resolver used to fall back to *the function's own enclosing
/// `test` package*, hand a `Package` ID to the validator, and panic
/// at `model.get_node` (which rejects Package IDs).
///
/// Post-fix expectations:
///   - Compile completes without panicking.
///   - At least one `UnresolvedElement` error covers the missing
///     profile.
///   - No misleading "is not a Profile" cascade error is added on
///     top.
#[test]
fn unresolvable_stereotype_profile_does_not_panic() {
    let source = "\
function <<test.Test>> test::myCheck(): Boolean[1]
{
  true
}
";
    let parsed = parse(source, "fixture.pure").expect("fixture must parse");

    // No platform loaded, no auto-imports — exactly the conditions
    // that produced the original panic.
    let result = pipeline::compile(&[parsed], &[]);
    let partial =
        result.expect_err("compile is expected to surface errors (the profile is not in scope)");

    // Must include at least one UnresolvedElement error.
    let unresolved: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::UnresolvedElement { .. }))
        .collect();
    assert!(
        !unresolved.is_empty(),
        "expected at least one UnresolvedElement error for the missing 'test' profile, got: {:?}",
        partial.errors
    );

    // Must NOT include the legacy "is not a Profile" cascade — that
    // message only fires when the resolver successfully resolved to
    // a real non-Profile element (e.g. a user typo'd a Class name).
    let is_not_profile: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| e.message.contains("is not a Profile"))
        .collect();
    assert!(
        is_not_profile.is_empty(),
        "did not expect a 'is not a Profile' cascade after the resolver fix, got: {:?}",
        is_not_profile
            .iter()
            .map(|e| &e.message)
            .collect::<Vec<_>>()
    );
}

/// Same shape, but with a tagged value (parallel resolver path).
#[test]
fn unresolvable_tagged_value_profile_does_not_panic() {
    let source = "\
function {test.note = 'hello'} test::myCheck(): Boolean[1]
{
  true
}
";
    let parsed = parse(source, "fixture.pure").expect("fixture must parse");

    let result = pipeline::compile(&[parsed], &[]);
    let partial = result.expect_err("expected compilation errors for the missing tag profile");

    let unresolved: Vec<_> = partial
        .errors
        .iter()
        .filter(|e| matches!(e.kind, CompilationErrorKind::UnresolvedElement { .. }))
        .collect();
    assert!(
        !unresolved.is_empty(),
        "expected at least one UnresolvedElement error, got: {:?}",
        partial.errors
    );
}
