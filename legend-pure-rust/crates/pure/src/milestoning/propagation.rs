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

//! Pass 2.5b — Milestoning date-context propagation.
//!
//! Java parity: `MilestoningDatesPropagationFunctions` +
//! `MilestoningDateSourceType`. Runs in [`finalize_model`] between Pass 2.5
//! (type inference) and Pass 3 (validation).
//!
//! Without this pass, user code that writes `$customer.address` on a
//! milestoned-target property fails — Phase A moved `address` to
//! `original_milestoned_properties` and replaced it with the qualified
//! property `address(td: Date[1])`. The user would have to pass `$td`
//! explicitly at every call site. This pass detects the implicit form,
//! finds the in-scope milestoning date(s) by walking the enclosing
//! expression context, and rewrites `PropertyCall("address", [$customer])`
//! into `QualifiedPropertyCall("address", [$customer, $td])`.
//!
//! ## Date source variants supported (a subset of Java's 8)
//!
//! The detector recognises these source kinds, mirroring Java's
//! `MilestoningDateSourceType` (in priority order):
//!
//! - **Variable lookup** — `$x` where the propagation scope stack has
//!   dates registered for `x` (set up by surrounding `filter` / `subType`
//!   / `map` lambdas — see below).
//! - **`getAll(Class, $date[, $date])`** — the date(s) supplied to the
//!   bulk fetch are visible to every property access on the result.
//! - **`filter(coll, x | …)` / `subType(coll, x | …)` / `map(coll, x | …)`**
//!   — recognised when *visiting the lambda*: we extract the milestoning
//!   context from `coll` and register it for the lambda's parameter.
//!
//! ## Variants deferred to a future B-3 follow-up
//!
//! - `%latest` sentinel (requires new `ExprKind` + `Value` variants)
//! - Already-explicit milestoned QP propagating its own dates to nested
//!   property access on the result
//! - AutoMap of milestoned collections through nested property chains
//!
//! The current slice covers the bread-and-butter pattern most existing
//! Pure milestoning code uses (`Class.getAll($d)->map(c | $c.prop)`).

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::error::CompilationError;
use crate::ids::ElementId;
use crate::milestoning::dates::MilestoningDates;
use crate::milestoning::{inherited_temporal_stereotype, resolve_temporal_profile};
use crate::model::{Element, PureModel};
use crate::nodes::class::Property;
use crate::types::{ExprKind, FunctionCallData, TypeExpr, ValueSpec};

/// Entry point. Walks every expression body in every chunk and rewrites
/// milestoned-target property accesses to qualified-property calls with
/// the in-scope dates injected as arguments.
pub fn propagate_dates(model: &mut PureModel, errors: &mut Vec<CompilationError>) {
    let Some(temporal_profile) = resolve_temporal_profile(model) else {
        // No platform → no milestoning anywhere → nothing to do.
        return;
    };

    // Snapshot targets — list of (chunk_id, local_idx, element_kind) — so
    // we can mutate model.chunks[].elements without holding an iterator
    // across the inner closure.
    let targets: Vec<(u16, u32)> = model
        .chunks
        .iter()
        .flat_map(|chunk| {
            chunk
                .elements
                .iter()
                .map(move |(local_idx, _)| (chunk.chunk_id, local_idx))
        })
        .collect();

    for (chunk_id, local_idx) in targets {
        rewrite_element(model, chunk_id, local_idx, temporal_profile, errors);
    }
}

// ---------------------------------------------------------------------------
// Element-level entry — clone-mutate-write each element's expression bodies.
// ---------------------------------------------------------------------------

