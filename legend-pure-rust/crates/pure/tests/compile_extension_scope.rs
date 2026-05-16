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

//! End-to-end proof that `CompileExtensionScope` survives the
//! declare → define_signatures → define_bodies → validate flow when
//! threaded through `compile_with_extensions`.
//!
//! This is the Phase 3-FULL substrate — once Diagram / Mapping /
//! Relational migrate their `RefCell`-on-struct state to the scope,
//! they become stateless unit structs that can self-register via
//! `#[distributed_slice]`. This test locks in the invariant that
//! data written in one pass is visible to a later pass.

use legend_pure_parser_pure::extension::{
    CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx,
};
use legend_pure_parser_pure::pipeline::compile_with_extensions;

/// Per-compile state a stateless `CompilerExtension` stashes in the
/// scope. Plain `Vec`s satisfy `Send + Sync` automatically — the
/// shape Diagram / Mapping / Relational will adopt.
#[derive(Default)]
struct ScopeFlowState {
    declared: Vec<String>,
    defined_signatures: Vec<String>,
    defined_bodies: Vec<String>,
}

/// Captures what each pass saw in the scope via an external Mutex
/// observer — Mutex makes the test struct `Send + Sync` for tests
/// that need to inspect post-validate.
#[test]
fn scope_survives_declare_to_validate() {
    let observed: std::sync::Mutex<Option<(usize, usize, usize)>> = std::sync::Mutex::new(None);

    struct CapturingExt<'a> {
        observed: &'a std::sync::Mutex<Option<(usize, usize, usize)>>,
    }
    impl<'a> CompilerExtension for CapturingExt<'a> {
        fn name(&self) -> &'static str {
            "capturing-scope-flow-test"
        }
        fn declare(&self, ctx: &mut DeclareCtx<'_>) {
            ctx.scope
                .as_deref_mut()
                .expect("scope wired")
                .get_or_default::<ScopeFlowState>()
                .declared
                .push("d".into());
        }
        fn define_signatures(&self, ctx: &mut DefineCtx<'_>) {
            ctx.scope
                .as_deref_mut()
                .expect("scope wired")
                .get_or_default::<ScopeFlowState>()
                .defined_signatures
                .push("s".into());
        }
        fn define_bodies(&self, ctx: &mut DefineCtx<'_>) {
            ctx.scope
                .as_deref_mut()
                .expect("scope wired")
                .get_or_default::<ScopeFlowState>()
                .defined_bodies
                .push("b".into());
        }
        fn validate(&self, ctx: &mut ValidateCtx<'_>) {
            let scope = ctx.scope.expect("scope wired");
            let state = scope
                .get::<ScopeFlowState>()
                .expect("prior passes wrote to scope");
            *self.observed.lock().unwrap() = Some((
                state.declared.len(),
                state.defined_signatures.len(),
                state.defined_bodies.len(),
            ));
        }
    }
    let capturing = CapturingExt {
        observed: &observed,
    };
    let _ = compile_with_extensions(&[], &[], &[&capturing]);

    let snapshot = *observed.lock().unwrap();
    assert_eq!(
        snapshot,
        Some((1, 1, 1)),
        "validate must see declare/define_signatures/define_bodies marks accumulated in scope",
    );
}

#[test]
fn scope_is_fresh_per_compile() {
    // Two back-to-back compiles must NOT share scope state — each
    // compile_with_extensions allocates a fresh model, which holds a
    // fresh compile_scope.
    let observed_first: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);
    let observed_second: std::sync::Mutex<Option<usize>> = std::sync::Mutex::new(None);

    struct CountingExt<'a> {
        observed: &'a std::sync::Mutex<Option<usize>>,
    }
    impl<'a> CompilerExtension for CountingExt<'a> {
        fn name(&self) -> &'static str {
            "counting-scope-fresh-test"
        }
        fn declare(&self, ctx: &mut DeclareCtx<'_>) {
            ctx.scope
                .as_deref_mut()
                .expect("scope wired")
                .get_or_default::<ScopeFlowState>()
                .declared
                .push("d".into());
        }
        fn validate(&self, ctx: &mut ValidateCtx<'_>) {
            let scope = ctx.scope.expect("scope wired");
            let state = scope.get::<ScopeFlowState>().expect("declare ran");
            *self.observed.lock().unwrap() = Some(state.declared.len());
        }
    }
    let ext_a = CountingExt {
        observed: &observed_first,
    };
    let _ = compile_with_extensions(&[], &[], &[&ext_a]);

    let ext_b = CountingExt {
        observed: &observed_second,
    };
    let _ = compile_with_extensions(&[], &[], &[&ext_b]);

    assert_eq!(*observed_first.lock().unwrap(), Some(1));
    assert_eq!(
        *observed_second.lock().unwrap(),
        Some(1),
        "second compile must not inherit first's scope state",
    );
}

#[test]
fn ctx_built_by_hand_has_none_scope() {
    // Quick smoke for the Option<&mut> shape — hand-built ctx (e.g.
    // a test that doesn't go through the pipeline) sets `scope: None`
    // and extensions that branch on `is_some()` skip cleanly.
    let mut model = legend_pure_parser_pure::model::PureModel::new();
    let mut errors = Vec::new();
    let ctx = DeclareCtx {
        source_files: &[],
        model: &mut model,
        auto_imports: &[],
        errors: &mut errors,
        scope: None,
    };
    assert!(ctx.scope.is_none(), "hand-built ctx defaults to no scope");
}
