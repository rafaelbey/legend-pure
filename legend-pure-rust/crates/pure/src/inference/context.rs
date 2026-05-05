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

//! Binding state carried through generic-substitution call sites.
//!
//! Today this is a flat `(types, mults)` map pair. The Java analog is
//! `org.finos.legend.pure.m3.navigation.generictype.GenericTypeWithXArguments`,
//! which pairs a parametric type with the values bound for its parameters.
//! Java additionally maintains a stack of these via `TypeInferenceContext`
//! (with `parent`, `tops`, `scope`, per-state `ahead` flags). We're
//! porting incrementally — the stack lands in a follow-up step
//! (3c of the plan) once the surrounding code routes binding /
//! substitution through this seam.

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::types::{Multiplicity, TypeExpr};

/// Bindings from generic parameter names (`T`, `m`, …) to the concrete
/// types/multiplicities inferred at a specific call site.
///
/// **Currently** a flat pair of maps; relocated here from
/// `crate::resolve::GenericBindings` so the binding/substitution seam
/// has a stable home. The Java analog is `GenericTypeWithXArguments`
/// (which additionally carries the parametric type the arguments
/// belong to). Future work threads a `TypeInferenceContext` stack
/// around this so authoritative-vs-constraint bindings are
/// distinguishable; at that point this struct may grow a `parent` link
/// or be replaced wholesale by a richer state record.
#[derive(Default, Clone)]
pub(crate) struct GenericBindings {
    /// Type-variable bindings: `T` → `TypeExpr::Named { Class, … }`.
    pub ty: HashMap<SmolStr, TypeExpr>,
    /// Multiplicity-variable bindings: `m` → `Multiplicity::PureOne`.
    pub mult: HashMap<SmolStr, Multiplicity>,
}