fn rewrite_element(
    model: &mut PureModel,
    chunk_id: u16,
    local_idx: u32,
    temporal_profile: ElementId,
    errors: &mut Vec<CompilationError>,
) {
    // Snapshot the element so we can read it while building rewrites,
    // then write back at the end.
    let element_clone = model.chunks[chunk_id as usize]
        .elements
        .get(local_idx)
        .clone();

    match element_clone {
        Element::Function(mut func) => {
            let outer_scope = function_parameter_scope(&func.parameters);
            let mut new_body: Vec<ValueSpec> = func.body.iter().cloned().collect();
            for expr in &mut new_body {
                rewrite_value_spec(model, temporal_profile, expr, &outer_scope, errors);
            }
            func.body = std::sync::Arc::from(new_body);
            *model.chunks[chunk_id as usize].elements.get_mut(local_idx) = Element::Function(func);
        }
        Element::Class(mut class) => {
            let owner_id = ElementId::InstanceId {
                chunk_id,
                local_idx,
            };
            // Each QP has its own parameter scope. Inside each QP body,
            // `$this` is bound to the owner class — but Phase A's
            // synthesized QPs (`address(td)` etc.) have empty bodies, so
            // we skip them and only rewrite user-declared QPs.
            for qp in &mut class.qualified_properties {
                if qp.body.is_empty() {
                    continue;
                }
                let scope = qp_parameter_scope(&qp.parameters, owner_id, model);
                let mut new_body: Vec<ValueSpec> = qp.body.iter().cloned().collect();
                for expr in &mut new_body {
                    rewrite_value_spec(model, temporal_profile, expr, &scope, errors);
                }
                qp.body = std::sync::Arc::from(new_body);
            }
            // Constraints have a $this scope too.
            for con in &mut class.constraints {
                let scope = class_this_scope(owner_id);
                rewrite_value_spec(model, temporal_profile, &mut con.function, &scope, errors);
                if let Some(msg) = &mut con.message {
                    rewrite_value_spec(model, temporal_profile, msg, &scope, errors);
                }
            }
            // Property default-value expressions evaluate in a $this scope.
            for prop in &mut class.properties {
                if let Some(dv) = &mut prop.default_value {
                    let scope = class_this_scope(owner_id);
                    rewrite_value_spec(model, temporal_profile, dv, &scope, errors);
                }
            }
            *model.chunks[chunk_id as usize].elements.get_mut(local_idx) = Element::Class(class);
        }
        Element::Association(_)
        | Element::Enumeration(_)
        | Element::Profile(_)
        | Element::Measure(_)
        | Element::Unit(_)
        | Element::PrimitiveType(_)
        | Element::PackageableMultiplicity(_)
        | Element::Package(_)
        | Element::DSLInstance(_) => {
            // Associations carry no QP bodies in current usage; primitive
            // constraints are pre-validated; the rest have no expressions
            // to walk.
        }
    }
}

// ---------------------------------------------------------------------------
// Scope construction — initial bindings before walking an expression tree.
// ---------------------------------------------------------------------------

fn function_parameter_scope(_params: &[crate::types::Parameter]) -> PropagationScope {
    // Function parameters never auto-carry milestoning context: a user
    // who declares `function f(d: Date[1])` is responsible for threading
    // `$d` through milestoned-property calls themselves. Variant 1
    // matches a variable only when surrounding lambda machinery has
    // registered dates for it.
    PropagationScope::new()
}

fn qp_parameter_scope(
    _params: &[crate::types::Parameter],
    _owner_id: ElementId,
    _model: &PureModel,
) -> PropagationScope {
    PropagationScope::new()
}

fn class_this_scope(_owner_id: ElementId) -> PropagationScope {
    PropagationScope::new()
}

// ---------------------------------------------------------------------------
// PropagationScope — stack of (variable name → milestoning context) maps.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct PropagationScope {
    frames: Vec<HashMap<SmolStr, MilestoningDates>>,
}

impl PropagationScope {
    fn new() -> Self {
        Self {
            frames: vec![HashMap::new()],
        }
    }

    fn push(&mut self) {
        self.frames.push(HashMap::new());
    }

    fn bind(&mut self, name: SmolStr, dates: MilestoningDates) {
        if let Some(top) = self.frames.last_mut() {
            top.insert(name, dates);
        }
    }

