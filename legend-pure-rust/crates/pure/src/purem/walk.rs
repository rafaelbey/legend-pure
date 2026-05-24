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

//! Generic visitor that walks every [`ElementId`] inside an [`Element`].
//!
//! Used by [`super::slice::slice_by_repo`] to translate cross-repo
//! references into FQN-indexed sentinels, and by
//! [`super::slice::merge_slice`] to do the reverse rewrite when a slice
//! lands in a fresh `PureModel`.
//!
//! The visitor is mutable: the closure receives `&mut ElementId` so it can
//! patch references in place. There are no fallible code paths — the
//! closure is responsible for any error handling.

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::ids::ElementId;
use crate::model::Element;
use crate::nodes::class::{Class, Constraint, Property, QualifiedProperty};
use crate::nodes::function::Function;
use crate::types::{ExprKind, FunctionCallData, Multiplicity, Parameter, TypeExpr, ValueSpec};

/// Walk every [`ElementId`] reference inside an [`Element`], invoking the
/// callback with a mutable reference. Modifications by the closure persist.
pub fn walk_element_ids(element: &mut Element, mut visit: impl FnMut(&mut ElementId)) {
    walk_element_ids_inner(element, &mut visit);
}

fn walk_element_ids_inner(element: &mut Element, visit: &mut dyn FnMut(&mut ElementId)) {
    match element {
        Element::Class(class) => walk_class(class, visit),
        Element::Enumeration(en) => {
            for s in &mut en.stereotypes {
                walk_stereo(s, visit);
            }
            for tv in &mut en.tagged_values {
                walk_tag(tv, visit);
            }
            for v in &mut en.values {
                for s in &mut v.stereotypes {
                    walk_stereo(s, visit);
                }
                for tv in &mut v.tagged_values {
                    walk_tag(tv, visit);
                }
            }
        }
        Element::Function(f) => walk_function(f, visit),
        #[allow(clippy::match_same_arms)]
        // distinct reasons per variant — keep the explanatory comments greppable
        Element::Profile(_) => {
            // Profile only carries stereotype / tag *names* (not refs).
            // Nothing to walk.
        }
        Element::Association(assoc) => {
            for prop in &mut assoc.properties {
                walk_property(prop, visit);
            }
            for qp in &mut assoc.qualified_properties {
                walk_qualified_property(qp, visit);
            }
            for s in &mut assoc.stereotypes {
                walk_stereo(s, visit);
            }
            for tv in &mut assoc.tagged_values {
                walk_tag(tv, visit);
            }
            for prop in &mut assoc.original_milestoned_properties {
                walk_property(prop, visit);
            }
        }
        Element::Measure(m) => {
            if let Some(c) = &mut m.canonical_unit {
                visit(c);
            }
            for u in &mut m.non_canonical_units {
                visit(u);
            }
        }
        Element::Unit(u) => {
            visit(&mut u.measure);
            if let Some(expr) = &mut u.conversion_expression {
                walk_value_spec(expr, visit);
            }
        }
        Element::PrimitiveType(p) => {
            if let Some(s) = &mut p.super_type {
                visit(s);
            }
            for tvp in &mut p.type_variable_parameters {
                walk_param(tvp, visit);
            }
            for c in &mut p.constraints {
                walk_constraint(c, visit);
            }
        }
        #[allow(clippy::match_same_arms)]
        // distinct reasons per variant — keep the explanatory comments greppable
        Element::PackageableMultiplicity(_) => {
            // No ElementId references; a Multiplicity is structural data.
        }
        #[allow(clippy::match_same_arms)]
        Element::Package(_) => {
            // Package entries hold a PackageId, not an element-level ref.
        }
        Element::DSLInstance(_) => {
            // DSL instances carry an opaque Postcard payload that the
            // contributing DSL extension owns. The Pure walker doesn't
            // know its shape and therefore can't visit any ElementId
            // refs inside it. Two consequences worth flagging:
            //
            // 1. Cross-element references inside a DSL payload are not
            //    redirected on slice/merge — DSLs that need cross-repo
            //    refs must encode them as FQNs themselves.
            // 2. The slice/merge `external_refs` table is M3-only.
            //
            // Both are acceptable for the current Diagram pilot
            // (diagrams reference Classes by FQN already in their AST).
        }
    }
}

