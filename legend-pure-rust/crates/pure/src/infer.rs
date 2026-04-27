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

//! Type inference engine for compiled Pure expressions.
//!
//! Performs bottom-up type inference over [`ValueSpec`] trees, setting
//! the [`type_info`](ValueSpec::type_info) field on every expression node.
//!
//! # Design
//!
//! - **Inline types** — types are set directly on `ValueSpec::type_info`,
//!   eliminating the need for a side map.
//! - **Bottom-up** — literals carry their own types, variables resolve from
//!   scope, property access looks up the class, function calls use the
//!   declared return type.
//! - **Scope chain** — `let` bindings and lambda parameters push entries
//!   into a scope stack. Variable references resolve by walking up.

use std::collections::HashMap;

use smol_str::SmolStr;

use crate::bootstrap;
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::types::{
    DateValue, ExprKind, FunctionCallData, Multiplicity, Parameter, ResolvedType, TypeExpr,
    ValueSpec,
};

// ---------------------------------------------------------------------------
// Scope — variable type tracking
// ---------------------------------------------------------------------------

/// A lexical scope for variable bindings during type inference.
///
/// Function parameters initialize the root scope. `let` bindings and
/// lambda parameters extend the current scope.
struct Scope {
    /// Variable name → inferred type.
    bindings: Vec<(SmolStr, ResolvedType)>,
}

#[allow(dead_code)]
impl Scope {
    /// Creates a scope pre-populated with function/lambda parameters.
    fn from_params(params: &[Parameter]) -> Self {
        let bindings = params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    ResolvedType {
                        type_expr: p.type_expr.clone(),
                        multiplicity: p.multiplicity.clone(),
                    },
                )
            })
            .collect();
        Scope { bindings }
    }

    /// Looks up a variable in this scope.
    fn lookup(&self, name: &str) -> Option<&ResolvedType> {
        self.bindings
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t)
    }

    /// Binds a new variable (e.g., from `let`).
    fn bind(&mut self, name: SmolStr, inferred: ResolvedType) {
        self.bindings.push((name, inferred));
    }
}

// ---------------------------------------------------------------------------
// Inference context
// ---------------------------------------------------------------------------

/// Accumulates inferred types and errors during a single function's inference.
struct InferCtx<'a> {
    /// The compiled model (read-only).
    model: &'a PureModel,
    /// Scope chain (outermost first).
    scopes: Vec<Scope>,
    /// Errors accumulator (used in Phase B for type mismatch errors).
    errors: &'a mut Vec<CompilationError>,
}

