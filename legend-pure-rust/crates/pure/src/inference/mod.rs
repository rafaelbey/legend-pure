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

//! Type-inference encapsulation. Java-parity port of
//! `org.finos.legend.pure.m3.compiler.postprocessing.inference.*`
//! plus the `GenericType.makeTypeArgumentAsConcreteAsPossible` substitution
//! entry point.
//!
//! # Why this module exists
//!
//! Generic-binding state was scattered across `resolve.rs`, `infer.rs`, and
//! `lower.rs`. `bind_type` was called from three unrelated sites with no
//! unified context, and `infer_function_call` accumulated 8 concerns in a
//! single ~280-line block. This made the residual `eval`-arg-validation
//! gap (BACKLOG "Authoritative vs constraint bindings") hard to reason
//! about — two structural fixes regressed platform fold-style chains.
//!
//! Java's `TypeInferenceContext` carries the answer: a stack of
//! per-function-call binding states with a `merge` flag on `register()`
//! that distinguishes the two dispatch branches
//! (`potentiallyUpdateTypeInferenceContextUsingFunctionSignature` vs
//! `updateTypeInferenceContextUsingFunctionSignature`). This module
//! ports the algorithm incrementally.
//!
//! # Three seams (mirrors `validate.rs`)
//!
//! | Seam | Where | What |
//! |---|---|---|
//! | **Binding** | [`context::GenericBindings`] | The substitution-binding carrier. Java analog: `GenericTypeWithXArguments`. |
//! | **Substitution** | [`make_concrete`][m] (planned) | Single entry point for "replace Generic / Variable in this TypeExpr / Multiplicity". Java analog: `GenericType.makeTypeArgumentAsConcreteAsPossible`. |
//! | **Function-call processing** | [`processor::FunctionCallProcessor`][p] (planned) | Phase split: first-pass arg inference → branch-select → register authoritative / constraint → reverse-match return → finalize. Java analog: `FunctionExpressionProcessor.process`. |
//!
//! [m]: context::GenericBindings::make_concrete
//! [p]: ../inference/processor/struct.FunctionCallProcessor.html
//!
//! Plan: `~/.claude/plans/do-we-have-enought-quiet-swing.md`. This is
//! Step 2 + 3a of the migration; later steps will populate
//! `make_concrete`, the `TypeInferenceContext` stack, and the
//! `FunctionCallProcessor` phases.

pub(crate) mod context;
#[allow(dead_code)] // placeholder docs; Step 3d-cont fills in
pub(crate) mod lambda;
#[allow(dead_code)] // placeholder docs; Step 3d-cont fills in
pub(crate) mod processor;

pub(crate) use context::GenericBindings;
