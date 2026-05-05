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

//! Lambda-parameter type filling against an enclosing
//! `Function<{T->X}>` expectation. Java analog:
//! `org.finos.legend.pure.m3.compiler.postprocessing.inference.TypeInference`
//! at lines 108-148
//! (`processParamTypesOfLambdaUsedAsAFunctionExpressionParamValue`).
//!
//! **Status: placeholder.** The two existing implementations stay in
//! their current homes until Step 3e of the plan consolidates them
//! here:
//!
//! - **Lower-side**: `crate::lower::compute_lambda_param_expectations`
//!   (`lower.rs:945-1021`). Reads the candidate's parameter type, finds
//!   the embedded `FunctionType` slot, substitutes any already-known
//!   bindings into its parameter types, and passes the resulting
//!   `(TypeExpr, Multiplicity)` tuples to
//!   `lower_lambda_with_expected_types`. This is what makes
//!   `[1,2,3]->filter(x | $x->plus(1))` type `x` as `Integer[1]`.
//!
//! - **Resolve-side**: `crate::resolve::infer_generic_bindings`'s
//!   second pass (`resolve.rs:2014-2043`) and the
//!   `bind_from_lambda_body` helper (`resolve.rs:2055-2117`). Walks
//!   each lambda arg (or every Lambda inside a Collection arg), infers
//!   the lambda body's last expression with the lambda's params in
//!   scope, and binds the FunctionType's return-type variable from
//!   that body type. This is what closed `getAllTypeGeneralisations`'s
//!   inference precision and the `match([λ1, λ2])` pattern.
//!
//! Both paths solve adjacent problems (typing the lambda's *params*
//! at lower time vs. binding the FunctionType's *return* at infer
//! time) but should cohabit in this module so the lambda-parameter
//! algorithm has one home — that's the Step 3e consolidation.
//!
//! ## Java's deferred-lambda case
//!
//! Java's `processParamTypes…` returns `true` ("not done") when the
//! template's parameter types aren't yet concrete. The orchestrator
//! then re-processes the lambda after later args have populated
//! bindings. We don't yet implement this deferral — when our second
//! pass can't bind a lambda's params, the param falls back to
//! `TypeExpr::Unresolved`. Tracked in BACKLOG: "Per-call-site
//! specialisation for let-bound lambdas".
//!
//! Plan: `~/.claude/plans/do-we-have-enought-quiet-swing.md`.