impl InferCtx<'_> {
    /// Looks up a variable by walking the scope chain from innermost to outermost.
    fn lookup_var(&self, name: &str) -> Option<&ResolvedType> {
        for scope in self.scopes.iter().rev() {
            if let Some(t) = scope.lookup(name) {
                return Some(t);
            }
        }
        None
    }

    /// Pushes a new child scope.
    fn push_scope(&mut self, scope: Scope) {
        self.scopes.push(scope);
    }

    /// Pops the innermost scope.
    fn pop_scope(&mut self) {
        self.scopes.pop();
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Infers types for all expressions in a function body.
///
/// Sets `type_info` on every expression node in `body` (in place).
/// Type errors are appended to `errors`.
pub(crate) fn infer_function_body(
    model: &PureModel,
    params: &[Parameter],
    body: &mut [ValueSpec],
    errors: &mut Vec<CompilationError>,
) {
    let root_scope = Scope::from_params(params);
    let mut ctx = InferCtx {
        model,
        scopes: vec![root_scope],
        errors,
    };

    for expr in body.iter_mut() {
        infer_expr(&mut ctx, expr);
    }
}

// ---------------------------------------------------------------------------
// Bottom-up inference
// ---------------------------------------------------------------------------

/// Infers the type of a single expression, setting its `type_info` and
/// returning a clone of the resolved type.
#[allow(clippy::too_many_lines)]
fn infer_expr(ctx: &mut InferCtx<'_>, expr: &mut ValueSpec) -> Option<ResolvedType> {
    let result = match &mut *expr.kind {
        // -- Literals -------------------------------------------------------
        ExprKind::IntegerLiteral(_) => Some(primitive(bootstrap::INTEGER_ID)),
        ExprKind::FloatLiteral(_) => Some(primitive(bootstrap::FLOAT_ID)),
        ExprKind::DecimalLiteral(_) => Some(primitive(bootstrap::DECIMAL_ID)),
        ExprKind::StringLiteral(_) => Some(primitive(bootstrap::STRING_ID)),
        ExprKind::BooleanLiteral(_) => Some(primitive(bootstrap::BOOLEAN_ID)),
        ExprKind::DateLiteral(dv) => Some(date_literal_type(dv)),

        // -- Variable -------------------------------------------------------
        ExprKind::Variable { name } => ctx.lookup_var(name).cloned(),

        // -- Function call --------------------------------------------------
        //
        // Overload-by-signature dispatch via `infer_function_call`.
        ExprKind::FunctionCall(FunctionCallData {
            function,
            function_name,
            arguments,
        }) => {
            // Infer argument types first (bottom-up)
            let arg_types: Vec<Option<ResolvedType>> =
                arguments.iter_mut().map(|a| infer_expr(ctx, a)).collect();

            // Extract let name from AST before calling inference
            let let_name = if function_name == "letFunction" && arguments.len() == 2 {
                if let ExprKind::StringLiteral(name) = &*arguments[0].kind {
                    Some((name, &arguments[0].source_info))
                } else {
                    None
                }
            } else {
                None
            };

            let result = infer_function_call(ctx, *function, function_name, let_name, &arg_types);
            return set_and_return(expr, result);
        }

        // -- Property / qualified-property call ----------------------------
        //
        // `arguments[0]` is the receiver; for QP, `arguments[1..]` are
        // the QP arguments. Resolution is class-property lookup, not
        // function dispatch. Mirrors Java's `SimpleFunctionExpression`
        // with `_propertyName` / `_qualifiedPropertyName` set.
        //
        // **Automap rewrite (Java parity).** When the receiver
        // multiplicity is NOT strictly `[1..1]` — i.e., `[0..1]`,
        // `[*]`, `[1..*]`, or any non-unit `Range` — we rewrite in
        // place to `map(receiver, λ{v_automap | property_call(v_automap, ...)})`.
        // Mirrors `FunctionExpressionProcessor.reprocessPropertyForManySources`
        // which uses `isToOne(mult, true)` strict on the receiver.
        // The synthetic lambda parameter is named `v_automap` (same
        // sentinel Java uses) so downstream tooling that wants to
        // unwrap the synthetic shape (`Automap.getAutoMapExpressionSequence`,
        // milestoning, class projection) sees the marker.
        ExprKind::PropertyCall(_) | ExprKind::QualifiedPropertyCall(_) => {
            return infer_property_or_qp_call(ctx, expr);
        }

        // -- Enum value -----------------------------------------------------
        ExprKind::EnumValue { enum_element, .. } => Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: *enum_element,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::PureOne,
        }),

        // -- Lambda ---------------------------------------------------------
        ExprKind::Lambda { parameters, body } => {
            // Push a child scope with lambda parameters
            ctx.push_scope(Scope::from_params(parameters));

            // Infer body types
            let mut last_type = None;
            for body_expr in body.iter_mut() {
                last_type = infer_expr(ctx, body_expr);
            }

            ctx.pop_scope();

            // The lambda's type is a FunctionType
            let param_types: Vec<(TypeExpr, Multiplicity)> = parameters
                .iter()
                .map(|p| (p.type_expr.clone(), p.multiplicity.clone()))
                .collect();

            let (return_type, return_mult) = if let Some(ref lt) = last_type {
                (lt.type_expr.clone(), lt.multiplicity.clone())
            } else {
                // No body or untyped → Any[*]
                (
                    TypeExpr::Named {
                        element: bootstrap::ANY_ID,
                        type_arguments: Vec::new(),
                        value_arguments: Vec::new(),
                    },
                    Multiplicity::ZeroOrMany,
                )
            };

            Some(ResolvedType {
                type_expr: TypeExpr::FunctionType {
                    parameters: param_types,
                    return_type: Box::new(return_type),
                    return_multiplicity: return_mult,
                },
                multiplicity: Multiplicity::PureOne,
            })
        }

        // -- Collection -----------------------------------------------------
        ExprKind::Collection { elements } => {
            let elem_types: Vec<Option<ResolvedType>> =
                elements.iter_mut().map(|e| infer_expr(ctx, e)).collect();

            let count = u32::try_from(elements.len()).unwrap_or(u32::MAX);
            let multiplicity = Multiplicity::Range {
                lower: count,
                upper: Some(count),
            };

            let type_expr = elem_types.iter().flatten().next().map_or_else(
                || TypeExpr::Named {
                    element: bootstrap::NIL_ID,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                |t| t.type_expr.clone(),
            );

            Some(ResolvedType {
                type_expr,
                multiplicity,
            })
        }

        // -- Type reference -------------------------------------------------
        ExprKind::TypeReference { type_expr: te } => Some(ResolvedType {
            type_expr: te.clone(),
            multiplicity: Multiplicity::PureOne,
        }),

        // -- Element reference ----------------------------------------------
        ExprKind::PackageableElementRef { element } => Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: *element,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::PureOne,
        }),

        // -- Column (TDS — deferred) ----------------------------------------
        ExprKind::Column => None,

        // -- Relation literals ----------------------------------------------
        // Mirror the lowering-time `type_info` so dispatch builds the right
        // mangled FQN regardless of whether the literal is consumed before
        // or after Pass 2.5.
        ExprKind::RelationLiteral { .. } => ctx
            .model
            .resolve_by_path(&[
                smol_str::SmolStr::new("meta"),
                smol_str::SmolStr::new("pure"),
                smol_str::SmolStr::new("metamodel"),
                smol_str::SmolStr::new("relation"),
                smol_str::SmolStr::new("RelationType"),
            ])
            .map(|element| ResolvedType {
                type_expr: TypeExpr::Named {
                    element,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                multiplicity: Multiplicity::PureOne,
            }),
        ExprKind::ColSpecArrayLiteral { .. } => ctx
            .model
            .resolve_by_path(&[
                smol_str::SmolStr::new("meta"),
                smol_str::SmolStr::new("pure"),
                smol_str::SmolStr::new("metamodel"),
                smol_str::SmolStr::new("relation"),
                smol_str::SmolStr::new("ColSpecArray"),
            ])
            .map(|element| ResolvedType {
                type_expr: TypeExpr::Named {
                    element,
                    type_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                },
                multiplicity: Multiplicity::PureOne,
            }),
    };

    set_and_return(expr, result)
}

/// Sets `expr.type_info` and returns the resolved type.
///
/// Honours pre-set `type_info` — when lowering already populated it
/// (e.g. `lower_new_instance` captures `^Class<T>(...)`'s parametric
/// shape directly from the AST), Pass 2.5 must not overwrite it. The
/// AST is more authoritative than the inferred-from-arg-types form
/// because it carries syntactic type-args the runtime-shaped argument
/// stream has already flattened away.
fn set_and_return(expr: &mut ValueSpec, result: Option<ResolvedType>) -> Option<ResolvedType> {
    if let Some(existing) = expr.type_info.as_deref() {
        return Some(existing.clone());
    }
    if let Some(r) = result.clone() {
        expr.type_info = Some(Box::new(r));
    }
    result
}

// ---------------------------------------------------------------------------
// Function call type inference
// ---------------------------------------------------------------------------

