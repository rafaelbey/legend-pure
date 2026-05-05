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

//! Function-call processing pipeline. Java analog:
//! `org.finos.legend.pure.m3.compiler.postprocessing.processor.valuespecification.FunctionExpressionProcessor`
//! (`process` at lines 121-265).
//!
//! **Status: placeholder.** The phase logic still lives in
//! `crate::infer::infer_function_call` after Step 3d split it into
//! `process_let_function_call` (Phase 0) and `validate_call_arguments`
//! (Phase 2). When the Java two-branch dispatch lands (Step 3d-cont +
//! Step 3g), the orchestration moves here as a `FunctionCallProcessor`
//! struct with explicit phases:
//!
//! 1. **Phase 0 — `process_let_function_call`**. Side-effect form;
//!    binds variable into scope, returns `Nil[0]`.
//! 2. **Phase 1 — `first_pass_inference`**. Walk each arg bottom-up,
//!    flagging args that didn't converge. Java analog:
//!    `FunctionExpressionProcessor.firstPassTypeInference` (`:796-821`).
//! 3. **Phase 1.5 — `branch_select`**. If any arg failed to converge
//!    in Phase 1 → `RegisterMode::Authoritative` over the succeeded
//!    args (Java's `potentiallyUpdate…`). Otherwise →
//!    `RegisterMode::Constraint` over all args (Java's `update…`).
//! 4. **Phase 2 — `register_*`**. Bind type/multiplicity vars into
//!    [`super::context::TypeInferenceContext`] using the chosen mode.
//! 5. **Phase 3 — `process_lambdas`**. The lambda body second-pass —
//!    extends `var_types` with substituted lambda params, infers each
//!    body's last expression, binds the FunctionType's return-var.
//!    Java analog: `FunctionExpressionProcessor:618-679`. Currently in
//!    `crate::resolve::infer_generic_bindings`'s second-pass loop;
//!    moves into [`super::lambda::LambdaParamFiller`] in Step 3e.
//! 6. **Phase 4 — `validate_call_arguments`**. Per-arg type +
//!    multiplicity check using `make_concrete(param)`. Currently in
//!    `crate::infer::validate_call_arguments`; routed through here in
//!    Step 3g (when strict-mode arg validation flips on).
//! 7. **Phase 5 — `finalize`**. Substitute the function's declared
//!    return signature with the bindings; emit any
//!    `UnresolvedTypeParameter` /
//!    `UnresolvedMultiplicityParameter` diagnostics under strict mode.
//!
//! Plan: `~/.claude/plans/do-we-have-enought-quiet-swing.md`.
