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

use std::collections::{HashMap, HashSet};

use smol_str::SmolStr;

use crate::ids::ElementId;
use crate::types::{Multiplicity, TypeExpr};

/// Bindings from generic parameter names (`T`, `m`, …) to the concrete
/// types/multiplicities inferred at a specific call site.
///
/// `ty` carries the LUB-merged Java-parity view (matches Java's
/// `findBestCommonGenericType` covariant LUB). `ty_auth` carries the
/// subset bound from *structural* `FunctionType` slots — those
/// bindings are invariant in Pure (a function declared
/// `Function<{Integer→X}>` cannot be upcast to
/// `Function<{Number→X}>`), so they're treated as authoritative for
/// the wrong-arg check that matches an arg's type
/// against a frozen-by-FunctionType T.
///
/// Strict-mode catches `eval(intFunc, 'wrong')` because arg 0's
/// `Function<{T→V}>` slot binds T=Integer authoritatively, while
/// arg 1's top-level `T` binds T=String as a constraint. The
/// LUB-merged view widens to Any (Java parity, default mode passes);
/// the auth-only view keeps T=Integer ( rejects). For
/// `compare<T>(a:T, b:T)`-style calls, both bindings are top-level
/// constraint, `ty_auth` is empty, the auth-only view leaves T as
/// `Generic("T")` which `is_type_compatible`'s wildcard arm accepts
/// — so doesn't false-positive on `compare(1, 'a')`.
#[derive(Debug, Default, Clone)]
pub(crate) struct GenericBindings {
    /// Type-variable bindings, LUB-merged across all sources
    /// (Java-parity merged view).
    pub ty: HashMap<SmolStr, TypeExpr>,
    /// Subset of `ty` bound from a structural `FunctionType` slot
    /// (invariant, treated as authoritative). When both auth and
    /// constraint contribute to the same `T`, auth wins absolutely
    /// — `ty_auth[T]` is the auth value, `ty[T]` is the LUB-merged
    /// (Java parity) value. Used by [`make_concrete_type_strict`] to
    /// produce the substituted view.
    pub ty_auth: HashMap<SmolStr, TypeExpr>,
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

    /// Substitute using *only* authoritative bindings (those bound from
    /// a structural `FunctionType` slot). Constraint-only `Generic(T)`
    /// references survive as `Generic("T")` in the result —
    /// `is_type_compatible`'s `_ => return true` arm then accepts any
    /// arg type against them, matching Java's LUB-to-supertype semantics
    /// for top-level Generic params without LUB-widening auth slots.
    ///
    /// Used by the arg-type check in
    /// `validate_call_arguments`. The catch surface: any `T` whose value
    /// was set authoritatively by another arg's structural FunctionType
    /// slot (most prominently `eval`'s `func:Function<{T[n]→V[m]}>`
    /// signature). The non-catch surface: `compare<T>(a:T,b:T)`-style
    /// calls where both args are top-level Generic-typed; `ty_auth` is
    /// empty for those, the substituted param remains `Generic("T")`,
    /// silently accepts.
    ///
    /// Auth-Generic-fallback: when `ty_auth[name]` is itself a
    /// `Generic(...)` placeholder (e.g. `$f`'s structural slot is
    /// `Function<{->Z[y]}>` where `Z` is the *caller's* outer generic —
    /// the auth value carries Z's name verbatim, not a concrete type),
    /// the constraint LUB in `self.ty` typically has more information
    /// (e.g. the lambda body's V resolved to `ValueSpecification`).
    /// Prefer the constraint side then. This unblocks
    /// `match.pure:185 $z.genericType.rawType->toOne()` where `$z` is a
    /// let-bound result of `$f->eval(|…->deactivate())` — the auth
    /// V=Generic("Z") would otherwise freeze the chain receiver as
    /// Generic and break property-access.
    #[must_use]
    pub fn make_concrete_type_strict(&self, ty: &TypeExpr) -> TypeExpr {
        let merged: HashMap<SmolStr, TypeExpr> = self
            .ty_auth
            .iter()
            .map(|(k, v)| {
                let value = if matches!(v, TypeExpr::Generic(_))
                    && let Some(constraint) = self.ty.get(k)
                    && !matches!(constraint, TypeExpr::Generic(_))
                {
                    constraint.clone()
                } else {
                    v.clone()
                };
                (k.clone(), value)
            })
            .collect();
        crate::resolve::substitute_type(ty, &merged)
    }
}

// ---------------------------------------------------------------------------
// TypeInferenceContext — Java port skeleton
// ---------------------------------------------------------------------------