/// Infers the return type of a function call.
fn infer_function_call(
    ctx: &mut InferCtx<'_>,
    function: Option<crate::ids::ElementId>,
    function_name: &SmolStr,
    let_name: Option<(&SmolStr, &legend_pure_parser_ast::SourceInfo)>,
    arg_types: &[Option<ResolvedType>],
) -> Option<ResolvedType> {
    // Handle `letFunction` — side effect: bind the variable in scope
    if function_name == "letFunction"
        && arg_types.len() == 2
        && let (Some(val_type), Some((name, source_info))) = (&arg_types[1], let_name)
    {
        if let Some(scope) = ctx.scopes.last()
            && scope.lookup(name).is_some()
        {
            ctx.errors.push(crate::error::CompilationError {
                message: format!("'{name}' has already been defined!"),
                source_info: (*source_info).clone(),
                kind: crate::error::CompilationErrorKind::DuplicateVariable { name: name.clone() },
            });
        }
        if let Some(scope) = ctx.scopes.last_mut() {
            scope.bind(name.clone(), val_type.clone());
        }
        // letFunction itself returns Nil[0] (it's a side-effect statement)
        return Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: bootstrap::NIL_ID,
                type_arguments: Vec::new(),
                value_arguments: Vec::new(),
            },
            multiplicity: Multiplicity::Range {
                lower: 0,
                upper: Some(0),
            },
        });
    }

    // Resolved user function — apply generic substitution from arg
    // types so `class<T>(T[*]):Class<T>[1]` called with `$l1: List<String>`
    // produces `Class<List<String>>` instead of bare `Class<T>`. Without
    // this, downstream calls like `new($l1->class(), '')` wouldn't see
    // T's binding.
    //
    // The substitution feeds on each arg's `arg_ty.type_expr`. That
    // type_expr must carry the parametric shape — e.g. `Named{LA_List,
    // [String]}` — for binding to work. Capturing parametric info into
    // `type_info` at the lowering layer (so `^LA_List<String>(...)` and
    // `cast(@LA_List<String>)` and `extends LA_List<String>` all share
    // the same type-info plumbing) is the upstream prerequisite. This
    // helper just consumes whatever `arg_ty.type_expr` already carries.
    if let Some(Element::Function(f)) = function.and_then(|id| ctx.model.try_get_element(id)) {
        let mut bindings: std::collections::HashMap<SmolStr, TypeExpr> =
            std::collections::HashMap::new();
        for (param, arg_ty) in f.parameters.iter().zip(arg_types.iter()) {
            let Some(arg_ty) = arg_ty else { continue };
            crate::resolve::bind_type(
                &param.type_expr,
                &arg_ty.type_expr,
                &mut bindings,
                ctx.model,
            );
        }
        let type_expr = if bindings.is_empty() {
            f.return_type.clone()
        } else {
            crate::resolve::substitute_type(&f.return_type, &bindings)
        };
        return Some(ResolvedType {
            type_expr,
            multiplicity: f.return_multiplicity.clone(),
        });
    }

    // Built-in operator return types
    infer_builtin_return_type(function_name, arg_types)
}