    fn lookup(&self, name: &str) -> Option<&MilestoningDates> {
        for frame in self.frames.iter().rev() {
            if let Some(d) = frame.get(name) {
                return Some(d);
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Recursive rewriter
// ---------------------------------------------------------------------------

fn rewrite_value_spec(
    model: &PureModel,
    temporal_profile: ElementId,
    expr: &mut ValueSpec,
    scope: &PropagationScope,
    errors: &mut Vec<CompilationError>,
) {
    rewrite_expr_kind(model, temporal_profile, &mut expr.kind, scope, errors);
}

fn rewrite_expr_kind(
    model: &PureModel,
    temporal_profile: ElementId,
    kind: &mut ExprKind,
    scope: &PropagationScope,
    errors: &mut Vec<CompilationError>,
) {
    match kind {
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_)
        | ExprKind::Variable { .. }
        | ExprKind::EnumValue { .. }
        | ExprKind::TypeReference { .. }
        | ExprKind::MultiplicityReference { .. }
        | ExprKind::PackageableElementRef { .. }
        | ExprKind::RelationLiteral { .. }
        | ExprKind::ColSpecArrayLiteral { .. }
        | ExprKind::ColSpecLiteral { .. }
        | ExprKind::PathLiteral { .. } => {}

        ExprKind::Collection { elements } => {
            for e in elements {
                rewrite_value_spec(model, temporal_profile, e, scope, errors);
            }
        }

        ExprKind::Lambda { parameters, body } => {
            // Free-standing lambda (no enclosing scoped-call). Recurse
            // with a fresh frame; parameters do NOT auto-receive
            // milestoning context — that's the job of the surrounding
            // call (filter/map/subType) which detects this lambda and
            // calls `rewrite_scoped_lambda` instead.
            let mut inner = scope.clone();
            inner.push();
            for _ in parameters {
                // No auto-bindings here.
            }
            for e in body {
                rewrite_value_spec(model, temporal_profile, e, &inner, errors);
            }
        }

        ExprKind::FunctionCall(data) => {
            rewrite_function_call(model, temporal_profile, data, scope, errors);
        }

        ExprKind::PropertyCall(data) | ExprKind::QualifiedPropertyCall(data) => {
            // Recurse into arguments (the receiver in [0] + any QP args)
            // before considering rewrite. Children get their own date
            // context first; the parent decides about itself last.
            for arg in &mut data.arguments {
                rewrite_value_spec(model, temporal_profile, arg, scope, errors);
            }
            // After children: try to rewrite *this* node if it's a
            // milestoned-target access missing dates.
            try_inject_dates_on_property_call(model, temporal_profile, kind, scope, errors);
        }
    }
}

/// Walk a `FunctionCall`, recurse into arguments, and handle the special
/// cases for milestoning-aware functions (`filter`, `map`, `subType`,
/// `getAll`). For known scoped-lambda calls, we register the milestoning
/// context of `arguments[0]` for the lambda's parameter 0.
fn rewrite_function_call(
    model: &PureModel,
    temporal_profile: ElementId,
    data: &mut FunctionCallData,
    scope: &PropagationScope,
    errors: &mut Vec<CompilationError>,
) {
    // Rewrite arg 0 (the receiver / source) first so its own milestoning
    // shape is finalised before we extract dates from it.
    if let Some(first) = data.arguments.first_mut() {
        rewrite_value_spec(model, temporal_profile, first, scope, errors);
    }

    // Detect dates from arg 0 (used by filter / map / subType to bind
    // their lambda parameter, and used as the "child scope" for any
    // lambda arg that follows).
    let arg0_dates: Option<MilestoningDates> = data
        .arguments
        .first()
        .and_then(|a| detect_dates_from_expression(model, temporal_profile, a, scope));

    let scoped_lambda_call = matches!(
        data.function_name.as_str(),
        "filter" | "map" | "subType" | "exists" | "forAll" | "first" | "removeDuplicates"
    );

    // Walk the remaining arguments, threading milestoning context into
    // any lambda that's an argument of a scoped lambda call.
    for (idx, arg) in data.arguments.iter_mut().enumerate().skip(1) {
        if scoped_lambda_call && let ExprKind::Lambda { parameters, body } = arg.kind.as_mut() {
            let mut inner_scope = scope.clone();
            inner_scope.push();
            // Bind every lambda parameter to the same milestoning context
            // as the source — Java threads through parameter 0; for the
            // multi-parameter operators it's the same context across all
            // params.
            if let Some(dates) = &arg0_dates {
                for p in parameters.iter() {
                    inner_scope.bind(p.name.clone(), dates.clone());
                }
            }
            for e in body {
                rewrite_value_spec(model, temporal_profile, e, &inner_scope, errors);
            }
        } else {
            let _ = idx;
            rewrite_value_spec(model, temporal_profile, arg, scope, errors);
        }
    }
}

// ---------------------------------------------------------------------------
// Date-source detection — Java's MilestoningDateSourceType variants
// ---------------------------------------------------------------------------

fn detect_dates_from_expression(
    model: &PureModel,
    temporal_profile: ElementId,
    expr: &ValueSpec,
    scope: &PropagationScope,
) -> Option<MilestoningDates> {
    match expr.kind.as_ref() {
        // Variant 1 — Variable lookup in the propagation scope stack.
        ExprKind::Variable { name } => scope.lookup(name).cloned(),

        // Variant 3 — getAll(Class, $date) / getAll(Class, $pd, $bd) /
        // getAllVersionsInRange(Class, $start, $end). The date(s) supplied
        // here flow downstream to property accesses on the result.
        ExprKind::FunctionCall(data) => {
            detect_dates_from_function_call(model, temporal_profile, data, scope)
        }

        // Variant 2 — `$x.qp($date)` or `$x.qp($pd, $bd)` — the explicit
        // QP call carries its own dates which propagate to nested access.
        // Variant 4 partial — receiver's own date context propagates
        // through ordinary property access to milestoned-target results
        // (e.g. `$customer.account.<…>` where account is also milestoned).
        ExprKind::QualifiedPropertyCall(data) | ExprKind::PropertyCall(data) => {
            detect_dates_from_qp_call(model, temporal_profile, data, scope)
        }

        _ => None,
    }
}

fn detect_dates_from_function_call(
    model: &PureModel,
    temporal_profile: ElementId,
    data: &FunctionCallData,
    scope: &PropagationScope,
) -> Option<MilestoningDates> {
    match data.function_name.as_str() {
        "getAll" if data.arguments.len() == 2 || data.arguments.len() == 3 => {
            // arg[0] is the Class; arg[1..] are dates.
            let class_id = class_id_of_first_arg(&data.arguments)?;
            let stereo = inherited_temporal_stereotype(model, class_id, temporal_profile)?;
            let date_args: Vec<ValueSpec> = data.arguments[1..].to_vec();
            MilestoningDates::from_source(stereo, &date_args)
        }
        "getAllVersionsInRange" if data.arguments.len() == 3 => {
            // Range queries don't propagate dates to downstream property
            // access — each version in the range has its own date. Java
            // treats `getAllVersionsInRange` as a non-source. Mirror that.
            None
        }
        "filter" | "map" | "subType" | "exists" | "forAll" | "first" | "removeDuplicates"
            if !data.arguments.is_empty() =>
        {
            // The result's milestoning context is the same as the source's.
            detect_dates_from_expression(model, temporal_profile, &data.arguments[0], scope)
        }
        _ => None,
    }
}

fn detect_dates_from_qp_call(
    model: &PureModel,
    temporal_profile: ElementId,
    data: &FunctionCallData,
    scope: &PropagationScope,
) -> Option<MilestoningDates> {
    // Variant 2 — `$x.qp($date)` / `$x.qp($pd, $bd)` with explicit
    // dates. The dates the user passed *to this QP* describe the
    // milestoning slice the result lives in, so they propagate to any
    // nested milestoned-target access on the result.
    //
    // Detection: arg[0] is the receiver; arg[1..] are the explicit
    // dates iff each is Date-typed and the receiver class declares a QP
    // by this name + arity whose return type is milestoned.
    if data.arguments.len() >= 2 {
        let date_args = &data.arguments[1..];
        if date_args.iter().all(is_date_typed_expr)
            && let Some(receiver) = data.arguments.first()
            && let Some(receiver_class_id) = resolved_class_of(receiver)
            && let Element::Class(receiver_class) = model.get_element(receiver_class_id)
            && let Some(target_stereotype) = find_qp_target_stereotype(
                model,
                receiver_class,
                &data.function_name,
                date_args.len(),
                temporal_profile,
            )
            && let Some(dates) = MilestoningDates::from_source(target_stereotype, date_args)
        {
            return Some(dates);
        }
    }

    // Variant 4 fallback / no-explicit-dates path — the receiver's own
    // milestoning context propagates through this QP call to the
    // result. Used for `$customer.address` where `address` was already
    // rewritten to `address(td)` by a surrounding source.
    data.arguments
        .first()
        .and_then(|r| detect_dates_from_expression(model, temporal_profile, r, scope))
}

/// Look up the target temporal stereotype for a qualified property on
/// `receiver_class` by `name` and `extra_arity` (the number of arguments
/// *after* the receiver). Walks supertypes for inherited QPs. Returns
/// the QP's return-type stereotype, or `None` when the QP isn't
/// milestoning-generated.
fn find_qp_target_stereotype(
    model: &PureModel,
    receiver_class: &crate::nodes::class::Class,
    qp_name: &SmolStr,
    extra_arity: usize,
    temporal_profile: ElementId,
) -> Option<crate::milestoning::MilestoningStereotype> {
    if let Some(qp) = receiver_class
        .qualified_properties
        .iter()
        .find(|q| &q.name == qp_name && q.parameters.len() == extra_arity)
        && let TypeExpr::Named { element, .. } = &qp.return_type
        && let Some(s) = inherited_temporal_stereotype(model, *element, temporal_profile)
    {
        return Some(s);
    }
    // Walk supertypes.
    for st in &receiver_class.super_types {
        if let TypeExpr::Named { element, .. } = st
            && *element != crate::bootstrap::ANY_ID
            && let Element::Class(parent) = model.get_element(*element)
            && let Some(s) =
                find_qp_target_stereotype(model, parent, qp_name, extra_arity, temporal_profile)
        {
            return Some(s);
        }
    }
    None
}

fn is_date_typed_expr(expr: &ValueSpec) -> bool {
    if let Some(t) = &expr.type_info
        && let TypeExpr::Named { element, .. } = &t.type_expr
        && *element == crate::bootstrap::DATE_ID
    {
        return true;
    }
    matches!(expr.kind.as_ref(), ExprKind::DateLiteral(_))
}

fn class_id_of_first_arg(args: &[ValueSpec]) -> Option<ElementId> {
    let first = args.first()?;
    match first.kind.as_ref() {
        ExprKind::PackageableElementRef { element }
        | ExprKind::TypeReference {
            type_expr: TypeExpr::Named { element, .. },
        } => Some(*element),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rewrite: PropertyCall on a milestoned-target property → QualifiedPropertyCall
// ---------------------------------------------------------------------------

fn try_inject_dates_on_property_call(
    model: &PureModel,
    temporal_profile: ElementId,
    kind: &mut ExprKind,
    scope: &PropagationScope,
    errors: &mut Vec<CompilationError>,
) {
    // Snapshot the per-variant info we need so we can drop the
    // borrow before mutating `*kind`.
    let (prop_name, receiver, current_arity, is_qualified) = match kind {
        ExprKind::PropertyCall(d) => {
            let Some(receiver) = d.arguments.first().cloned() else {
                return;
            };
            (d.function_name.clone(), receiver, d.arguments.len(), false)
        }
        ExprKind::QualifiedPropertyCall(d) => {
            let Some(receiver) = d.arguments.first().cloned() else {
                return;
            };
            (d.function_name.clone(), receiver, d.arguments.len(), true)
        }
        _ => return,
    };

    // Already-explicit QP call (receiver + ≥1 extra arg) — leave alone.
    if is_qualified && current_arity > 1 {
        return;
    }

    // Find the receiver's resolved class — must be a Named class.
    let Some(receiver_class_id) = resolved_class_of(&receiver) else {
        return;
    };
    let Element::Class(receiver_class) = model.get_element(receiver_class_id) else {
        return;
    };

    // Is `prop_name` a milestoned-target property on the receiver class
    // or an ancestor? If yes, find its target class so we can determine
    // what date(s) the call needs.
    let Some(milestoned_target) =
        find_milestoned_target_property(model, receiver_class_id, receiver_class, &prop_name)
    else {
        return;
    };

    // What's the target's temporal stereotype? Drives how many dates to
    // inject and where to slot them.
    let Some(target_stereo) =
        inherited_temporal_stereotype(model, milestoned_target, temporal_profile)
    else {
        return;
    };

    // Detect available dates from the receiver expression.
    let Some(dates) = detect_dates_from_expression(model, temporal_profile, &receiver, scope)
    else {
        // B-4.2 — missing date context. The receiver-class declared
        // `prop_name` as a milestoned-target property, and no
        // surrounding source provided a date. Emit a hard error
        // pointing the user at the explicit-date form.
        let expected_dates: u8 = match target_stereo {
            crate::milestoning::MilestoningStereotype::Bitemporal => 2,
            _ => 1,
        };
        let receiver_si = receiver.source_info.clone();
        let receiver_class_name = model.element_name(receiver_class_id).clone();
        errors.push(CompilationError {
            message: format!(
                "Milestoned property '{prop_name}' on '{receiver_class_name}' is accessed \
                 without a milestoning date in scope. Supply the date explicitly \
                 (`{prop_name}({})`) or wrap the access in a milestoning context.",
                if expected_dates == 2 {
                    "$pd, $bd"
                } else {
                    "$d"
                },
                prop_name = prop_name,
            ),
            source_info: receiver_si,
            kind: crate::error::CompilationErrorKind::MilestoningMissingDateContext {
                class_name: receiver_class_name,
                property_name: prop_name.clone(),
                expected_dates,
            },
        });
        return;
    };

    // Project the dates to the target's stereotype and inject.
    let Some(injection) = dates.for_target_stereotype(target_stereo) else {
        return;
    };

    let mut new_args = Vec::with_capacity(1 + injection.len());
    new_args.push(receiver);
    new_args.extend(injection);

    let new_data = FunctionCallData {
        function: None,
        function_name: prop_name,
        arguments: new_args,
    };
    *kind = ExprKind::QualifiedPropertyCall(new_data);
}

fn resolved_class_of(expr: &ValueSpec) -> Option<ElementId> {
    let t = expr.type_info.as_ref()?;
    match &t.type_expr {
        TypeExpr::Named { element, .. } => Some(*element),
        _ => None,
    }
}

/// Walk a class's `original_milestoned_properties` plus its supertypes'
/// equivalent and return the target class `ElementId` for `prop_name`, if
/// the property is a milestoned-target.
fn find_milestoned_target_property(
    model: &PureModel,
    class_id: ElementId,
    class: &crate::nodes::class::Class,
    prop_name: &SmolStr,
) -> Option<ElementId> {
    if let Some(prop) = class
        .original_milestoned_properties
        .iter()
        .find(|p| p.name == *prop_name)
    {
        return target_class_of_property(prop);
    }
    // Walk supertypes for inherited milestoned-target properties.
    for st in &class.super_types {
        if let TypeExpr::Named { element, .. } = st
            && *element != class_id
            && *element != crate::bootstrap::ANY_ID
            && let Element::Class(sc) = model.get_element(*element)
            && let Some(found) = find_milestoned_target_property(model, *element, sc, prop_name)
        {
            return Some(found);
        }
    }
    None
}

fn target_class_of_property(prop: &Property) -> Option<ElementId> {
    match &prop.type_expr {
        TypeExpr::Named { element, .. } => Some(*element),
        _ => None,
    }
}
