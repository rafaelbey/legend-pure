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

impl GenericBindings {
    /// Substitute the type-variable bindings into `ty`. Replaces every
    /// `TypeExpr::Generic(name)` whose `name` is in `self.ty`; recurses
    /// through `Named { type_arguments }`, `FunctionType { parameters,
    /// return_type }`, and `AlgebraUnion`.
    ///
    /// This is the single entry point for "make this `TypeExpr` as
    /// concrete as possible given the bindings I've collected." Mirrors
    /// Java's `GenericType.makeTypeArgumentAsConcreteAsPossible`
    /// (`navigation/generictype/GenericType.java:125-171`).
    ///
    /// Today it delegates to `crate::resolve::substitute_type`. As
    /// `TypeInferenceContext` grows (Step 3c), this method will pick up
    /// parent-context lookup so a child call site's missing binding can
    /// resolve through the enclosing function's bound parameters.
    #[must_use]
    pub fn make_concrete_type(&self, ty: &TypeExpr) -> TypeExpr {
        crate::resolve::substitute_type(ty, &self.ty)
    }

    /// Substitute the multiplicity-variable bindings into `m`. Replaces
    /// every `Multiplicity::Variable(name)` whose `name` is in
    /// `self.mult`. Mirrors the multiplicity-side of
    /// `GenericType.makeTypeArgumentAsConcreteAsPossible`.
    ///
    /// Today it delegates to `crate::resolve::substitute_mult`.
    #[must_use]
    pub fn make_concrete_mult(&self, m: &Multiplicity) -> Multiplicity {
        crate::resolve::substitute_mult(m, &self.mult)
    }
}

/// Walk a (presumed already-substituted) `TypeExpr` and collect every
/// `Generic(name)` that survived. A non-empty result is the Java
/// parity signal for `TypeInference.java:87-89` ("The type parameter
/// X was not resolved") — emitted when a callee declares `<T>` but
/// no call-site argument supplied a value for `T`.
///
/// Recurses through `Named { type_arguments }`, `FunctionType`,
/// `AlgebraUnion`, and `Relation` columns so a parametric position
/// (`Class<T>`, `Function<{T->X}>`) doesn't hide an unresolved
/// generic.
#[must_use]
pub fn unresolved_type_params(ty: &TypeExpr) -> Vec<SmolStr> {
    let mut out = Vec::new();
    walk_type_for_unresolved(ty, &mut out);
    out
}

fn walk_type_for_unresolved(ty: &TypeExpr, out: &mut Vec<SmolStr>) {
    match ty {
        TypeExpr::Generic(name) => {
            if !out.contains(name) {
                out.push(name.clone());
            }
        }
        TypeExpr::Named { type_arguments, .. } => {
            for ta in type_arguments {
                walk_type_for_unresolved(ta, out);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            ..
        } => {
            for (p, _) in parameters {
                walk_type_for_unresolved(p, out);
            }
            walk_type_for_unresolved(return_type, out);
        }
        TypeExpr::AlgebraUnion(a, b) => {
            walk_type_for_unresolved(a, out);
            walk_type_for_unresolved(b, out);
        }
        TypeExpr::Relation(cols) => {
            for c in cols {
                walk_type_for_unresolved(&c.type_expr, out);
            }
        }
        TypeExpr::Unresolved => {}
    }
}

/// Returns the variable name if `m` is `Multiplicity::Variable`,
/// otherwise `None`. Used at the same site as
/// [`unresolved_type_params`] to detect unresolved multiplicity
/// generics — Java parity for `TypeInference.java:102` ("The
/// multiplicity parameter X was not resolved").
#[must_use]
pub fn unresolved_mult_param(m: &Multiplicity) -> Option<SmolStr> {
    match m {
        Multiplicity::Variable(name) => Some(name.clone()),
        _ => None,
    }
}