/// Infers return types for well-known built-in operators.
fn infer_builtin_return_type(
    name: &str,
    arg_types: &[Option<ResolvedType>],
) -> Option<ResolvedType> {
    match name {
        // Comparison operators → Boolean[1]
        "equal" | "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" => {
            Some(primitive(bootstrap::BOOLEAN_ID))
        }

        // Boolean operators → Boolean[1]
        "and" | "or" | "not" => Some(primitive(bootstrap::BOOLEAN_ID)),

        // Arithmetic operators — return widened type of arguments
        "plus" | "minus" | "times" | "divide" => arg_types.iter().flatten().next().cloned(),

        // String concatenation
        "joinStrings" => Some(primitive(bootstrap::STRING_ID)),

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Property access type inference
// ---------------------------------------------------------------------------

/// Outcome of resolving `target.property` against the type model.
enum PropertyLookup {
    /// A regular property (declared on the class or association-injected,
    /// possibly inherited) matched the name.
    FoundProperty(ResolvedType),
    /// One or more qualified-property overloads matched the name.
    /// Caller picks the one whose arity matches the call site.
    FoundQualifiedProperties(Vec<QpCandidate>),
    /// Receiver resolved to a Class but no member with this name exists
    /// on it, on any supertype, or in any association targeting it.
    NotFound {
        /// Name of the receiver class (for diagnostics).
        type_name: SmolStr,
    },
    /// Receiver type is unknown / not a Class — caller should silently
    /// propagate `None` (existing upstream error already reported, or
    /// non-Class receiver is out of scope for this check).
    ///
    /// Also used when the receiver is an M3 metatype (`Class<X>`,
    /// `Enumeration<X>`, etc.) — element-side reflection (`MyEnum.RED`,
    /// `Person.name`) goes through runtime dispatch and the compile-time
    /// check would produce false positives. TODO: tighten this once the
    /// metatype-aware lookup lands.
    UnknownTarget,
}

/// One qualified-property overload candidate, ready for arity + arg-type
/// validation against a specific call site.
struct QpCandidate {
    /// Return type with receiver type-arg bindings already substituted.
    return_type: ResolvedType,
    /// QP parameter list (cloned from the resolved QP).
    parameters: Vec<Parameter>,
    /// Bindings to substitute when checking `parameters[i].type_expr`.
    bindings: HashMap<SmolStr, TypeExpr>,
    /// Receiver class name (for diagnostics).
    receiver_type_name: SmolStr,
}

/// Inference helper for simple property access (`$x.name`). Shared by
/// the legacy `ExprKind::PropertyAccess` arm and the new
/// `ExprKind::PropertyCall(FunctionCallData { .. })` arm.
///
/// Infers a `PropertyCall` or `QualifiedPropertyCall` expression in
/// place, performing the Java-parity automap rewrite when the receiver
/// is not strictly `[1..1]`.
///
/// Steps:
/// 1. Take ownership of the expression's `FunctionCallData` via
///    `mem::replace` so we can mutate `expr.kind` later without
///    borrow conflicts.
/// 2. Infer all argument types bottom-up.
/// 3. Resolve the property's return type via `infer_simple_property`
///    or `infer_qualified_property` (handles UnknownProperty errors,
///    QP arity + arg-type validation, etc.).
/// 4. If the receiver multiplicity is non-strictly-toOne, rewrite
///    `expr.kind` to a `map(receiver, λ{v_automap | property(v_automap, ...)})`
///    call and return the rewritten map's resolved type.
/// 5. Otherwise restore the original `PropertyCall` /
///    `QualifiedPropertyCall` variant and return the property's type
///    directly.
fn infer_property_or_qp_call(ctx: &mut InferCtx<'_>, expr: &mut ValueSpec) -> Option<ResolvedType> {
    // Step 1: take the data out so we can later mutate `expr.kind`.
    let is_qualified = matches!(&*expr.kind, ExprKind::QualifiedPropertyCall(_));
    let placeholder = ExprKind::IntegerLiteral(0);
    let kind = std::mem::replace(&mut *expr.kind, placeholder);
    let mut data = match kind {
        ExprKind::PropertyCall(d) | ExprKind::QualifiedPropertyCall(d) => d,
        _ => unreachable!("matched on PropertyCall/QualifiedPropertyCall above"),
    };

    // Step 2: infer argument types (bottom-up, mutates the args).
    let arg_types: Vec<Option<ResolvedType>> = data
        .arguments
        .iter_mut()
        .map(|a| infer_expr(ctx, a))
        .collect();
    let target_type: Option<ResolvedType> = arg_types.first().cloned().flatten();

    // Step 3: resolve property's return type.
    let property_return_type = if is_qualified {
        let (qp_args, qp_arg_types): (&[ValueSpec], &[Option<ResolvedType>]) =
            if data.arguments.is_empty() {
                (&[], &[])
            } else {
                (&data.arguments[1..], &arg_types[1..])
            };
        infer_qualified_property(
            ctx,
            target_type.as_ref(),
            &data.function_name,
            qp_args,
            qp_arg_types,
            &expr.source_info,
        )
    } else {
        infer_simple_property(
            ctx,
            target_type.as_ref(),
            &data.function_name,
            &expr.source_info,
        )
    };

    // Step 4: maybe rewrite to automap.
    if let (Some(rt), Some(tt)) = (property_return_type.as_ref(), target_type.as_ref())
        && !is_strictly_to_one(&tt.multiplicity)
    {
        let map_kind = build_automap_rewrite(
            ctx.model,
            data,
            tt,
            rt,
            is_qualified,
            expr.source_info.clone(),
        );
        let map_result_type = ResolvedType {
            type_expr: rt.type_expr.clone(),
            multiplicity: multiply_multiplicities(&tt.multiplicity, &rt.multiplicity),
        };
        *expr.kind = map_kind;
        return set_and_return(expr, Some(map_result_type));
    }

    // Step 5: restore the original variant.
    *expr.kind = if is_qualified {
        ExprKind::QualifiedPropertyCall(data)
    } else {
        ExprKind::PropertyCall(data)
    };
    set_and_return(expr, property_return_type)
}

/// Returns `true` when the multiplicity is strictly `[1..1]` — the only
/// shape that AVOIDS automap rewrite. `[0..1]`, `[*]`, `[1..*]`, and
/// any non-unit `Range` all return `false`. Mirrors Java's
/// `Multiplicity.isToOne(mult, true)` strict check.
fn is_strictly_to_one(m: &Multiplicity) -> bool {
    match m {
        Multiplicity::PureOne => true,
        Multiplicity::Range {
            lower: 1,
            upper: Some(1),
        } => true,
        _ => false,
    }
}

/// Multiplies two multiplicities to produce the result of `map(coll: T[m], λ: T[1] → U[n]): U[m*n]`.
fn multiply_multiplicities(a: &Multiplicity, b: &Multiplicity) -> Multiplicity {
    let bounds = |m: &Multiplicity| -> (u32, u32) {
        match m {
            Multiplicity::PureOne => (1, 1),
            Multiplicity::ZeroOrOne => (0, 1),
            Multiplicity::ZeroOrMany => (0, u32::MAX),
            Multiplicity::OneOrMany => (1, u32::MAX),
            Multiplicity::Range { lower, upper } => (*lower, upper.unwrap_or(u32::MAX)),
            Multiplicity::Variable(_) => (0, u32::MAX),
        }
    };
    let (a_lo, a_hi) = bounds(a);
    let (b_lo, b_hi) = bounds(b);
    let lo = a_lo.saturating_mul(b_lo);
    let hi = if a_hi == u32::MAX || b_hi == u32::MAX {
        u32::MAX
    } else {
        a_hi.saturating_mul(b_hi)
    };
    match (lo, hi) {
        (1, 1) => Multiplicity::PureOne,
        (0, 1) => Multiplicity::ZeroOrOne,
        (1, u32::MAX) => Multiplicity::OneOrMany,
        (0, u32::MAX) => Multiplicity::ZeroOrMany,
        (l, u) if u == u32::MAX => Multiplicity::Range {
            lower: l,
            upper: None,
        },
        (l, u) => Multiplicity::Range {
            lower: l,
            upper: Some(u),
        },
    }
}

/// Builds the `map(receiver, λ{v_automap | property_call(v_automap, ...)})`
/// expression that replaces a property/QP call on a non-toOne receiver.
/// Mirrors Java's `FunctionExpressionProcessor.buildLambdaForMapWithProperty`.
fn build_automap_rewrite(
    model: &PureModel,
    mut data: FunctionCallData,
    target_type: &ResolvedType,
    property_return_type: &ResolvedType,
    is_qualified: bool,
    source_info: legend_pure_parser_ast::SourceInfo,
) -> ExprKind {
    // Receiver-element type (multiplicity stripped to [1]).
    let elem_type_expr = target_type.type_expr.clone();

    // Take the receiver out of `data.arguments`; remaining entries
    // are QP arguments (empty for simple property access).
    let receiver = data.arguments.remove(0);
    let qp_args = data.arguments;
    let property_name = data.function_name;

    // `v_automap` parameter expression — sentinel name matches Java's
    // `Automap.AUTOMAP_LAMBDA_VARIABLE_NAME`.
    let v_automap_name = SmolStr::new("v_automap");
    let v_automap_var = ValueSpec {
        kind: Box::new(ExprKind::Variable {
            name: v_automap_name.clone(),
        }),
        source_info: source_info.clone(),
        type_info: Some(Box::new(ResolvedType {
            type_expr: elem_type_expr.clone(),
            multiplicity: Multiplicity::PureOne,
        })),
    };

    // Lambda body: `v_automap.<property>(...)` — same shape as the
    // original call, just with the receiver replaced by v_automap.
    let mut body_args = Vec::with_capacity(qp_args.len() + 1);
    body_args.push(v_automap_var);
    body_args.extend(qp_args);
    let body_call_data = FunctionCallData {
        function: None,
        function_name: property_name,
        arguments: body_args,
    };
    let body_kind = if is_qualified {
        ExprKind::QualifiedPropertyCall(body_call_data)
    } else {
        ExprKind::PropertyCall(body_call_data)
    };
    let body_spec = ValueSpec {
        kind: Box::new(body_kind),
        source_info: source_info.clone(),
        type_info: Some(Box::new(property_return_type.clone())),
    };

    // Lambda: single parameter `v_automap: <elem>[1]`.
    let lambda_param = Parameter {
        name: v_automap_name,
        type_expr: elem_type_expr,
        multiplicity: Multiplicity::PureOne,
        source_info: source_info.clone(),
    };
    let lambda_spec = ValueSpec {
        kind: Box::new(ExprKind::Lambda {
            parameters: vec![lambda_param],
            body: vec![body_spec],
        }),
        source_info: source_info.clone(),
        type_info: None,
    };

    // Resolve `meta::pure::functions::collection::map`. Returns None if
    // the platform isn't loaded — leaves `function: None` and lets
    // downstream resolution surface the issue.
    let map_id = model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("functions"),
        SmolStr::new("collection"),
        SmolStr::new("map"),
    ]);

    ExprKind::FunctionCall(FunctionCallData {
        function: map_id,
        function_name: SmolStr::new("map"),
        arguments: vec![receiver, lambda_spec],
    })
}