/// Java's `register(merge: bool)` flag distinguishing the two
/// dispatch branches.
///
/// In Java's
/// `FunctionExpressionProcessor.java:121-265`, the orchestrator
/// chooses one of two paths after `firstPassTypeInference` walks each
/// argument:
///
/// - [`RegisterMode::Authoritative`] — `merge=false`. Used by
/// `potentiallyUpdateTypeInferenceContextUsingFunctionSignature`
/// (`:567-584`) when at least one argument failed to converge in
/// the first pass. Only the *succeeded* arg pairs are walked, and
/// each binding is treated as the canonical value for the type
/// parameter — an existing non-concrete entry can be replaced
/// outright by a concrete incoming value.
/// - [`RegisterMode::Constraint`] — `merge=true`. Used by
/// `updateTypeInferenceContextUsingFunctionSignature` (`:586-594`)
/// when every argument converged. All pairs are walked
/// left-to-right and each registration LUB-merges with the
/// existing binding (Java acknowledges a known-broken
/// widen-to-Any-then-drop case at `register():474-478`).
///
/// Today `bind_type` always behaves like `Constraint` (LUB-merge
/// unconditionally). Step 3d of the plan will route the two-branch
/// dispatch through the new [`TypeInferenceContext`] and select the
/// mode based on first-pass convergence, exactly as Java does.
#[allow(dead_code)] // wired in Step 3d
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterMode {
    /// Authoritative source for this binding (Java `merge=false`).
    Authoritative,
    /// Constraint slot LUB-merging with the existing binding (Java
    /// `merge=true`).
    Constraint,
}

/// Skeleton port of Java's
/// `org.finos.legend.pure.m3.compiler.postprocessing.inference.TypeInferenceContext`.
///
/// **Today**: a single-frame wrapper around [`GenericBindings`] with
/// stack-frame metadata (`parent`, `scope`, `tops`) and a
/// `register*`-shaped API. **Not yet wired** as the inner carrier of
/// `infer_generic_bindings` — Step 3d of the plan does that. This
/// commit lands the structure so subsequent steps have a stable home
/// for the merge-aware register semantics, the lambda-body second-
/// pass, and the arg-type checks.
///
/// Java fields mirrored:
/// - `id` — sequential identifier; useful for the
/// `TypeInferenceObserver` trace.
/// - `parent` — pointer to the enclosing context. Recursive generic
/// functions like `getAllTypeGeneralisations` rely on the parent
/// chain to resolve a type parameter that's bound in an outer
/// call.
/// - `scope` — the M3 element being processed (function, class).
/// Surfaces in error messages.
/// - `bindings` — the current frame's `(types, mults)` map pair. Two
/// per-state Java fields (`ahead`, `ahead_consumed`) are *not*
/// ported yet — they cover the deferred-lambda-body case and land
/// together with [`crate::inference::lambda::LambdaParamFiller`]
/// (Step 3e).
/// - `tops` — set of type-parameter names that are *top-level* in
/// this context (declared on the M3 element being processed). A
/// `Generic(name)` whose name is in `tops` MUST stay generic
/// through `make_concrete` — substituting it would conflate
/// distinct outer-scope parameters with bindings collected at this
/// call site.
///
/// The struct deliberately uses owned values (no `Rc`/`Weak`) until
/// Step 3d shows whether shared ownership is required. Most call
/// sites in our pipeline are single-threaded and tree-structured, so
/// `Box<...>` for the parent link should suffice.
#[allow(dead_code)] // wired in Step 3d
#[derive(Debug, Clone, Default)]
pub struct TypeInferenceContext {
    /// Sequential id (cheap diagnostic + observer trace anchor).
    pub id: u32,
    /// The element being processed — surfaces in error messages.
    pub scope: Option<ElementId>,
    /// Set of type-parameter names declared on this scope's element.
    /// `make_concrete` does not substitute these; they are the
    /// surrounding generic's own parameters.
    pub tops: HashSet<SmolStr>,
    /// The current frame's bindings.
    pub bindings: GenericBindings,
    /// Enclosing context, if any. Recursive generic functions look up
    /// unresolved bindings here (Java's
    /// `find_parent_for_operation`).
    pub parent: Option<Box<TypeInferenceContext>>,
}

#[allow(dead_code)] // wired in Step 3d
impl TypeInferenceContext {
    /// Construct a fresh top-level context for `scope` with the given
    /// top-level type-parameter names.
    #[must_use]
    pub fn root(scope: Option<ElementId>, tops: HashSet<SmolStr>) -> Self {
        Self {
            id: 0,
            scope,
            tops,
            bindings: GenericBindings::default(),
            parent: None,
        }
    }