fn walk_function(f: &mut Function, visit: &mut dyn FnMut(&mut ElementId)) {
    // Function.parameters and Function.body are Rc<[…]>. Replace with
    // cloned-and-edited boxed slices: at slice/merge time the runtime
    // sharing across calls is not yet established, so it's safe (and
    // cheaper than failing) to break the share here.
    let mut params: Vec<Parameter> = f.parameters.iter().cloned().collect();
    for p in &mut params {
        walk_param(p, visit);
    }
    f.parameters = params.into();

    walk_type(&mut f.return_type, visit);
    walk_multiplicity(&mut f.return_multiplicity, visit);

    let mut body: Vec<ValueSpec> = f.body.iter().cloned().collect();
    for expr in &mut body {
        walk_value_spec(expr, visit);
    }
    f.body = body.into();

    for s in &mut f.stereotypes {
        walk_stereo(s, visit);
    }
    for tv in &mut f.tagged_values {
        walk_tag(tv, visit);
    }
}

fn walk_class(c: &mut Class, visit: &mut dyn FnMut(&mut ElementId)) {
    for tp in &mut c.type_variable_parameters {
        walk_param(tp, visit);
    }
    for st in &mut c.super_types {
        walk_type(st, visit);
    }
    for p in &mut c.properties {
        walk_property(p, visit);
    }
    for qp in &mut c.qualified_properties {
        walk_qualified_property(qp, visit);
    }
    for con in &mut c.constraints {
        walk_constraint(con, visit);
    }
    for s in &mut c.stereotypes {
        walk_stereo(s, visit);
    }
    for tv in &mut c.tagged_values {
        walk_tag(tv, visit);
    }
    for p in &mut c.original_milestoned_properties {
        walk_property(p, visit);
    }
}

fn walk_property(p: &mut Property, visit: &mut dyn FnMut(&mut ElementId)) {
    walk_type(&mut p.type_expr, visit);
    walk_multiplicity(&mut p.multiplicity, visit);
    if let Some(dv) = &mut p.default_value {
        walk_value_spec(dv, visit);
    }
    for s in &mut p.stereotypes {
        walk_stereo(s, visit);
    }
    for tv in &mut p.tagged_values {
        walk_tag(tv, visit);
    }
}

fn walk_qualified_property(qp: &mut QualifiedProperty, visit: &mut dyn FnMut(&mut ElementId)) {
    // QP parameters / body live behind Rc<[…]>. Replace with cloned-and-edited
    // boxed slices: at slice/merge time we are restructuring identity, so
    // sharing across QPs is acceptable to break.
    let mut params: Vec<Parameter> = qp.parameters.iter().cloned().collect();
    for p in &mut params {
        walk_param(p, visit);
    }
    qp.parameters = params.into();

    walk_type(&mut qp.return_type, visit);
    walk_multiplicity(&mut qp.return_multiplicity, visit);

    let mut body: Vec<ValueSpec> = qp.body.iter().cloned().collect();
    for expr in &mut body {
        walk_value_spec(expr, visit);
    }
    qp.body = body.into();

    for s in &mut qp.stereotypes {
        walk_stereo(s, visit);
    }
    for tv in &mut qp.tagged_values {
        walk_tag(tv, visit);
    }
}

fn walk_constraint(c: &mut Constraint, visit: &mut dyn FnMut(&mut ElementId)) {
    walk_value_spec(&mut c.function, visit);
    if let Some(m) = &mut c.message {
        walk_value_spec(m, visit);
    }
}

fn walk_param(p: &mut Parameter, visit: &mut dyn FnMut(&mut ElementId)) {
    walk_type(&mut p.type_expr, visit);
    walk_multiplicity(&mut p.multiplicity, visit);
}