/// Looks up `property_name` on `target_type`'s class (and supertypes /
/// associations); emits `UnknownProperty` if not found and the receiver
/// isn't a bootstrap-chunk metatype. For QP overloads matched by name
/// (no-paren property access on a 0-arg QP), picks the 0-arg
/// candidate's return type.
fn infer_simple_property(
    ctx: &mut InferCtx<'_>,
    target_type: Option<&ResolvedType>,
    property: &SmolStr,
    expr_source_info: &legend_pure_parser_ast::SourceInfo,
) -> Option<ResolvedType> {
    let lookup = infer_property_access(ctx, target_type, property);
    match lookup {
        PropertyLookup::FoundProperty(rt) => Some(rt),
        PropertyLookup::FoundQualifiedProperties(mut candidates) => {
            // No-paren property access on a QP — pick the 0-arg
            // overload if one exists; otherwise return the first
            // candidate's return type without firing arity errors
            // (mirrors the prior best-effort behaviour for `obj.qp`
            // when qp expects args).
            let idx = candidates
                .iter()
                .position(|c| c.parameters.is_empty())
                .unwrap_or(0);
            let chosen = candidates.swap_remove(idx);
            Some(chosen.return_type)
        }
        PropertyLookup::NotFound { type_name } => {
            ctx.errors.push(CompilationError {
                message: format!(
                    "The property '{property}' can't be found in the type \
                     '{type_name}' (or any supertype)"
                ),
                source_info: expr_source_info.clone(),
                kind: CompilationErrorKind::UnknownProperty {
                    type_name,
                    property_name: property.clone(),
                },
            });
            None
        }
        PropertyLookup::UnknownTarget => None,
    }
}

/// Inference helper for qualified property invocation
/// (`$x.qp(arg1, arg2)`). Shared by the legacy
/// `ExprKind::QualifiedPropertyAccess` arm and the new
/// `ExprKind::QualifiedPropertyCall(FunctionCallData { .. })` arm.
///
/// Performs the same overload-by-arity resolution and per-argument
/// type/multiplicity validation as the legacy path.
/// `qp_arguments` and `qp_arg_types` exclude the receiver.
fn infer_qualified_property(
    ctx: &mut InferCtx<'_>,
    target_type: Option<&ResolvedType>,
    property: &SmolStr,
    qp_arguments: &[ValueSpec],
    qp_arg_types: &[Option<ResolvedType>],
    expr_source_info: &legend_pure_parser_ast::SourceInfo,
) -> Option<ResolvedType> {
    let lookup = infer_property_access(ctx, target_type, property);
    match lookup {
        PropertyLookup::FoundQualifiedProperties(candidates) => {
            Some(resolve_qualified_property_overload(
                ctx,
                candidates,
                property,
                qp_arguments,
                qp_arg_types,
                expr_source_info,
            ))
        }
        PropertyLookup::FoundProperty(rt) => {
            // QP-style call against a regular property — the runtime
            // would error; treat the property's type as the result and
            // let runtime surface the mismatch.
            Some(rt)
        }
        PropertyLookup::NotFound { type_name } => {
            ctx.errors.push(CompilationError {
                message: format!(
                    "The property '{property}' can't be found in the type \
                     '{type_name}' (or any supertype)"
                ),
                source_info: expr_source_info.clone(),
                kind: CompilationErrorKind::UnknownProperty {
                    type_name,
                    property_name: property.clone(),
                },
            });
            None
        }
        PropertyLookup::UnknownTarget => None,
    }
}

