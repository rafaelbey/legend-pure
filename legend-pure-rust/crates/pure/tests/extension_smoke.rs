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

//! Smoke test: a registered `CompilerExtension` receives `declare`,
//! `define_signatures`, `define_bodies`, and `validate` calls in the
//! expected pass order, without disturbing the M3 compile.
//!
//! Locks the contract for the M2 DSL plug-in path. If a future
//! pipeline refactor accidentally drops the extension dispatch, this
//! test flips red.

use std::cell::RefCell;

use legend_pure_parser_pure::extension::{CompilerExtension, DeclareCtx, DefineCtx, ValidateCtx};

#[derive(Default)]
struct MockExtension {
    /// Records the ordered list of hook names invoked.
    calls: RefCell<Vec<&'static str>>,
}

impl CompilerExtension for MockExtension {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn declare(&self, _ctx: &mut DeclareCtx<'_>) {
        self.calls.borrow_mut().push("declare");
    }
    fn define_signatures(&self, _ctx: &mut DefineCtx<'_>) {
        self.calls.borrow_mut().push("define_signatures");
    }
    fn define_bodies(&self, _ctx: &mut DefineCtx<'_>) {
        self.calls.borrow_mut().push("define_bodies");
    }
    fn validate(&self, _ctx: &mut ValidateCtx<'_>) {
        self.calls.borrow_mut().push("validate");
    }
}

#[test]
fn extension_hooks_invoked_in_pass_order() {
    let sf =
        legend_pure_parser_parser::parse("Class smoke::Person { name: String[1]; }", "smoke.pure")
            .expect("parse");

    let mock = MockExtension::default();
    let extensions: [&dyn CompilerExtension; 1] = [&mock];

    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(
        std::slice::from_ref(&sf),
        &[],
        &extensions,
    );
    // Compilation may produce errors against the bare bootstrap
    // (no platform loaded), but extension hooks must still fire.
    let _ = result;

    let captured = mock.calls.borrow().clone();
    assert_eq!(
        captured,
        vec!["declare", "define_signatures", "define_bodies", "validate"],
        "extension hooks invoked in wrong order or some skipped",
    );
}

#[test]
fn zero_extensions_unchanged_compile() {
    // The plain `compile()` entry must remain a thin wrapper that
    // produces the same model as `compile_with_extensions(.., &[])`.
    let sf =
        legend_pure_parser_parser::parse("Class smoke::Person { name: String[1]; }", "smoke.pure")
            .expect("parse");

    let plain = legend_pure_parser_pure::pipeline::compile(std::slice::from_ref(&sf), &[]);
    let with_ext = legend_pure_parser_pure::pipeline::compile_with_extensions(
        std::slice::from_ref(&sf),
        &[],
        &[],
    );

    // Both should agree on Ok/Err; element counts must match.
    let plain_n = match &plain {
        Ok(m) => m
            .chunks
            .iter()
            .map(|c| c.nodes.len() as usize)
            .sum::<usize>(),
        Err(p) => p
            .model
            .chunks
            .iter()
            .map(|c| c.nodes.len() as usize)
            .sum::<usize>(),
    };
    let with_ext_n = match &with_ext {
        Ok(m) => m
            .chunks
            .iter()
            .map(|c| c.nodes.len() as usize)
            .sum::<usize>(),
        Err(p) => p
            .model
            .chunks
            .iter()
            .map(|c| c.nodes.len() as usize)
            .sum::<usize>(),
    };
    assert_eq!(
        plain_n, with_ext_n,
        "compile vs compile_with_extensions(_, _, &[]) produced different element counts",
    );
}
