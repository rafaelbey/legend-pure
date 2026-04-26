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

//! Heap-aware structural equality — the shared engine behind Pure `==`
//! (`equal`) and `<<equality.Key>>`-keyed map lookups.
//!
//! Pure's `equal` for object references is **not** identity. When a class
//! annotates one or more properties with the `<<equality.Key>>` stereotype
//! from `meta::pure::profiles::equality`, two distinct heap instances of
//! that class compare equal iff every annotated property compares equal
//! (recursively). Classes without any `<<equality.Key>>` property keep
//! identity semantics — `Value::Object(a) == Value::Object(b)` iff
//! `a == b`.
//!
//! `eq` / `is` stay strictly identity-based — those Pure natives do
//! *not* route through here.

use legend_pure_parser_pure::annotations::StereotypeRef;
use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use smol_str::SmolStr;

use std::cell::Cell;

use crate::heap::ObjectId;
use crate::native::EvalContextTrait;
use crate::value::Value;

thread_local! {
    /// Recursion depth for `values_equal` / `objects_equal`. Mutual
    /// recursion across collection elements + `<<equality.Key>>`
    /// properties has no inherent bound today; pathological inputs
    /// (deeply nested `List<List<…>>`, an equality-keyed self-
    /// referential class) would blow the Rust stack — default 8 MB
    /// on the main thread but as little as 512 KB on spawned threads.
    /// This stopgap converts the segfault-class bug into a Pure-level
    /// "not equal" answer: if recursion exceeds [`MAX_EQUALITY_DEPTH`]
    /// we return `false` (best-effort: an actually-equal pair past
    /// the limit reads as unequal, but the alternative is process
    /// abort). The proper fix is an explicit work-stack with a
    /// visited-set cycle guard — tracked in BACKLOG.
    static EQ_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Maximum mutual recursion depth before [`values_equal`] / [`objects_equal`]
/// short-circuit to `false`. 1000 frames is well above the deepest legitimate
/// Pure value graph we've seen; pathological cycles or overly nested data
/// hit the limit instead of overflowing the Rust stack.
const MAX_EQUALITY_DEPTH: usize = 1000;

/// RAII guard that bumps [`EQ_DEPTH`] on construction and decrements on
/// drop. Returns `None` once the depth limit is reached so the caller
/// can short-circuit without entering another recursive frame.
struct DepthGuard;

impl DepthGuard {
    fn enter() -> Option<Self> {
        EQ_DEPTH.with(|d| {
            let cur = d.get();
            if cur >= MAX_EQUALITY_DEPTH {
                None
            } else {
                d.set(cur + 1);
                Some(Self)
            }
        })
    }
}

impl Drop for DepthGuard {
    fn drop(&mut self) {
        EQ_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}

/// Structural value equality, heap-aware.
///
/// Primitives compare by value (delegating to `PartialEq for Value`).
/// Collections compare length then element-wise.
/// Heap objects compare via `<<equality.Key>>` stereotype if the class
/// declares any; otherwise fall back to `ObjectId` identity.
#[must_use]
pub fn values_equal(ctx: &dyn EvalContextTrait, a: &Value, b: &Value) -> bool {
    let Some(_guard) = DepthGuard::enter() else {
        return false;
    };
    match (a, b) {
        (Value::Collection(xs), Value::Collection(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| values_equal(ctx, x, y))
        }
        (Value::Object(oa), Value::Object(ob)) => objects_equal(ctx, *oa, *ob),
        // Cross-variant equality for metamodel references — when one
        // side is `Element(eid)` / `Function(Compiled(eid))` and the
        // other is the heap row produced by `bootstrap_metamodel`,
        // both views point at the same `ObjectId`. Without this
        // bridge `assertIs($cls, ^Class<…>(...)->class())` fails
        // because the bare `Element` and the materialised `Object`
        // wouldn't compare equal even though Java treats them as
        // the same `CoreInstance`. Bridges `Element ↔ Object`,
        // `Function(Compiled) ↔ Object`, and the symmetric
        // `Element ↔ Function(Compiled)` (same compiled-element id).
        (
            Value::Element(_) | Value::Function(_) | Value::Object(_),
            Value::Element(_) | Value::Function(_) | Value::Object(_),
        ) => {
            let oa = a.as_object_id(ctx.heap());
            let ob = b.as_object_id(ctx.heap());
            match (oa, ob) {
                (Some(la), Some(lb)) if la == lb => true,
                (Some(la), Some(lb)) => objects_equal(ctx, la, lb),
                _ => a == b,
            }
        }
        // Pure multiplicity coercion: a single-element collection equals
        // its scalar (`[x] == x` holds, `[x, y] == x` does not). The
        // heap's `apply_property_to_instance` returns `Value::from_vec`'d
        // results which collapse 1-element lists to the bare element,
        // while Element-side property access (`eval_function_property`'s
        // `expressionSequence`) explicitly wraps as `Value::Collection`.
        // Without this branch, `$f1.expressionSequence` (Collection) and
        // `$f2.expressionSequence` (bare Function) would compare unequal
        // even though Pure considers them the same value.
        (Value::Collection(xs), other) | (other, Value::Collection(xs)) if xs.len() == 1 => {
            values_equal(ctx, &xs[0], other)
        }
        // Empty collection equals `Unit` — same multiplicity coercion
        // applied at the zero end. `from_vec(Vec::new())` produces
        // `Value::Unit`, so a heap slot with no values vs an explicit
        // empty Collection should still compare equal.
        (Value::Collection(xs), Value::Unit) | (Value::Unit, Value::Collection(xs))
            if xs.is_empty() =>
        {
            true
        }
        _ => a == b,
    }
}

/// Heap-object structural equality. Walks the classifier, gathers every
/// `<<equality.Key>>`-annotated property, and recurses. No keys
/// annotated ⇒ identity equality.
fn objects_equal(ctx: &dyn EvalContextTrait, a: ObjectId, b: ObjectId) -> bool {
    if a == b {
        return true;
    }
    let Ok(a_classifier) = ctx.heap().classifier(a) else {
        return false;
    };
    let Ok(b_classifier) = ctx.heap().classifier(b) else {
        return false;
    };
    if a_classifier != b_classifier {
        return false;
    }
    let class_id = match crate::m3_paths::resolve(ctx.model(), a_classifier) {
        Some(id) => id,
        None => return false, // unknown classifier — can't structurally compare
    };
    let keys = equality_key_properties(ctx.model(), class_id);
    if keys.is_empty() {
        return false; // no <<equality.Key>> → identity semantics (already checked above)
    }
    for prop in &keys {
        let Ok(av) = ctx.heap().get_property_values(a, prop.as_str()) else {
            return false;
        };
        let Ok(bv) = ctx.heap().get_property_values(b, prop.as_str()) else {
            return false;
        };
        if av.len() != bv.len() {
            return false;
        }
        if !av
            .iter()
            .zip(bv.iter())
            .all(|(x, y)| values_equal(ctx, x, y))
        {
            return false;
        }
    }
    true
}

/// Property names annotated with `<<equality.Key>>` on the given Class,
/// in declaration order. Empty ⇒ class uses identity equality.
///
/// Resolves `meta::pure::profiles::equality` on the model once so every
/// stereotype-ref check is a cheap `ElementId` compare rather than a
/// string match. Returns empty if the equality profile isn't loaded.
#[must_use]
pub fn equality_key_properties(model: &PureModel, class_id: ElementId) -> Vec<SmolStr> {
    let Some(equality_profile) = crate::m3_paths::resolve(model, crate::m3_paths::EQUALITY_PROFILE)
    else {
        return Vec::new();
    };
    // Walk class + supertypes (BFS) so an `<<equality.Key>>` declared on
    // `TopClass` is honoured when comparing two `LeftClass extends
    // TopClass` instances. Keep declaration order: subclass properties
    // appear before inherited supertype properties.
    //
    // Override semantics: a subclass that redefines a property — with or
    // without the `<<equality.Key>>` stereotype — wins for that property
    // name. So `OtherBottomClass.sides : SideClass[*]` (no stereotype)
    // overrides `TopClass.<<equality.Key>> sides : SideClass[*]` and the
    // result excludes `sides` from equality keys for `OtherBottomClass`
    // instances. Tracking *all* seen property names (not just keys)
    // enforces this: when the BFS reaches the supertype with a key, the
    // name is already in `seen_names` and gets skipped.
    let mut visited: std::collections::HashSet<ElementId> = std::collections::HashSet::new();
    let mut queue: std::collections::VecDeque<ElementId> = std::collections::VecDeque::new();
    let mut seen_names: std::collections::HashSet<SmolStr> = std::collections::HashSet::new();
    let mut out: Vec<SmolStr> = Vec::new();
    queue.push_back(class_id);
    visited.insert(class_id);
    while let Some(cid) = queue.pop_front() {
        let Element::Class(class) = model.get_element(cid) else {
            continue;
        };
        for prop in &class.properties {
            if !seen_names.insert(prop.name.clone()) {
                continue; // subclass already defined (or redefined) this name
            }
            if prop
                .stereotypes
                .iter()
                .any(|s| is_equality_key_stereotype(s, equality_profile))
            {
                out.push(prop.name.clone());
            }
        }
        for super_ty in &class.super_types {
            if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = super_ty
                && visited.insert(*element)
            {
                queue.push_back(*element);
            }
        }
    }
    out
}

/// True when a stereotype reference is exactly `meta::pure::profiles::equality.Key`.
///
/// Both the profile identity and the stereotype label must match — a
/// stereotype named `Key` in some *other* profile does not qualify as
/// a structural-equality marker.
#[must_use]
pub fn is_equality_key_stereotype(stereo: &StereotypeRef, equality_profile: ElementId) -> bool {
    stereo.profile == equality_profile && stereo.value.as_str() == "Key"
}