/// Resolves `target.property` against the type model.
///
/// Walks the receiver class plus its supertypes, looking at declared
/// properties, qualified properties, and association-injected properties
/// (via the derived index). Returns a [`PropertyLookup`] describing the
/// outcome — callers decide whether to emit an error.
fn infer_property_access(
    ctx: &InferCtx<'_>,
    target_type: Option<&ResolvedType>,
    property_name: &str,
) -> PropertyLookup {
    let Some(target) = target_type else {
        return PropertyLookup::UnknownTarget;
    };

    // Receiver must be a `Named` type whose element resolves to a Class.
    let (receiver_id, receiver_type_args) = match &target.type_expr {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => (*element, type_arguments.clone()),
        _ => return PropertyLookup::UnknownTarget,
    };
    let Some(receiver_elem) = ctx.model.try_get_element(receiver_id) else {
        return PropertyLookup::UnknownTarget;
    };
    if !matches!(receiver_elem, Element::Class(_)) {
        return PropertyLookup::UnknownTarget;
    }
    let receiver_type_name = ctx.model.get_node(receiver_id).name.clone();

    // Build type-argument bindings for the immediate receiver class
    // (e.g., receiver `Pair<Integer,String>` with class `Pair<U,V>`
    // produces { U → Integer, V → String }).
    let receiver_bindings = compute_type_arg_bindings(ctx.model, receiver_id, &receiver_type_args);

    // Walk type hierarchy (own + association + supertypes) looking for
    // a member named `property_name`.
    let mut visited: std::collections::HashSet<ElementId> = std::collections::HashSet::new();
    if let Some(found) = lookup_member_in_class(
        ctx.model,
        receiver_id,
        property_name,
        &receiver_bindings,
        &receiver_type_name,
        &mut visited,
    ) {
        return found;
    }

    // Defer to runtime reflection ONLY when the receiver is a
    // parametric metatype carrier (Class<X>, Enumeration<X>,
    // Function<…>) or `Any`. For these:
    //   - Class<Person>.allInstances, MyEnum.RED, lambda-param-infers-Any
    //     all bring in members from the inner element X (or are simply
    //     unknown), and the compile-time lookup can't resolve them
    //     without a metatype-aware inner-element walk + improved
    //     lambda-param inference.
    //
    // Non-parametric chunk-0 classes (`GenericType`, `Property`,
    // `FunctionType`, `MultiplicityValue`, `SimpleFunctionExpression`,
    // …) are real M3 classes with declared properties — validate
    // them normally so typos like `pair(1,2)->genericType().rawTyp`
    // produce a compile error instead of silently returning Unit.
    if is_metatype_carrier(ctx.model, receiver_id) {
        return PropertyLookup::UnknownTarget;
    }

    PropertyLookup::NotFound {
        type_name: receiver_type_name,
    }
}

/// Returns `true` when the receiver class is a "metatype carrier" —
/// a parametric M3 metaclass that wraps an inner element type
/// (`Class<T>`, `Enumeration<T>`, `Function<...>`, etc.) — or `Any`.
/// These receivers participate in runtime reflection that the
/// compile-time lookup can't yet resolve.
///
/// Non-parametric chunk-0 classes (`GenericType`, `Property`, etc.)
/// are real M3 classes with declared properties — they validate
/// normally.
fn is_metatype_carrier(model: &PureModel, class_id: ElementId) -> bool {
    // `Any` — top type, concrete properties unknown (lambda-param-infers-Any).
    if class_id == bootstrap::ANY_ID {
        return true;
    }
    // Bootstrap-chunk class with type parameters — Class<T>,
    // Enumeration<T>, Function<…>, etc. User-defined parametric
    // classes (Pair<U,V>, List<T>, …) live in chunk 1+ and aren't
    // captured by this predicate.
    let is_bootstrap = matches!(
        class_id,
        ElementId::InstanceId {
            chunk_id: crate::bootstrap::BOOTSTRAP_CHUNK_ID,
            ..
        }
    );
    if !is_bootstrap {
        return false;
    }
    matches!(
        model.try_get_element(class_id),
        Some(Element::Class(c)) if !c.type_parameters.is_empty()
    )
}

/// Computes type-parameter → type-argument bindings for a class.
///
/// Returns an empty map when the class has no type parameters or the
/// receiver carried no type arguments.
fn compute_type_arg_bindings(
    model: &PureModel,
    class_id: ElementId,
    type_arguments: &[TypeExpr],
) -> HashMap<SmolStr, TypeExpr> {
    let mut out = HashMap::new();
    if let Some(Element::Class(class)) = model.try_get_element(class_id) {
        for (param_name, arg) in class.type_parameters.iter().zip(type_arguments.iter()) {
            out.insert(param_name.clone(), arg.clone());
        }
    }
    out
}

/// Recursively searches `class_id` and its supertypes (including
/// association-injected properties) for a member named `property_name`.
///
/// Returns a [`PropertyLookup`] describing the kind of match. Qualified
/// properties may have multiple overloads at the same class (Pure
/// supports overload-by-arity for QPs); all of them are returned so the
/// caller can pick by arity. Bindings are threaded through the supertype
/// chain so a parametric supertype (e.g., `Foo<T> extends List<T>`)
/// substitutes `T` when looking up properties on `List`.
fn lookup_member_in_class(
    model: &PureModel,
    class_id: ElementId,
    property_name: &str,
    bindings: &HashMap<SmolStr, TypeExpr>,
    receiver_type_name: &SmolStr,
    visited: &mut std::collections::HashSet<ElementId>,
) -> Option<PropertyLookup> {
    if !visited.insert(class_id) {
        return None;
    }
    let Some(Element::Class(class)) = model.try_get_element(class_id) else {
        return None;
    };

    // 1. Own declared properties.
    if let Some(prop) = class.properties.iter().find(|p| p.name == property_name) {
        let resolved = ResolvedType {
            type_expr: crate::resolve::substitute_type(&prop.type_expr, bindings),
            multiplicity: prop.multiplicity.clone(),
        };
        return Some(PropertyLookup::FoundProperty(resolved));
    }

    // 2. Own qualified properties — collect ALL overloads with this name.
    let qp_overloads: Vec<&crate::nodes::class::QualifiedProperty> = class
        .qualified_properties
        .iter()
        .filter(|q| q.name == property_name)
        .collect();
    if !qp_overloads.is_empty() {
        let candidates: Vec<QpCandidate> = qp_overloads
            .into_iter()
            .map(|qp| QpCandidate {
                return_type: ResolvedType {
                    type_expr: crate::resolve::substitute_type(&qp.return_type, bindings),
                    multiplicity: qp.return_multiplicity.clone(),
                },
                parameters: qp.parameters.clone(),
                bindings: bindings.clone(),
                receiver_type_name: receiver_type_name.clone(),
            })
            .collect();
        return Some(PropertyLookup::FoundQualifiedProperties(candidates));
    }

    // 3. Association-injected properties. The derived index registers
    //    each association property on its OWN target class; the property
    //    visible from this class for navigation is the OTHER end (index
    //    `1 - prop_idx_pointing_to_self`). Same convention as
    //    `runtime/native/lang.rs:832`.
    for (assoc_id, prop_idx_pointing_to_self) in model.association_properties(class_id) {
        if let Some(Element::Association(assoc)) = model.try_get_element(*assoc_id)
            && assoc.properties.len() == 2
        {
            let injected = &assoc.properties[1 - *prop_idx_pointing_to_self];
            if injected.name == property_name {
                let resolved = ResolvedType {
                    type_expr: crate::resolve::substitute_type(&injected.type_expr, bindings),
                    multiplicity: injected.multiplicity.clone(),
                };
                return Some(PropertyLookup::FoundProperty(resolved));
            }
        }
    }

    // 4. Supertypes — thread bindings through any parametric supertype.
    let super_types = class.super_types.clone();
    for st in &super_types {
        if let TypeExpr::Named {
            element: super_id,
            type_arguments: super_args,
            ..
        } = st
        {
            // Substitute current bindings into the supertype's type args
            // so generics carry through (`Foo<T> extends Bar<List<T>>`
            // looks up properties on Bar with X → List<T_resolved>).
            let substituted_args: Vec<TypeExpr> = super_args
                .iter()
                .map(|a| crate::resolve::substitute_type(a, bindings))
                .collect();
            let super_bindings = compute_type_arg_bindings(model, *super_id, &substituted_args);
            if let Some(found) = lookup_member_in_class(
                model,
                *super_id,
                property_name,
                &super_bindings,
                receiver_type_name,
                visited,
            ) {
                return Some(found);
            }
        }
    }

    None
}