fn walk_stereo(s: &mut StereotypeRef, visit: &mut dyn FnMut(&mut ElementId)) {
    visit(&mut s.profile);
}

fn walk_tag(t: &mut TaggedValueRef, visit: &mut dyn FnMut(&mut ElementId)) {
    visit(&mut t.profile);
}

fn walk_multiplicity(_m: &mut Multiplicity, _visit: &mut dyn FnMut(&mut ElementId)) {
    // Multiplicity carries no ElementId today. This stub stays as a hook
    // for a future variant (e.g. resolved PackageableMultiplicity refs).
}

/// Walk every [`ElementId`] inside a [`TypeExpr`] tree.
///
/// Public so DSL extensions can reuse the same recursion when they
/// register their own `crate::purem::extension::PuremExtension` payloads.
pub fn walk_type(ty: &mut TypeExpr, visit: &mut dyn FnMut(&mut ElementId)) {
    match ty {
        TypeExpr::Named {
            element,
            type_arguments,
            multiplicity_arguments: _,
            value_arguments: _,
            source_info: _,
        } => {
            visit(element);
            for arg in type_arguments {
                walk_type(arg, visit);
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            return_multiplicity,
        } => {
            for (pty, pmul) in parameters {
                walk_type(pty, visit);
                walk_multiplicity(pmul, visit);
            }
            walk_type(return_type, visit);
            walk_multiplicity(return_multiplicity, visit);
        }
        TypeExpr::Relation(_) | TypeExpr::Generic(_) | TypeExpr::Unresolved => {}
        TypeExpr::AlgebraUnion(a, b) => {
            walk_type(a, visit);
            walk_type(b, visit);
        }
    }
}

fn walk_value_spec(vs: &mut ValueSpec, visit: &mut dyn FnMut(&mut ElementId)) {
    if let Some(t) = &mut vs.type_info {
        walk_type(&mut t.type_expr, visit);
        walk_multiplicity(&mut t.multiplicity, visit);
    }
    walk_expr_kind(&mut vs.kind, visit);
}

fn walk_expr_kind(kind: &mut ExprKind, visit: &mut dyn FnMut(&mut ElementId)) {
    match kind {
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_)
        | ExprKind::Variable { .. } => {}
        ExprKind::FunctionCall(d)
        | ExprKind::PropertyCall(d)
        | ExprKind::QualifiedPropertyCall(d) => {
            walk_function_call_data(d, visit);
        }
        ExprKind::EnumValue { enum_element, .. } => {
            visit(enum_element);
        }
        ExprKind::Lambda { parameters, body } => {
            for p in parameters {
                walk_param(p, visit);
            }
            for e in body {
                walk_value_spec(e, visit);
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                walk_value_spec(e, visit);
            }
        }
        ExprKind::TypeReference { type_expr } => {
            walk_type(type_expr, visit);
        }
        ExprKind::MultiplicityReference { multiplicity } => {
            walk_multiplicity(multiplicity, visit);
        }
        ExprKind::PackageableElementRef { element } => {
            visit(element);
        }
        ExprKind::RelationLiteral { columns } | ExprKind::ColSpecArrayLiteral { columns, .. } => {
            for col in columns {
                visit(&mut col.type_element);
                walk_multiplicity(&mut col.multiplicity, visit);
            }
        }
        ExprKind::ColSpecLiteral { column, .. } => {
            visit(&mut column.type_element);
            walk_multiplicity(&mut column.multiplicity, visit);
        }
        ExprKind::PathLiteral {
            start_type, steps, ..
        } => {
            walk_type(start_type, visit);
            for step in steps {
                for param in &mut step.parameters {
                    walk_value_spec(param, visit);
                }
            }
        }
    }
}

fn walk_function_call_data(d: &mut FunctionCallData, visit: &mut dyn FnMut(&mut ElementId)) {
    if let Some(f) = &mut d.function {
        visit(f);
    }
    for a in &mut d.arguments {
        walk_value_spec(a, visit);
    }
}
