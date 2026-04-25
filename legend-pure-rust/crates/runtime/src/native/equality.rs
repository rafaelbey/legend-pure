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

use crate::heap::ObjectId;
use crate::native::EvalContextTrait;
use crate::value::Value;

/// Structural value equality, heap-aware.
///
/// Primitives compare by value (delegating to `PartialEq for Value`).
/// Collections compare length then element-wise.
/// Heap objects compare via `<<equality.Key>>` stereotype if the class
/// declares any; otherwise fall back to `ObjectId` identity.
#[must_use]
pub fn values_equal(ctx: &dyn EvalContextTrait, a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Collection(xs), Value::Collection(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| values_equal(ctx, x, y))
        }
        (Value::Object(oa), Value::Object(ob)) => objects_equal(ctx, *oa, *ob),
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
    let Element::Class(class) = model.get_element(class_id) else {
        return Vec::new();
    };
    let Some(equality_profile) = crate::m3_paths::resolve(model, crate::m3_paths::EQUALITY_PROFILE)
    else {
        return Vec::new();
    };
    class
        .properties
        .iter()
        .filter(|p| {
            p.stereotypes
                .iter()
                .any(|s| is_equality_key_stereotype(s, equality_profile))
        })
        .map(|p| p.name.clone())
        .collect()
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