/// Resolves a qualified-property overload at a call site against the
/// declared QP candidates: pick the unique arity match, then validate
/// per-argument type and multiplicity.
///
/// Returns the resolved return type. Errors are pushed to `ctx.errors`.
/// Mirrors the rigour of function-call dispatch in `resolve.rs`.
fn resolve_qualified_property_overload(
    ctx: &mut InferCtx<'_>,
    mut candidates: Vec<QpCandidate>,
    property_name: &SmolStr,
    arguments: &[ValueSpec],
    arg_types: &[Option<ResolvedType>],
    call_source_info: &legend_pure_parser_ast::SourceInfo,
) -> ResolvedType {
    // Arity match: pick the candidate whose parameter count equals the
    // call's argument count.
    let chosen_idx = candidates
        .iter()
        .position(|c| c.parameters.len() == arguments.len());

    let chosen = if let Some(idx) = chosen_idx {
        candidates.swap_remove(idx)
    } else {
        // No overload matches arity — emit one error using the
        // smallest-arity candidate (most likely the user intended that
        // overload). The receiver type name and other diagnostic fields
        // are identical across candidates for the same call site.
        let receiver_type_name = candidates[0].receiver_type_name.clone();
        let arities: Vec<usize> = candidates.iter().map(|c| c.parameters.len()).collect();
        let expected = *arities.iter().min().unwrap_or(&0);
        ctx.errors.push(CompilationError {
            message: format!(
                "Qualified property '{}.{}' expects {} argument(s), got {} \
                 (available overload arities: {:?})",
                receiver_type_name,
                property_name,
                expected,
                arguments.len(),
                arities,
            ),
            source_info: call_source_info.clone(),
            kind: CompilationErrorKind::QualifiedPropertyArityMismatch {
                type_name: receiver_type_name,
                property_name: property_name.clone(),
                expected,
                actual: arguments.len(),
            },
        });
        // Return the first candidate's return type so downstream
        // inference has something to work with.
        return candidates.swap_remove(0).return_type;
    };

    // Per-argument type + multiplicity check on the chosen overload.
    for (idx, (param, arg_ty)) in chosen.parameters.iter().zip(arg_types.iter()).enumerate() {
        let expected_type = crate::resolve::substitute_type(&param.type_expr, &chosen.bindings);
        let arg_eid = arg_ty.as_ref().and_then(|rt| match &rt.type_expr {
            TypeExpr::Named { element, .. } => Some(*element),
            _ => None,
        });
        let arg_mult = arg_ty.as_ref().map(|rt| &rt.multiplicity);

        let type_ok = crate::resolve::is_type_compatible(arg_eid, &expected_type, ctx.model);
        let mult_ok = crate::resolve::is_multiplicity_compatible(arg_mult, &param.multiplicity);

        if !type_ok || !mult_ok {
            let expected = render_type(ctx.model, &expected_type, &param.multiplicity);
            let actual = arg_ty.as_ref().map_or_else(
                || SmolStr::new("<unknown>"),
                |rt| render_type(ctx.model, &rt.type_expr, &rt.multiplicity),
            );
            ctx.errors.push(CompilationError {
                message: format!(
                    "Qualified property '{}.{}' argument {} ('{}'): expected '{}', got '{}'",
                    chosen.receiver_type_name, property_name, idx, param.name, expected, actual
                ),
                source_info: arguments
                    .get(idx)
                    .map_or_else(|| call_source_info.clone(), |a| a.source_info.clone()),
                kind: CompilationErrorKind::QualifiedPropertyArgTypeMismatch {
                    type_name: chosen.receiver_type_name.clone(),
                    property_name: property_name.clone(),
                    param_index: idx,
                    param_name: param.name.clone(),
                    expected,
                    actual,
                },
            });
        }
    }

    chosen.return_type
}

/// Renders a `TypeExpr` + `Multiplicity` as a Pure-style string like
/// `Integer[1]` or `Foo<Bar>[*]` for use in diagnostic messages.
fn render_type(model: &PureModel, type_expr: &TypeExpr, multiplicity: &Multiplicity) -> SmolStr {
    let mut s = String::new();
    render_type_expr(model, type_expr, &mut s);
    s.push('[');
    s.push_str(&render_multiplicity(multiplicity));
    s.push(']');
    SmolStr::new(s)
}

fn render_type_expr(model: &PureModel, type_expr: &TypeExpr, out: &mut String) {
    match type_expr {
        TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            out.push_str(&model.get_node(*element).name);
            if !type_arguments.is_empty() {
                out.push('<');
                for (i, arg) in type_arguments.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    render_type_expr(model, arg, out);
                }
                out.push('>');
            }
        }
        TypeExpr::Generic(name) => out.push_str(name),
        TypeExpr::FunctionType { .. } => out.push_str("<FunctionType>"),
        TypeExpr::AlgebraUnion(a, b) => {
            render_type_expr(model, a, out);
            out.push_str(" | ");
            render_type_expr(model, b, out);
        }
        TypeExpr::Relation(_) => out.push_str("<Relation>"),
    }
}