    /// Push a child frame onto the stack. The returned context's
    /// `parent` points at the previous one. Java analog:
    /// `TypeInferenceContext.addStateForCollectionElement` and the
    /// new-state cases in `FunctionExpressionProcessor`.
    #[must_use]
    pub fn enter_child(self, scope: Option<ElementId>, tops: HashSet<SmolStr>) -> Self {
        let next_id = self.id + 1;
        Self {
            id: next_id,
            scope,
            tops,
            bindings: GenericBindings::default(),
            parent: Some(Box::new(self)),
        }
    }

    /// Register a type-variable binding with the given mode.
    ///
    /// **Authoritative**: a fresh binding is inserted; an existing
    /// one is REPLACED (mirrors Java's `merge=false` branch's late
    /// "non-concrete + concrete → propagate upward" rule).
    ///
    /// **Constraint**: a fresh binding is inserted; an existing one
    /// LUB-merges with the new value via
    /// `crate::resolve::bind_type`'s existing rule. This is what
    /// `bind_type` currently does unconditionally.
    ///
    /// Today both modes delegate to `bind_type` — Step 3d will split
    /// out the authoritative path properly. Surfacing the API now
    /// lets call sites declare their intent ahead of the algorithm
    /// change.
    pub fn register_type(
        &mut self,
        name: &SmolStr,
        value: TypeExpr,
        mode: RegisterMode,
        model: &crate::model::PureModel,
    ) {
        match mode {
            RegisterMode::Authoritative => {
                self.bindings.ty.insert(name.clone(), value);
            }
            RegisterMode::Constraint => {
                let template = TypeExpr::Generic(name.clone());
                crate::resolve::bind_type(&template, &value, &mut self.bindings.ty, model);
            }
        }
    }

    /// Register a multiplicity-variable binding with the given mode.
    pub fn register_mult(&mut self, name: &SmolStr, value: Multiplicity, mode: RegisterMode) {
        match mode {
            RegisterMode::Authoritative => {
                self.bindings.mult.insert(name.clone(), value);
            }
            RegisterMode::Constraint => {
                use std::collections::hash_map::Entry;
                let promotion: Option<(SmolStr, Multiplicity)> =
                    match self.bindings.mult.entry(name.clone()) {
                        Entry::Vacant(e) => {
                            e.insert(value);
                            None
                        }
                        Entry::Occupied(mut e) => {
                            let prev = e.get().clone();
                            let lub = crate::resolve::mult_lub(&prev, &value);
                            *e.get_mut() = lub.clone();
                            // See `bind_mult_with_mode`'s
                            // promote-and-propagate doc-comment.
                            // When a function-ref's lifted
                            // FunctionType binds eval's `n_eval` and
                            // `m_eval` BOTH to the same callee
                            // `Variable("m")`, the next arg's
                            // top-level mult bind promotes `n_eval`
                            // here and we have to propagate to
                            // `m_eval` so eval's return-mult
                            // resolves.
                            if matches!(prev, Multiplicity::Variable(_))
                                && !matches!(lub, Multiplicity::Variable(_))
                            {
                                if let Multiplicity::Variable(prev_name) = prev {
                                    Some((prev_name, lub))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                    };
                if let Some((prev_var, concrete)) = promotion {
                    for v in self.bindings.mult.values_mut() {
                        if let Multiplicity::Variable(n) = v
                            && n == &prev_var
                        {
                            *v = concrete.clone();
                        }
                    }
                }
            }
        }
    }

    /// `true` when `name` is a top-level type parameter of this
    /// context — meaning `make_concrete` should NOT substitute it.
    /// Java's `TypeInferenceContext.isTop`.
    #[must_use]
    pub fn is_top(&self, name: &SmolStr) -> bool {
        self.tops.contains(name)
    }

    /// Walk up the parent chain looking for a binding for `name`.
    /// Returns `None` if no ancestor has bound it. Java's
    /// `find_parent_for_operation` shape, simplified.
    #[must_use]
    pub fn lookup_type_in_parents(&self, name: &SmolStr) -> Option<&TypeExpr> {
        let mut cur = self.parent.as_deref();
        while let Some(ctx) = cur {
            if let Some(v) = ctx.bindings.ty.get(name) {
                return Some(v);
            }
            cur = ctx.parent.as_deref();
        }
        None
    }

    /// Walk up the parent chain looking for a multiplicity binding.
    #[must_use]
    pub fn lookup_mult_in_parents(&self, name: &SmolStr) -> Option<&Multiplicity> {
        let mut cur = self.parent.as_deref();
        while let Some(ctx) = cur {
            if let Some(v) = ctx.bindings.mult.get(name) {
                return Some(v);
            }
            cur = ctx.parent.as_deref();
        }
        None
    }
}