fn render_multiplicity(m: &Multiplicity) -> String {
    match m {
        Multiplicity::PureOne => "1".to_string(),
        Multiplicity::ZeroOrOne => "0..1".to_string(),
        Multiplicity::ZeroOrMany => "*".to_string(),
        Multiplicity::OneOrMany => "1..*".to_string(),
        Multiplicity::Range { lower, upper } => match upper {
            Some(u) if u == lower => format!("{lower}"),
            Some(u) => format!("{lower}..{u}"),
            None => format!("{lower}..*"),
        },
        Multiplicity::Variable(v) => v.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Creates a `ResolvedType` for a primitive type with multiplicity `[1]`.
fn primitive(element_id: crate::ids::ElementId) -> ResolvedType {
    ResolvedType {
        type_expr: TypeExpr::Named {
            element: element_id,
            type_arguments: Vec::new(),
            value_arguments: Vec::new(),
        },
        multiplicity: Multiplicity::PureOne,
    }
}

/// Returns the resolved type for a date literal based on its variant.
fn date_literal_type(dv: &DateValue) -> ResolvedType {
    match dv {
        DateValue::StrictDate { .. } => primitive(bootstrap::STRICT_DATE_ID),
        DateValue::DateTime { .. } => primitive(bootstrap::DATE_TIME_ID),
        DateValue::StrictTime { .. } => primitive(bootstrap::STRICT_TIME_ID),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap;
    use crate::types::{ExprKind, Multiplicity, TypeExpr, ValueSpec};
    use legend_pure_parser_ast::SourceInfo;
    use smol_str::SmolStr;

    fn si() -> SourceInfo {
        SourceInfo::new("test.pure", 1, 1, 1, 10)
    }

    fn named_type(id: crate::ids::ElementId) -> TypeExpr {
        TypeExpr::Named {
            element: id,
            type_arguments: Vec::new(),
            value_arguments: Vec::new(),
        }
    }

    fn model_with_bootstrap() -> PureModel {
        let mut model = PureModel::new();
        let bootstrap = bootstrap::create_bootstrap_chunk(model.root_package);
        model.chunks.push(bootstrap);
        model
    }

    /// Helper: create a `ValueSpec` with no type info.
    fn untyped(kind: ExprKind, source_info: SourceInfo) -> ValueSpec {
        ValueSpec {
            kind: Box::new(kind),
            source_info,
            type_info: None,
        }
    }

    #[test]
    fn infer_integer_literal() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(ExprKind::IntegerLiteral(42), si())];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        assert!(errors.is_empty());
        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::INTEGER_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_string_literal() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(
            ExprKind::StringLiteral(SmolStr::new("hello")),
            si(),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_boolean_literal() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(ExprKind::BooleanLiteral(true), si())];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::BOOLEAN_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_variable_from_param() {
        let model = model_with_bootstrap();
        let params = vec![Parameter {
            name: SmolStr::new("x"),
            type_expr: named_type(bootstrap::STRING_ID),
            multiplicity: Multiplicity::PureOne,
            source_info: SourceInfo::new("test.pure", 1, 1, 1, 5),
        }];

        let mut body = vec![untyped(
            ExprKind::Variable {
                name: SmolStr::new("x"),
            },
            SourceInfo::new("test.pure", 2, 1, 2, 3),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &params, &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    #[test]
    fn infer_collection() {
        let model = model_with_bootstrap();
        let si1 = SourceInfo::new("test.pure", 1, 2, 1, 3);
        let si2 = SourceInfo::new("test.pure", 1, 5, 1, 6);
        let si3 = SourceInfo::new("test.pure", 1, 8, 1, 9);
        let coll_si = SourceInfo::new("test.pure", 1, 1, 1, 10);

        let mut body = vec![untyped(
            ExprKind::Collection {
                elements: vec![
                    untyped(ExprKind::IntegerLiteral(1), si1),
                    untyped(ExprKind::IntegerLiteral(2), si2),
                    untyped(ExprKind::IntegerLiteral(3), si3),
                ],
            },
            coll_si,
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::INTEGER_ID));
        assert_eq!(
            ti.multiplicity,
            Multiplicity::Range {
                lower: 3,
                upper: Some(3)
            }
        );
    }

    #[test]
    fn infer_lambda() {
        let model = model_with_bootstrap();
        let lambda_si = SourceInfo::new("test.pure", 1, 1, 1, 30);

        let mut body = vec![untyped(
            ExprKind::Lambda {
                parameters: vec![Parameter {
                    name: SmolStr::new("x"),
                    type_expr: named_type(bootstrap::STRING_ID),
                    multiplicity: Multiplicity::PureOne,
                    source_info: SourceInfo::new("test.pure", 1, 2, 1, 15),
                }],
                body: vec![untyped(
                    ExprKind::Variable {
                        name: SmolStr::new("x"),
                    },
                    SourceInfo::new("test.pure", 1, 20, 1, 22),
                )],
            },
            lambda_si,
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        match &ti.type_expr {
            TypeExpr::FunctionType {
                parameters,
                return_type,
                return_multiplicity,
            } => {
                assert_eq!(parameters.len(), 1);
                assert_eq!(parameters[0].0, named_type(bootstrap::STRING_ID));
                assert_eq!(**return_type, named_type(bootstrap::STRING_ID));
                assert_eq!(*return_multiplicity, Multiplicity::PureOne);
            }
            other => panic!("Expected FunctionType, got {other:?}"),
        }
    }

    #[test]
    fn infer_date_literals() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(
            ExprKind::DateLiteral(DateValue::StrictDate {
                year: 2024,
                month: Some(1),
                day: Some(15),
            }),
            si(),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRICT_DATE_ID));
    }

    #[test]
    fn infer_enum_value() {
        let model = model_with_bootstrap();
        let enum_id = crate::ids::ElementId::InstanceId {
            chunk_id: 1,
            local_idx: 0,
        };

        let mut body = vec![untyped(
            ExprKind::EnumValue {
                enum_element: enum_id,
                value: SmolStr::new("VALUE_A"),
            },
            si(),
        )];
        let mut errors = Vec::new();

        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(enum_id));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }
}
