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
    /// Type-parameter names declared on the enclosing function. A
    /// `Generic(name)` whose `name` is in this set is *in scope*, not
    /// unbound — it's a transitive generic from the outer signature
    /// and must not surface as `UnresolvedTypeParameter`. Mirrors
    /// Java's `TypeInferenceContext.getParent() == null` guard at
    /// `TypeInference.java:87-89`. Seeded at `infer_function_body`
    /// entry by walking the enclosing fn's parameter types + return
    /// type for `Generic(_)` references.
    #[allow(dead_code)] // wired in Step 3g (strict-mode flag)
    type_params_in_scope: std::collections::HashSet<SmolStr>,
    /// Multiplicity-parameter names declared on the enclosing
    /// function (`Variable(name)` shapes appearing in params/return).
    /// Same role as `type_params_in_scope` for the multiplicity side
    /// — `TypeInference.java:102` ("multiplicity parameter X was not
    /// resolved") only fires for genuine top-level unboundeds.
    #[allow(dead_code)] // wired in Step 3g (strict-mode flag)
    mult_params_in_scope: std::collections::HashSet<SmolStr>,
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
///
/// Public so external compiler extensions (`crates/dsl-mapping`,
/// future `crates/dsl-relational`, …) can run inference on lambda
/// bodies they own — the regular pipeline (`Pass 2b'`) calls this for
/// every M3 function body, and DSL extensions need the same surface
/// to validate user-supplied expressions (filter clauses, transform
/// expressions, mapping property bodies). The contract is:
///
/// 1. `body` must already be lowered to [`ValueSpec`]s. Use the
///    extension API in `crates/pure` (Pass 2b' lowering helpers,
///    promoted as needed in future stages) to lower from AST.
/// 2. `params` provides the variable bindings visible at the start of
///    the body (e.g. the lambda's parameters). `Scope::from_params`
///    builds the root scope.
/// 3. The function mutates `body` in place — every successfully
///    inferred expression has `type_info` populated. Read it with
///    `body[i].type_info.as_ref()` after the call.
/// 4. Errors append to `errors` rather than aborting; partial
///    inference results are still observable.
pub fn infer_function_body(
    model: &PureModel,
    params: &[Parameter],
    body: &mut [ValueSpec],
    errors: &mut Vec<CompilationError>,
) {
    let root_scope = Scope::from_params(params);
    let mut type_params_in_scope = std::collections::HashSet::new();
    let mut mult_params_in_scope = std::collections::HashSet::new();
    for p in params {
        harvest_in_scope_generics(
            &p.type_expr,
            &p.multiplicity,
            &mut type_params_in_scope,
            &mut mult_params_in_scope,
        );
    }
    let mut ctx = InferCtx {
        model,
        scopes: vec![root_scope],
        errors,
        type_params_in_scope,
        mult_params_in_scope,
    };

    for expr in body.iter_mut() {
        infer_expr(&mut ctx, expr);
    }
}

/// Walk a parameter's type+multiplicity expressions and record every
/// `Generic(name)` and `Variable(name)` that appears, treating those
/// names as in-scope for the enclosing function's body. Used to seed
/// `InferCtx::type_params_in_scope` and `mult_params_in_scope` so the
/// post-dispatch unresolved-generic check (Java parity
/// `TypeInference.java:87-102`) doesn't fire on transitive generics.
fn harvest_in_scope_generics(
    ty: &TypeExpr,
    mult: &Multiplicity,
    types: &mut std::collections::HashSet<SmolStr>,
    mults: &mut std::collections::HashSet<SmolStr>,
) {
    walk_type_for_generics(ty, types, mults);
    if let Multiplicity::Variable(name) = mult {
        mults.insert(name.clone());
    }
}

fn walk_type_for_generics(
    ty: &TypeExpr,
    types: &mut std::collections::HashSet<SmolStr>,
    mults: &mut std::collections::HashSet<SmolStr>,
) {
    match ty {
        TypeExpr::Generic(name) => {
            types.insert(name.clone());
        }
        TypeExpr::Named {
            type_arguments,
            multiplicity_arguments,
            ..
        } => {
            for ta in type_arguments {
                walk_type_for_generics(ta, types, mults);
            }
            for ma in multiplicity_arguments {
                if let Multiplicity::Variable(name) = ma {
                    mults.insert(name.clone());
                }
            }
        }
        TypeExpr::FunctionType {
            parameters,
            return_type,
            return_multiplicity,
        } => {
            for (p, m) in parameters {
                walk_type_for_generics(p, types, mults);
                if let Multiplicity::Variable(name) = m {
                    mults.insert(name.clone());
                }
            }
            walk_type_for_generics(return_type, types, mults);
            if let Multiplicity::Variable(name) = return_multiplicity {
                mults.insert(name.clone());
            }
        }
        TypeExpr::AlgebraUnion(a, b) => {
            walk_type_for_generics(a, types, mults);
            walk_type_for_generics(b, types, mults);
        }
        TypeExpr::Relation(cols) => {
            for c in cols {
                walk_type_for_generics(&c.type_expr, types, mults);
                if let Multiplicity::Variable(name) = &c.multiplicity {
                    mults.insert(name.clone());
                }
            }
        }
        TypeExpr::Unresolved => {}
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
        ExprKind::Variable { name } => {
            let resolved = ctx.lookup_var(name).cloned();
            if resolved.is_none() {
                // Undeclared variable. Surface a `UnresolvedElement`
                // diagnostic so the IDE can red-squiggle the use-site.
                //
                // Implicit variables like `$this` must be added to
                // the scope by the caller (e.g. `pass_infer` injecting
                // a `this` Parameter when entering a class QP body /
                // constraint expression / property default-value).
                // No name-based whitelisting here.
                ctx.errors.push(crate::error::CompilationError {
                    message: format!("Variable '${name}' is not declared in scope"),
                    source_info: expr.source_info.clone(),
                    kind: crate::error::CompilationErrorKind::UnresolvedElement {
                        path: SmolStr::from(format!("${name}")),
                    },
                });
            }
            resolved
        }

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

            let arg_source_infos: Vec<legend_pure_parser_ast::SourceInfo> =
                arguments.iter().map(|a| a.source_info.clone()).collect();
            let result = infer_function_call(
                ctx,
                *function,
                function_name,
                let_name,
                arguments,
                &arg_types,
                &arg_source_infos,
            );
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
        // place to `map(receiver, λ{v_automap | property_call(v_automap,...)})`.
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
                multiplicity_arguments: Vec::new(),
                value_arguments: Vec::new(),
                source_info: None,
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
                        multiplicity_arguments: Vec::new(),
                        value_arguments: Vec::new(),
                        source_info: None,
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

            // LUB across all element types — `[1, 1.5]` widens to
            // `Number`, not `Integer`. Without this, body-return
            // checks against a literal collection (and downstream
            // dispatch on the collection's element type) silently
            // accepted heterogeneous numeric literals as the
            // first element's type.
            //
            // Preserve type_arguments when both elements share the
            // same head element id and arity. `[pair(1,'a'), pair(2,'b')]`
            // must keep its `Pair<Integer,String>` shape so callers
            // like `newMap<U,V>(pairs:Pair<U,V>[*])` can bind U/V from
            // the collection arg's parametric form. Drop type_args
            // only when elements have different head elements (the
            // hierarchy LUB takes over there).
            let type_expr = elem_types
                .iter()
                .flatten()
                .map(|t| t.type_expr.clone())
                .reduce(|acc, te| match (&acc, &te) {
                    (
                        TypeExpr::Named {
                            element: a,
                            type_arguments: a_args,
                            multiplicity_arguments: a_margs,
                            ..
                        },
                        TypeExpr::Named {
                            element: b,
                            type_arguments: b_args,
                            multiplicity_arguments: b_margs,
                            ..
                        },
                    ) => {
                        if a == b
                            && a_args.len() == b_args.len()
                            && a_margs.len() == b_margs.len()
                            && a_args == b_args
                            && a_margs == b_margs
                        {
                            // Same shape — preserve fully.
                            acc
                        } else {
                            let lub_id = crate::resolve::least_upper_bound_ids(*a, *b, ctx.model);
                            TypeExpr::Named {
                                element: lub_id,
                                type_arguments: Vec::new(),
                                multiplicity_arguments: Vec::new(),
                                value_arguments: Vec::new(),
                                source_info: None,
                            }
                        }
                    }
                    _ => acc,
                })
                .unwrap_or_else(|| TypeExpr::Named {
                    element: bootstrap::NIL_ID,
                    type_arguments: Vec::new(),
                    multiplicity_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                    source_info: None,
                });

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

        // -- Multiplicity reference -----------------------------------------
        // `@[m]` already has eager `type_info = Any[m]` set at lowering
        // time (`lower_multiplicity_reference`); pass through whatever
        // the lowering pinned. Fallback for safety only.
        ExprKind::MultiplicityReference { multiplicity } => Some(ResolvedType {
            type_expr: TypeExpr::Named {
                element: bootstrap::ANY_ID,
                type_arguments: vec![],
                multiplicity_arguments: vec![],
                value_arguments: vec![],
                source_info: None,
            },
            multiplicity: multiplicity.clone(),
        }),

        // -- Element reference ----------------------------------------------
        //
        // A bare element reference's *type* is its M3 metatype, not
        // the element itself. `Class<Foo>` referenced as a value has
        // type `meta::pure::metamodel::type::Class`; a function name
        // used as a value (e.g. `reverse_T_m__T_m_->eval([1,2,3])`)
        // has type `meta::pure::metamodel::function::NativeFunctionDefinition`
        // (or `ConcreteFunctionDefinition`). This mirrors what
        // `resolve.rs::infer_type_from_valuespec` returns at dispatch
        // time, keeping Pass 2.5 and dispatch in sync.
        //
        // Without this alignment, the arg-vs-param check would see
        // arg-eid = the function's id and param-eid = the Function
        // metaclass, fail `is_type_compatible`, and require a
        // specialised Function-element skip — exactly the kind of
        // gate the user pushed back on.
        ExprKind::PackageableElementRef { element } => {
            let metatype_eid = ctx
                .model
                .try_get_element(*element)
                .and_then(|e| bootstrap::metatype_of(ctx.model, e))
                .unwrap_or(*element);
            Some(ResolvedType {
                type_expr: TypeExpr::Named {
                    element: metatype_eid,
                    type_arguments: Vec::new(),
                    multiplicity_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                    source_info: None,
                },
                multiplicity: Multiplicity::PureOne,
            })
        }

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
                    multiplicity_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                    source_info: None,
                },
                multiplicity: Multiplicity::PureOne,
            }),
        ExprKind::ColSpecArrayLiteral { kind, .. } => {
            let class_name = match kind {
                crate::types::ColSpecLiteralKind::Plain => "ColSpecArray",
                crate::types::ColSpecLiteralKind::Func => "FuncColSpecArray",
                crate::types::ColSpecLiteralKind::Agg => "AggColSpecArray",
            };
            ctx.model
                .resolve_by_path(&[
                    smol_str::SmolStr::new("meta"),
                    smol_str::SmolStr::new("pure"),
                    smol_str::SmolStr::new("metamodel"),
                    smol_str::SmolStr::new("relation"),
                    smol_str::SmolStr::new(class_name),
                ])
                .map(|element| ResolvedType {
                    type_expr: TypeExpr::Named {
                        element,
                        type_arguments: Vec::new(),
                        multiplicity_arguments: Vec::new(),
                        value_arguments: Vec::new(),
                        source_info: None,
                    },
                    multiplicity: Multiplicity::PureOne,
                })
        }
        ExprKind::ColSpecLiteral { kind, .. } => {
            let class_name = match kind {
                crate::types::ColSpecLiteralKind::Plain => "ColSpec",
                crate::types::ColSpecLiteralKind::Func => "FuncColSpec",
                crate::types::ColSpecLiteralKind::Agg => "AggColSpec",
            };
            ctx.model
                .resolve_by_path(&[
                    smol_str::SmolStr::new("meta"),
                    smol_str::SmolStr::new("pure"),
                    smol_str::SmolStr::new("metamodel"),
                    smol_str::SmolStr::new("relation"),
                    smol_str::SmolStr::new(class_name),
                ])
                .map(|element| ResolvedType {
                    type_expr: TypeExpr::Named {
                        element,
                        type_arguments: Vec::new(),
                        multiplicity_arguments: Vec::new(),
                        value_arguments: Vec::new(),
                        source_info: None,
                    },
                    multiplicity: Multiplicity::PureOne,
                })
        }

        // -- Path literal ---------------------------------------------------
        // Stage 3 minimum: type as a bare `Path` instance. Full
        // `Path<U,V|m>` parametric inference (chain return type +
        // multiplicity product) is deferred — would require walking
        // each step's resolved property and propagating type-args
        // through the running class. Pass 2.5 currently leaves
        // type-args empty; runtime evaluation is what consumers
        // actually depend on (Stage 4).
        ExprKind::PathLiteral { .. } => ctx
            .model
            .resolve_by_path(&[
                smol_str::SmolStr::new("meta"),
                smol_str::SmolStr::new("pure"),
                smol_str::SmolStr::new("metamodel"),
                smol_str::SmolStr::new("path"),
                smol_str::SmolStr::new("Path"),
            ])
            .map(|element| ResolvedType {
                type_expr: TypeExpr::Named {
                    element,
                    type_arguments: Vec::new(),
                    multiplicity_arguments: Vec::new(),
                    value_arguments: Vec::new(),
                    source_info: None,
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
/// Flattens the `InferCtx` scope chain (innermost-last winner) into a
/// `VarTypes` map suitable for `resolve::infer_generic_bindings`.
fn collect_var_types(ctx: &InferCtx<'_>) -> crate::resolve::VarTypes {
    let mut out = crate::resolve::VarTypes::new();
    for scope in &ctx.scopes {
        for (name, rt) in &scope.bindings {
            out.insert(
                name.clone(),
                (rt.type_expr.clone(), rt.multiplicity.clone()),
            );
        }
    }
    out
}

fn infer_function_call(
    ctx: &mut InferCtx<'_>,
    function: Option<crate::ids::ElementId>,
    function_name: &SmolStr,
    let_name: Option<(&SmolStr, &legend_pure_parser_ast::SourceInfo)>,
    arguments: &[ValueSpec],
    arg_types: &[Option<ResolvedType>],
    arg_source_infos: &[legend_pure_parser_ast::SourceInfo],
) -> Option<ResolvedType> {
    // Phase 0: `letFunction` is a side-effect form — binds a variable
    // into scope and returns `Nil[0]`. Routed via its own helper so the
    // user-fn dispatch path below stays focused on the binding +
    // substitution pipeline.
    if let Some(let_result) = process_let_function_call(ctx, function_name, let_name, arg_types) {
        return Some(let_result);
    }

    // Phase 1: resolved user function — bind generics, validate args,
    // substitute the return signature.
    //
    // The substitution feeds on each arg's `arg_ty.type_expr`. That
    // type_expr must carry the parametric shape — e.g. `Named{LA_List,
    // [String]}` — for binding to work. Capturing parametric info into
    // `type_info` at the lowering layer (so `^LA_List<String>(...)` and
    // `cast(@LA_List<String>)` and `extends LA_List<String>` all share
    // the same type-info plumbing) is the upstream prerequisite. This
    // helper just consumes whatever `arg_ty.type_expr` already carries.
    if let Some(Element::Function(f)) = function.and_then(|id| ctx.model.try_get_element(id)) {
        // Bindings up front: needed both for substituting the param
        // types we're about to check arguments against AND for
        // substituting the function's return signature. Computing
        // them once before the per-arg loop also lets us validate
        // generic param shapes (`param: T[n]`) against their
        // resolved binding (`Integer[1]`) instead of their raw
        // `Generic(T)` placeholder, which `is_type_compatible`
        // treats as permissive.
        let var_types = collect_var_types(ctx);
        let bindings =
            crate::resolve::infer_generic_bindings(&f.parameters, arguments, ctx.model, &var_types);

        validate_call_arguments(
            ctx,
            function,
            function_name,
            &f.parameters,
            arg_types,
            arg_source_infos,
            &bindings,
        );

        // Reuse the up-front `bindings` to substitute the function's
        // declared return signature. The lambda second-pass
        // (`infer_generic_bindings` runs over `Function<{T->V}>` slots
        // and binds V from the lambda body's last expression) is part
        // of `bindings` already, so `map<T,V>(coll:T[*],
        // pred:Function<{T[1]->V[*]}>[1]):V[*]` returns a substituted
        // `Named{V_resolved}[*]` here.
        let type_expr = bindings.make_concrete_type(&f.return_type);
        let multiplicity = bindings.make_concrete_mult(&f.return_multiplicity);

        // Java parity (lenient): we don't fire
        // `TypeInference.java:87-89`'s "type parameter X was not
        // resolved" error here. Java gates that diagnostic on
        // `typeInferenceContext.getParent() == null` — it fires
        // only at the outermost processing context. Until our
        // inference module tracks that nesting, replicate Java's
        // silent behaviour at non-outermost calls (the platform PCT
        // corpus depends on this — `<Z|y>` parameters constantly
        // thread through nested `eval` calls).

        return Some(ResolvedType {
            type_expr,
            multiplicity,
        });
    }

    // Built-in operator return types
    infer_builtin_return_type(function_name, arg_types)
}

/// Phase 0 of the function-call processor: `letFunction` is a
/// side-effect form that binds a variable into scope and returns
/// `Nil[0]`. Returns `Some(...)` when this branch handled the call;
/// `None` when the caller should continue with the user-fn dispatch
/// path.
///
/// Bind even when the value's type couldn't be inferred (None) —
/// otherwise subsequent `$name` references downstream would
/// false-positive the undeclared-variable check just because we
/// couldn't infer the let-rhs's type. The placeholder is
/// `TypeExpr::Unresolved` which type-compatibility helpers already
/// treat as "matches anything".
fn process_let_function_call(
    ctx: &mut InferCtx<'_>,
    function_name: &SmolStr,
    let_name: Option<(&SmolStr, &legend_pure_parser_ast::SourceInfo)>,
    arg_types: &[Option<ResolvedType>],
) -> Option<ResolvedType> {
    if function_name != "letFunction" || arg_types.len() != 2 {
        return None;
    }
    let (name, source_info) = let_name?;
    if let Some(scope) = ctx.scopes.last()
        && scope.lookup(name).is_some()
    {
        ctx.errors.push(crate::error::CompilationError {
            message: format!("'{name}' has already been defined!"),
            source_info: (*source_info).clone(),
            kind: crate::error::CompilationErrorKind::DuplicateVariable { name: name.clone() },
        });
    }
    let bound = arg_types[1].clone().unwrap_or(ResolvedType {
        type_expr: TypeExpr::Unresolved,
        multiplicity: Multiplicity::PureOne,
    });
    if let Some(scope) = ctx.scopes.last_mut() {
        scope.bind(name.clone(), bound);
    }
    // `letFunction` itself returns `Nil[0]` (a side-effect statement).
    Some(ResolvedType {
        type_expr: TypeExpr::Named {
            element: bootstrap::NIL_ID,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
        },
        multiplicity: Multiplicity::Range {
            lower: 0,
            upper: Some(0),
        },
    })
}

/// Phase 2 of the function-call processor: per-argument type +
/// multiplicity check. The dispatcher's `narrow_candidates_by_type`
/// short-circuits when there's only one candidate by name + arity, so
/// a sole-overload function would otherwise silently accept mismatched
/// argument types (e.g. `range(0.0, 5)` against
/// `range(Integer[1], Integer[1])`). Run the same `is_type_compatible`
/// check the dispatcher uses, but emit a diagnostic on failure rather
/// than just filtering.
///
/// Substitutes the param using *only* authoritative bindings
/// (`ty_auth`). A `T` whose value was set by a structural
/// `FunctionType` slot (Pure-invariant) survives as its concrete
/// value; a `T` set only by a top-level Generic-typed arg (LUB-able
/// under Java semantics) survives as `Generic("T")`, which
/// `is_type_compatible`'s wildcard arm accepts. Result:
/// - `eval(intFunc, 'wrong')` — T_auth=Integer (from arg 0's
///   `Function<{T→V}>` slot); arg 1 (String) checked against
///   Integer → catch.
/// - `compare(1, 'a')` — T_auth empty; arg 1 ('a') checked
///   against Generic("T") → wildcard pass.
/// - `compare(1, 2.2)` — same; T_auth empty → wildcard pass.
/// - `takesInt(2.2)` — no Generic; param is concrete Integer;
///   `is_type_compatible(Float, Integer)` → false → catch.
///
/// This is a deliberate divergence over Java semantics: Java itself
/// silently widens via `findBestCommonGenericType` covariant LUB,
/// turning mismatched-type args into `Any`. We surface the catch
/// surface listed above. See
/// `inference::context::GenericBindings::ty_auth` for the source-side
/// of the auth-vs-constraint distinction.
#[allow(clippy::too_many_arguments)]
fn validate_call_arguments(
    ctx: &mut InferCtx<'_>,
    function: Option<crate::ids::ElementId>,
    function_name: &SmolStr,
    parameters: &[Parameter],
    arg_types: &[Option<ResolvedType>],
    arg_source_infos: &[legend_pure_parser_ast::SourceInfo],
    bindings: &crate::inference::GenericBindings,
) {
    for (param, (arg_ty, arg_idx)) in parameters.iter().zip(arg_types.iter().zip(0usize..)) {
        let Some(arg_ty) = arg_ty else { continue };
        let arg_eid = match &arg_ty.type_expr {
            TypeExpr::Named { element, .. } => Some(*element),
            _ => None,
        };
        let param_te = bindings.make_concrete_type_strict(&param.type_expr);
        // Multiplicity bindings don't suffer the same LUB-widening
        // issue (multiplicity LUB stays in the range lattice), so
        // re-use the full bindings for the multiplicity side.
        let param_mult = bindings.make_concrete_mult(&param.multiplicity);
        // Suppress when an alternative overload at this package
        // would accept the actual arg type AND multiplicity —
        // the dispatcher had a real choice; second-guessing its
        // ranking is out of scope for this layer.
        if has_compatible_sibling_overload(
            ctx.model,
            function,
            function_name,
            arg_idx,
            arg_eid,
            Some(&arg_ty.multiplicity),
        ) {
            continue;
        }
        // Imprecise-inference detector: when arg and param share
        // the same element id but the param carries type-arguments
        // (generic specialisation) and the arg doesn't, the arg's
        // generic bindings — including its multiplicity — were
        // never narrowed. Multiplicity inference for chained
        // `cast<T|m>` and similar `m`-generic returns currently
        // loses the binding; reporting an error here would just
        // surface that upstream gap as a noisy false positive.
        // Skip until generic-multiplicity binding propagates
        // through the chain.
        if let (
            TypeExpr::Named {
                element: a_eid,
                type_arguments: a_args,
                ..
            },
            TypeExpr::Named {
                element: p_eid,
                type_arguments: p_args,
                ..
            },
        ) = (&arg_ty.type_expr, &param_te)
            && a_eid == p_eid
            && a_args.is_empty()
            && !p_args.is_empty()
        {
            continue;
        }
        if !crate::resolve::is_multiplicity_compatible(Some(&arg_ty.multiplicity), &param_mult) {
            let arg_si = arg_source_infos.get(arg_idx).cloned().unwrap_or_else(|| {
                arg_source_infos.first().cloned().unwrap_or_else(|| {
                    legend_pure_parser_ast::SourceInfo::new("<unknown>", 0, 0, 0, 0)
                })
            });
            ctx.errors.push(crate::error::CompilationError {
                message: format!(
                    "Argument {} of '{}': expected multiplicity {}, got {}",
                    arg_idx + 1,
                    function_name,
                    render_multiplicity(&param_mult),
                    render_multiplicity(&arg_ty.multiplicity),
                ),
                source_info: arg_si,
                kind: crate::error::CompilationErrorKind::UnresolvedElement {
                    path: SmolStr::from(format!(
                        "argument-multiplicity-mismatch:{function_name}:{arg_idx}"
                    )),
                },
            });
        }
        // Three cases this nominal check can't model — they
        // need structural / lattice-aware matching that lives
        // upstream and isn't this layer's concern. Skipping them
        // is *not* the kind of "trustworthy" gate the user
        // pushed back on (those suppressed real bugs); these are
        // category mismatches the check fundamentally doesn't
        // handle:
        //
        // 1. **Function references**: `myFn` as a value has
        // M3 metatype `ConcreteFunctionDefinition` /
        // `NativeFunctionDefinition`, both subtypes of the
        // `Function` metaclass. `is_subtype` walks Class
        // `super_types`, but the M3 metamodel hierarchy
        // isn't always loaded as Class supertypes, so a
        // function-arg vs `Function<{…}>` param falsely
        // fails. The proper fix is to teach `is_subtype`
        // about the metamodel hierarchy; until then the
        // structural `FunctionType` type-arg on the param
        // is enough to identify the callee shape.
        //
        // 2. **Structural `FunctionType` params**: the param's
        // type-expr is `TypeExpr::FunctionType { … }`
        // (lambda arrow type), not `Named { … }`. Nominal
        // element comparison is meaningless.
        //
        // 3. **`Nil` arg**: the empty-collection literal `[]`
        // lowers to `Nil[0..0]`. `Nil` is the bottom of
        // Pure's subtyping lattice, compatible with every
        // type. The platform passes it freely into Function
        // and other typed params.
        if let Some(eid) = arg_eid
            && matches!(ctx.model.try_get_element(eid), Some(Element::Function(_)))
        {
            continue;
        }
        // Package values (`meta::pure::functions::meta` passed
        // as a `PackageableElement` arg) — same metaclass-
        // hierarchy issue as function references. `is_subtype`
        // walks `Class.super_types` but doesn't navigate the
        // metaclass hierarchy of value-element-IDs, so the
        // nominal check spuriously rejects every Package arg.
        // Skip until `is_subtype` learns the metaclass story.
        if matches!(arg_eid, Some(crate::ids::ElementId::Package(_))) {
            continue;
        }
        if arg_eid == Some(bootstrap::NIL_ID) {
            continue;
        }
        // Structural compat (type-arguments-aware). Catches mismatches
        // that live inside parametric wrappers — `Function<{Function<{
        // ->String}>->…}>` vs `Function<{Function<{->Integer}>->…}>`
        // at nested depth. The previous nominal-only check
        // (`is_type_compatible`) considered these compatible because
        // the outer element id matches.
        if !crate::resolve::is_type_compatible_structural(&arg_ty.type_expr, &param_te, ctx.model) {
            // Render both sides through the same helper so FunctionType
            // / Generic / nested-Named arguments are surfaced cleanly
            // (the previous element-name-only path produced `<unknown>`
            // for any non-Named arg type, e.g. lambda literals).
            let arg_rendered = render_type(ctx.model, &arg_ty.type_expr, &arg_ty.multiplicity);
            let param_rendered = render_type(ctx.model, &param_te, &param_mult);
            let arg_si = arg_source_infos.get(arg_idx).cloned().unwrap_or_else(|| {
                arg_source_infos.first().cloned().unwrap_or_else(|| {
                    legend_pure_parser_ast::SourceInfo::new("<unknown>", 0, 0, 0, 0)
                })
            });
            ctx.errors.push(crate::error::CompilationError {
                message: format!(
                    "Argument {} of '{}': expected {}, got {}",
                    arg_idx + 1,
                    function_name,
                    param_rendered,
                    arg_rendered,
                ),
                source_info: arg_si,
                kind: crate::error::CompilationErrorKind::UnresolvedElement {
                    path: SmolStr::from(format!(
                        "argument-type-mismatch:{function_name}:{arg_idx}"
                    )),
                },
            });
        }
    }
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

        // `copy`'s type comes from `lower_copy`'s pre-set `type_info`
        // (it captures the source variable's declared type at lower
        // time — see `lower/copy_slice.rs`). The `set_and_return`
        // honour-pre-set rule routes that through inference without
        // needing a string-match arm here.
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
    /// Return type with receiver type-arg + mult-arg bindings already
    /// substituted.
    return_type: ResolvedType,
    /// QP parameter list (cloned from the resolved QP).
    parameters: Vec<Parameter>,
    /// Bindings to substitute when checking `parameters[i].type_expr`
    /// and `parameters[i].multiplicity`.
    bindings: crate::resolve::GenericBindings,
    /// Receiver class name (for diagnostics).
    receiver_type_name: SmolStr,
}

/// Inference helper for simple property access (`$x.name`). Shared by
/// the legacy `ExprKind::PropertyAccess` arm and the new
/// `ExprKind::PropertyCall(FunctionCallData {.. })` arm.
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
///    or `infer_qualified_property` (handles `UnknownProperty` errors,
///    QP arity + arg-type validation, etc.).
/// 4. If the receiver multiplicity is non-strictly-toOne, rewrite
///    `expr.kind` to a `map(receiver, λ{v_automap | property(v_automap,...)})`
///    call and return the rewritten map's resolved type.
/// 5. Otherwise restore the original `PropertyCall` /
///    `QualifiedPropertyCall` variant and return the property's type
///    directly.
fn infer_property_or_qp_call(ctx: &mut InferCtx<'_>, expr: &mut ValueSpec) -> Option<ResolvedType> {
    // Step 1: take the data out so we can later mutate `expr.kind`.
    let is_qualified = matches!(&*expr.kind, ExprKind::QualifiedPropertyCall(_));
    let placeholder = ExprKind::IntegerLiteral(0);
    let kind = std::mem::replace(&mut *expr.kind, placeholder);
    let (ExprKind::PropertyCall(mut data) | ExprKind::QualifiedPropertyCall(mut data)) = kind
    else {
        unreachable!("matched on PropertyCall/QualifiedPropertyCall above")
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
    matches!(
        m,
        Multiplicity::PureOne
            | Multiplicity::Range {
                lower: 1,
                upper: Some(1),
            }
    )
}

/// Multiplies two multiplicities to produce the result of `map(coll: T[m], λ: T[1] → U[n]): U[m*n]`.
fn multiply_multiplicities(a: &Multiplicity, b: &Multiplicity) -> Multiplicity {
    let bounds = |m: &Multiplicity| -> (u32, u32) {
        match m {
            Multiplicity::PureOne => (1, 1),
            Multiplicity::ZeroOrOne => (0, 1),
            Multiplicity::ZeroOrMany | Multiplicity::Variable(_) => (0, u32::MAX),
            Multiplicity::OneOrMany => (1, u32::MAX),
            Multiplicity::Range { lower, upper } => (*lower, upper.unwrap_or(u32::MAX)),
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

/// Builds the `map(receiver, λ{v_automap | property_call(v_automap,...)})`
/// expression that replaces a property/QP call on a non-toOne receiver.
/// Mirrors Java's `FunctionExpressionProcessor.buildLambdaForMapWithProperty`.
#[allow(clippy::needless_pass_by_value)] // SourceInfo is small but not Copy; consumed below
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
/// `ExprKind::QualifiedPropertyCall(FunctionCallData {.. })` arm.
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
    // Bare `FunctionType` receivers — produced by `infer_expr`'s Lambda
    // branch for inline lambdas like `{|1}` — are bridged to
    // `Named<LambdaFunction>{[FunctionType]}` for property lookup so
    // metamodel-reflection chains like
    // `{|1}->evaluateAndDeactivate().expressionSequence->toOne()` can
    // navigate through `LambdaFunction → FunctionDefinition` to find
    // `expressionSequence`. Without this bridge, the receiver fell off
    // the cliff at `.expressionSequence` and downstream `toOne` had no
    // T to bind. Mirrors the lower-time wrapping in
    // `infer_let_type`'s Lambda branch — the two paths produce the
    // same shape for property access purposes.
    //
    // `Generic(name)` receivers — a let-bound or substitution result
    // that landed on a still-unbound type-parameter (commonly the
    // enclosing function's outer Generic, e.g.
    // `testFn<Z|y>(...) { let z = $f->eval(...); $z.genericType... }`
    // where Z survives in $z's stored type) — are treated as `Any`
    // for property lookup. Java's runtime defers reflection on
    // Generic-typed receivers; our compiler does the same so the
    // chain doesn't fall off and downstream calls (`->toOne()` etc.)
    // can still bind T from the *property's* declared return type.
    // Without this, `$z.genericType.rawType->toOne()` left T
    // unresolved at the return-check
    // (match.pure:185).
    let (receiver_id, receiver_type_args, receiver_mult_args) = match &target.type_expr {
        TypeExpr::Named {
            element,
            type_arguments,
            multiplicity_arguments,
            ..
        } => (
            *element,
            type_arguments.clone(),
            multiplicity_arguments.clone(),
        ),
        TypeExpr::FunctionType { .. } => {
            let lambda_metaclass = ctx.model.resolve_by_path(&[
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("function"),
                SmolStr::new("LambdaFunction"),
            ]);
            match lambda_metaclass {
                Some(eid) => (eid, vec![target.type_expr.clone()], Vec::new()),
                None => return PropertyLookup::UnknownTarget,
            }
        }
        // Generic receiver: defer to runtime-flexible Any-result.
        // Property access on a still-unbound type-parameter (the
        // enclosing function's outer Generic, e.g. `$z` typed
        // `Generic("Z")` from `let z = $f->eval(…)`) can't compile-
        // time-resolve a property. Java treats this permissively:
        // the chain compiles, the result is Any, and downstream
        // generic calls can still bind T from the property's
        // declared return type. Return a synthetic
        // `FoundProperty(Any[*])` so the chain continues; the
        // check then fires on whatever the next call
        // does with this Any.
        TypeExpr::Generic(_) => {
            return PropertyLookup::FoundProperty(ResolvedType {
                type_expr: TypeExpr::Named {
                    element: bootstrap::ANY_ID,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                    source_info: None,
                },
                multiplicity: Multiplicity::ZeroOrMany,
            });
        }
        _ => return PropertyLookup::UnknownTarget,
    };
    let Some(receiver_elem) = ctx.model.try_get_element(receiver_id) else {
        return PropertyLookup::UnknownTarget;
    };
    if !matches!(receiver_elem, Element::Class(_)) {
        return PropertyLookup::UnknownTarget;
    }
    let receiver_type_name = ctx.model.get_node(receiver_id).name.clone();

    // Build type-argument and multiplicity-argument bindings for the
    // immediate receiver class. `Pair<Integer,String>` (class
    // `Pair<U,V>`) → { U → Integer, V → String }. `Holder<String|*>`
    // (class `Holder<T|m>`) → { T → String } and { m → ZeroOrMany }.
    let receiver_bindings = compute_type_arg_bindings(
        ctx.model,
        receiver_id,
        &receiver_type_args,
        &receiver_mult_args,
    );

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

    // Enum-value access shortcut. `MyEnum.RED` lowers to a
    // PropertyCall whose receiver type is
    // `Named<Enumeration>{[Named<MyEnum>{}]}` (per the bare-element
    // ref's metatype lift). The property name `RED` isn't a real
    // property of Enumeration — it's a value of MyEnum. Look for it
    // there so downstream `enum.RED->class()` etc. flows the
    // enum's element type rather than falling off as
    // UnknownTarget. Java's
    // `meta::pure::functions::meta::class<T>(any:T[*]):Class<T>[1]`
    // depends on this (`class.pure:49 enum_value->class()`).
    let enumeration_eid = ctx.model.resolve_by_path(&[
        SmolStr::new("meta"),
        SmolStr::new("pure"),
        SmolStr::new("metamodel"),
        SmolStr::new("type"),
        SmolStr::new("Enumeration"),
    ]);
    if Some(receiver_id) == enumeration_eid
        && let [
            TypeExpr::Named {
                element: enum_eid, ..
            },
        ] = receiver_type_args.as_slice()
        && let Some(Element::Enumeration(enum_def)) = ctx.model.try_get_element(*enum_eid)
        && enum_def
            .values
            .iter()
            .any(|v| v.name.as_str() == property_name)
    {
        return PropertyLookup::FoundProperty(ResolvedType {
            type_expr: TypeExpr::Named {
                element: *enum_eid,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            },
            multiplicity: Multiplicity::PureOne,
        });
    }

    // Defer to runtime reflection ONLY when the receiver is a
    // parametric metatype carrier (Class<X>, Enumeration<X>,
    // Function<…>) or `Any`. For these:
    // - Class<Person>.allInstances, MyEnum.RED, lambda-param-infers-Any
    // all bring in members from the inner element X (or are simply
    // unknown), and the compile-time lookup can't resolve them
    // without a metatype-aware inner-element walk + improved
    // lambda-param inference.
    //
    // For `Any` specifically, Java allows arbitrary property access
    // and yields `Any[*]` at runtime — used by reflective chains
    // like `$x.genericType.rawType->toOne()` where the receiver
    // chain transits through Any-typed intermediate values. Return
    // `FoundProperty(Any[*])` so the chain compiles; downstream
    // generic calls then see a Any-typed arg and can bind T=Any.
    //
    // Non-parametric chunk-0 classes (`GenericType`, `Property`,
    // `FunctionType`, `MultiplicityValue`, `SimpleFunctionExpression`,
    // …) are real M3 classes with declared properties — validate
    // them normally so typos like `pair(1,2)->genericType().rawTyp`
    // produce a compile error instead of silently returning Unit.
    if receiver_id == bootstrap::ANY_ID {
        return PropertyLookup::FoundProperty(ResolvedType {
            type_expr: TypeExpr::Named {
                element: bootstrap::ANY_ID,
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
                source_info: None,
            },
            multiplicity: Multiplicity::ZeroOrMany,
        });
    }
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

/// Computes type-parameter and multiplicity-parameter bindings for a
/// class from a use-site's `<TypeArgs|MultArgs>`.
///
/// Empty maps when the class has no parameters or the receiver carried
/// no arguments.
fn compute_type_arg_bindings(
    model: &PureModel,
    class_id: ElementId,
    type_arguments: &[TypeExpr],
    multiplicity_arguments: &[Multiplicity],
) -> crate::resolve::GenericBindings {
    let mut out = crate::resolve::GenericBindings::default();
    if let Some(Element::Class(class)) = model.try_get_element(class_id) {
        for (param, arg) in class.type_parameters.iter().zip(type_arguments.iter()) {
            out.ty.insert(param.name.clone(), arg.clone());
        }
        for (param_name, arg) in class
            .multiplicity_parameters
            .iter()
            .zip(multiplicity_arguments.iter())
        {
            out.mult.insert(param_name.clone(), arg.clone());
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
    bindings: &crate::resolve::GenericBindings,
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
            type_expr: bindings.make_concrete_type(&prop.type_expr),
            multiplicity: bindings.make_concrete_mult(&prop.multiplicity),
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
                    type_expr: bindings.make_concrete_type(&qp.return_type),
                    multiplicity: bindings.make_concrete_mult(&qp.return_multiplicity),
                },
                parameters: qp.parameters.to_vec(),
                bindings: bindings.clone(),
                receiver_type_name: receiver_type_name.clone(),
            })
            .collect();
        return Some(PropertyLookup::FoundQualifiedProperties(candidates));
    }

    // 3. Association-injected properties. The derived index registers
    // each association property on its OWN target class; the property
    // visible from this class for navigation is the OTHER end (index
    // `1 - prop_idx_pointing_to_self`). Same convention as
    // `runtime/native/lang.rs:832`.
    for (assoc_id, prop_idx_pointing_to_self) in model.association_properties(class_id) {
        if let Some(Element::Association(assoc)) = model.try_get_element(*assoc_id)
            && assoc.properties.len() == 2
        {
            let injected = &assoc.properties[1 - *prop_idx_pointing_to_self];
            if injected.name == property_name {
                let resolved = ResolvedType {
                    type_expr: bindings.make_concrete_type(&injected.type_expr),
                    multiplicity: bindings.make_concrete_mult(&injected.multiplicity),
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
            multiplicity_arguments: super_mult_args,
            ..
        } = st
        {
            // Substitute current bindings into the supertype's type and
            // mult args so generics carry through (`Foo<T|m> extends
            // Bar<List<T>|m>` looks up properties on Bar with X →
            // List<T_resolved> and m → m_resolved).
            let substituted_args: Vec<TypeExpr> = super_args
                .iter()
                .map(|a| bindings.make_concrete_type(a))
                .collect();
            let substituted_mult_args: Vec<Multiplicity> = super_mult_args
                .iter()
                .map(|m| bindings.make_concrete_mult(m))
                .collect();
            let super_bindings = compute_type_arg_bindings(
                model,
                *super_id,
                &substituted_args,
                &substituted_mult_args,
            );
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
        let expected_type = chosen.bindings.make_concrete_type(&param.type_expr);
        let expected_mult = chosen.bindings.make_concrete_mult(&param.multiplicity);
        let arg_eid = arg_ty.as_ref().and_then(|rt| match &rt.type_expr {
            TypeExpr::Named { element, .. } => Some(*element),
            _ => None,
        });
        let arg_mult = arg_ty.as_ref().map(|rt| &rt.multiplicity);

        let type_ok = crate::resolve::is_type_compatible(arg_eid, &expected_type, ctx.model);
        let mult_ok = crate::resolve::is_multiplicity_compatible(arg_mult, &expected_mult);

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
pub(crate) fn render_type(
    model: &PureModel,
    type_expr: &TypeExpr,
    multiplicity: &Multiplicity,
) -> SmolStr {
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
        TypeExpr::FunctionType {
            parameters,
            return_type,
            return_multiplicity,
        } => {
            out.push_str("Function<{");
            for (i, (te, mult)) in parameters.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                render_type_expr(model, te, out);
                out.push('[');
                out.push_str(&render_multiplicity(mult));
                out.push(']');
            }
            out.push_str("->");
            render_type_expr(model, return_type, out);
            out.push('[');
            out.push_str(&render_multiplicity(return_multiplicity));
            out.push(']');
            out.push_str("}>");
        }
        TypeExpr::AlgebraUnion(a, b) => {
            render_type_expr(model, a, out);
            out.push_str(" | ");
            render_type_expr(model, b, out);
        }
        TypeExpr::Relation(_) => out.push_str("<Relation>"),
        TypeExpr::Unresolved => out.push_str("<unresolved>"),
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
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
        },
        multiplicity: Multiplicity::PureOne,
    }
}

/// True when at least one sibling overload (same package + simple
/// name, same parameter count) has a parameter at `arg_idx` whose
/// element id is *different* from `current_fn`'s param at the same
/// index. Used by the arg-vs-param type check to suppress
/// false-positive errors when the dispatcher was ranking among
/// multiple compatible overloads — only single-overload functions
/// (where dispatch has no choice) emit type-mismatch errors.
fn has_compatible_sibling_overload(
    model: &PureModel,
    current_fn: Option<crate::ids::ElementId>,
    function_name: &SmolStr,
    arg_idx: usize,
    arg_eid: Option<crate::ids::ElementId>,
    arg_mult: Option<&Multiplicity>,
) -> bool {
    let Some(current_id) = current_fn else {
        return false;
    };
    let Some(Element::Function(current)) = model.try_get_element(current_id) else {
        return false;
    };
    if matches!(current_id, crate::ids::ElementId::Package(_)) {
        return false;
    }
    let pkg = model.get_node(current_id).parent_package;
    let candidates = model.resolve_functions_by_name_in_package(pkg, function_name);
    for candidate_id in candidates {
        if candidate_id == current_id {
            continue;
        }
        let Some(Element::Function(candidate)) = model.try_get_element(candidate_id) else {
            continue;
        };
        if candidate.parameters.len() != current.parameters.len() {
            continue;
        }
        let Some(candidate_param) = candidate.parameters.get(arg_idx) else {
            continue;
        };
        // If this candidate would accept the actual arg type AND
        // multiplicity, the dispatcher had a real alternative —
        // don't second-guess.
        if crate::resolve::is_type_compatible(arg_eid, &candidate_param.type_expr, model)
            && crate::resolve::is_multiplicity_compatible(arg_mult, &candidate_param.multiplicity)
        {
            return true;
        }
    }
    false
}

/// Verifies that a function body's last expression satisfies the
/// declared return signature.
///
/// Same gating principles as the call-site arg-vs-param check:
///
/// - Both sides must be **leaf primitives** (Integer / Float /
///   Decimal / String / Boolean / three Date kinds). Generic and
///   class returns involve subtyping nuances that this layer
///   doesn't second-guess.
/// - Multiplicity is checked only for **trustworthy expression
///   shapes** (literals, `Variable`, `Collection` literal). Other
///   shapes — chained `FunctionCall`s, `PropertyCall`s, lambdas —
///   can have inferred multiplicity that's wrong upstream (e.g.
///   `expr->toOne()` not narrowing `[*]` to `[1]`); reporting on
///   those would emit noise on real platform code.
///
/// Errors get the function's source span, since "the body returns
/// the wrong thing" is a property of the function as a whole and
/// the IDE squiggle should land on the declaration.
pub fn check_body_return_signature(
    model: &PureModel,
    function_name: &SmolStr,
    function_si: &legend_pure_parser_ast::SourceInfo,
    body: &[ValueSpec],
    expected_type: &TypeExpr,
    expected_mult: &Multiplicity,
    errors: &mut Vec<crate::error::CompilationError>,
) {
    let Some(last) = body.last() else { return };
    let Some(rt) = last.type_info.as_ref() else {
        return;
    };

    // Pure semantics: a `let x = expr;` as the body's last
    // statement is treated as if the body returned `expr` itself —
    // the let's `Nil[0]` return type is a syntactic artifact.
    // (Java Pure's `letAsLastStatement` test names the rule.) Skip
    // the body-return check when we'd otherwise flag every
    // platform-side function whose tail is a `let`.
    if let ExprKind::FunctionCall(crate::types::FunctionCallData { function_name, .. }) =
        &*last.kind
        && function_name == "letFunction"
    {
        return;
    }

    let actual_eid = match &rt.type_expr {
        TypeExpr::Named { element, .. } => Some(*element),
        _ => None,
    };
    let expected_eid = match expected_type {
        TypeExpr::Named { element, .. } => Some(*element),
        _ => None,
    };

    // Previously this site short-circuited on `actual_eid == Any` or
    // `actual_eid == Nil` — protection against imprecise inference
    // that left the body's tail typed as `Any` (top, "couldn't
    // narrow") or `Nil` (bottom, "generic binding picked bottom
    // because there was nothing to anchor against, like
    // `fold(λ, [])`'s accumulator").
    //
    // No longer needed:
    // - `Nil` is now a subtype of every type per
    // `resolve::is_subtype`'s explicit Nil-as-bottom rule. So
    // `is_type_compatible(Nil, X) = true` always — the check below
    // passes for free.
    // - `Any`-typed bodies against more-specific declared returns are
    // real precision losses. Locked to zero on the embedded
    // platform by `inference_precision_sweep`'s
    // `PRECISION_CEILING = 0`. Letting this check fire turns any
    // future regression into a per-function diagnostic instead of a
    // silent miss.
    if !crate::resolve::is_type_compatible(actual_eid, expected_type, model) {
        let actual = actual_eid.map_or_else(
            || "<unknown>".to_string(),
            |e| model.element_name(e).to_string(),
        );
        let expected = expected_eid.map_or_else(
            || "<unknown>".to_string(),
            |e| model.element_name(e).to_string(),
        );
        errors.push(crate::error::CompilationError {
            message: format!(
                "Function '{function_name}' declares return type {expected} but body returns {actual}"
            ),
            source_info: function_si.clone(),
            kind: crate::error::CompilationErrorKind::UnresolvedElement {
                path: SmolStr::from(format!("return-type-mismatch:{function_name}")),
            },
        });
    }

    if !crate::resolve::is_multiplicity_compatible(Some(&rt.multiplicity), expected_mult) {
        errors.push(crate::error::CompilationError {
            message: format!(
                "Function '{function_name}' declares return multiplicity {} but body returns {}",
                render_multiplicity(expected_mult),
                render_multiplicity(&rt.multiplicity),
            ),
            source_info: function_si.clone(),
            kind: crate::error::CompilationErrorKind::UnresolvedElement {
                path: SmolStr::from(format!("return-multiplicity-mismatch:{function_name}")),
            },
        });
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
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
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

        // Sanity: known variable should not emit any error.
        assert!(
            errors.is_empty(),
            "known variable should not error, got: {errors:?}"
        );
        let ti = body[0].type_info.as_ref().unwrap();
        assert_eq!(ti.type_expr, named_type(bootstrap::STRING_ID));
        assert_eq!(ti.multiplicity, Multiplicity::PureOne);
    }

    /// Calling a known-signature function with a literal whose type
    /// doesn't match the corresponding parameter type must surface
    /// a compile-time error. Observed in the IDE as "I changed
    /// `range(0, $stop)` to `range(0.0, $stop)` and got no error,
    /// even though `range`'s first parameter is `Integer[1]`."
    ///
    /// Today `infer_function_call` skips arg-vs-param type checks
    /// once a function has been resolved by name+arity (the
    /// dispatcher's `narrow_candidates_by_type` is short-circuited
    /// at `candidates.len() <= 1`). So a sole-overload function
    /// accepting `Integer[1]` happily takes a `Float[1]` argument.
    #[test]
    fn infer_function_call_with_wrong_arg_type_emits_error() {
        use crate::model::{Element as ModelElement, ElementNode, ModelChunk};
        use crate::nodes::function::Function;

        // Bootstrap + a chunk containing a synthetic
        // `meta::test::takesInt(Integer[1]): Integer[1]` function.
        let mut model = model_with_bootstrap();
        let pkg = model.get_or_create_package(&[SmolStr::new("meta"), SmolStr::new("test")]);
        let chunk_id: u16 = 1;
        let mut chunk = ModelChunk::new(chunk_id);
        let func_si = SourceInfo::new("synth.pure", 1, 1, 3, 1);
        let local_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("takesInt_Integer_1__Integer_1_"),
                source_info: func_si.clone(),
                name_source_info: func_si.clone(),
                parent_package: pkg,
            },
            ModelElement::Function(Function {
                function_name: SmolStr::new("takesInt"),
                is_native: false,
                parameters: std::sync::Arc::from(vec![Parameter {
                    name: SmolStr::new("n"),
                    type_expr: named_type(bootstrap::INTEGER_ID),
                    multiplicity: Multiplicity::PureOne,
                    source_info: func_si.clone(),
                }]),
                return_type: named_type(bootstrap::INTEGER_ID),
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let func_id = crate::ids::ElementId::InstanceId {
            chunk_id,
            local_idx,
        };
        model.chunks.push(chunk);
        model.register_element(pkg, func_id);

        // Body: `takesInt(0.0)` — wrong type, must error.
        let mut body = vec![untyped(
            ExprKind::FunctionCall(crate::types::FunctionCallData {
                function: Some(func_id),
                function_name: SmolStr::new("takesInt"),
                arguments: vec![untyped(
                    ExprKind::FloatLiteral(0.0),
                    SourceInfo::new("call.pure", 5, 10, 5, 13),
                )],
            }),
            SourceInfo::new("call.pure", 5, 1, 5, 14),
        )];
        let mut errors = Vec::new();
        infer_function_body(&model, &[], &mut body, &mut errors);

        assert!(
            !errors.is_empty(),
            "expected a type-mismatch error for takesInt(Float), got none"
        );
        let mismatch = errors.iter().find(|e| {
            let m = &e.message;
            (m.contains("Integer") && m.contains("Float")) || m.to_lowercase().contains("type")
        });
        assert!(
            mismatch.is_some(),
            "expected an error mentioning the type mismatch (Integer vs Float), got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Helper: builds a model with a synthetic
    /// `meta::test::takesInt(n: Integer[1]): Integer[1]` and returns
    /// `(model, takesInt_id)`. Reused by the multiplicity tests.
    fn model_with_takes_int() -> (PureModel, crate::ids::ElementId) {
        use crate::model::{Element as ModelElement, ElementNode, ModelChunk};
        use crate::nodes::function::Function;

        let mut model = model_with_bootstrap();
        let pkg = model.get_or_create_package(&[SmolStr::new("meta"), SmolStr::new("test")]);
        let chunk_id: u16 = 1;
        let mut chunk = ModelChunk::new(chunk_id);
        let func_si = SourceInfo::new("synth.pure", 1, 1, 3, 1);
        let local_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("takesInt_Integer_1__Integer_1_"),
                source_info: func_si.clone(),
                name_source_info: func_si.clone(),
                parent_package: pkg,
            },
            ModelElement::Function(Function {
                function_name: SmolStr::new("takesInt"),
                is_native: false,
                parameters: std::sync::Arc::from(vec![Parameter {
                    name: SmolStr::new("n"),
                    type_expr: named_type(bootstrap::INTEGER_ID),
                    multiplicity: Multiplicity::PureOne,
                    source_info: func_si.clone(),
                }]),
                return_type: named_type(bootstrap::INTEGER_ID),
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let func_id = crate::ids::ElementId::InstanceId {
            chunk_id,
            local_idx,
        };
        model.chunks.push(chunk);
        model.register_element(pkg, func_id);
        (model, func_id)
    }

    /// Calling a `[1]`-multiplicity parameter with a `[0..1]`
    /// argument (e.g. an outer parameter typed `Integer[0..1]`)
    /// must error. Observed in the IDE as: "If `range`'s start
    /// param is changed to a multiplicity other than [1], calls
    /// expecting [1] still don't error."
    #[test]
    fn infer_function_call_with_wrong_arg_multiplicity_emits_error() {
        let (model, func_id) = model_with_takes_int();

        // Body: `takesInt($x)` with $x: Integer[0..1]. Param expects [1].
        let mut body = vec![untyped(
            ExprKind::FunctionCall(crate::types::FunctionCallData {
                function: Some(func_id),
                function_name: SmolStr::new("takesInt"),
                arguments: vec![untyped(
                    ExprKind::Variable {
                        name: SmolStr::new("x"),
                    },
                    SourceInfo::new("call.pure", 5, 10, 5, 12),
                )],
            }),
            SourceInfo::new("call.pure", 5, 1, 5, 13),
        )];
        let outer_params = vec![Parameter {
            name: SmolStr::new("x"),
            type_expr: named_type(bootstrap::INTEGER_ID),
            multiplicity: Multiplicity::ZeroOrOne,
            source_info: SourceInfo::new("call.pure", 1, 1, 1, 1),
        }];
        let mut errors = Vec::new();
        infer_function_body(&model, &outer_params, &mut body, &mut errors);

        let mismatch = errors.iter().find(|e| {
            let m = e.message.to_lowercase();
            m.contains("multiplicity") || m.contains("[0..1]") || m.contains("[1]")
        });
        assert!(
            mismatch.is_some(),
            "expected a multiplicity-mismatch error, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Builds a synthetic `meta::test::takes(n: Integer[<param_mult>])`
    /// function for parameterised multiplicity tests.
    fn model_with_takes_int_mult(param_mult: Multiplicity) -> (PureModel, crate::ids::ElementId) {
        use crate::model::{Element as ModelElement, ElementNode, ModelChunk};
        use crate::nodes::function::Function;

        let mut model = model_with_bootstrap();
        let pkg = model.get_or_create_package(&[SmolStr::new("meta"), SmolStr::new("test")]);
        let chunk_id: u16 = 1;
        let mut chunk = ModelChunk::new(chunk_id);
        let func_si = SourceInfo::new("synth.pure", 1, 1, 3, 1);
        let local_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("takes_Integer_X__Integer_1_"),
                source_info: func_si.clone(),
                name_source_info: func_si.clone(),
                parent_package: pkg,
            },
            ModelElement::Function(Function {
                function_name: SmolStr::new("takes"),
                is_native: false,
                parameters: std::sync::Arc::from(vec![Parameter {
                    name: SmolStr::new("n"),
                    type_expr: named_type(bootstrap::INTEGER_ID),
                    multiplicity: param_mult,
                    source_info: func_si.clone(),
                }]),
                return_type: named_type(bootstrap::INTEGER_ID),
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let func_id = crate::ids::ElementId::InstanceId {
            chunk_id,
            local_idx,
        };
        model.chunks.push(chunk);
        model.register_element(pkg, func_id);
        (model, func_id)
    }

    /// Run `takes(<arg>)` against a function whose parameter has
    /// `param_mult`. The `arg` is a single Variable `$x` whose
    /// outer-scope binding has `arg_mult`. Asserts whether an error
    /// is emitted matching `expect_error`.
    fn run_mult_check(
        param_mult: Multiplicity,
        arg_mult: Multiplicity,
        expect_error: bool,
        scenario: &str,
    ) {
        let (model, func_id) = model_with_takes_int_mult(param_mult);
        let mut body = vec![untyped(
            ExprKind::FunctionCall(crate::types::FunctionCallData {
                function: Some(func_id),
                function_name: SmolStr::new("takes"),
                arguments: vec![untyped(
                    ExprKind::Variable {
                        name: SmolStr::new("x"),
                    },
                    SourceInfo::new("call.pure", 5, 10, 5, 12),
                )],
            }),
            SourceInfo::new("call.pure", 5, 1, 5, 13),
        )];
        let outer_params = vec![Parameter {
            name: SmolStr::new("x"),
            type_expr: named_type(bootstrap::INTEGER_ID),
            multiplicity: arg_mult,
            source_info: SourceInfo::new("call.pure", 1, 1, 1, 1),
        }];
        let mut errors = Vec::new();
        infer_function_body(&model, &outer_params, &mut body, &mut errors);

        let got_error = errors.iter().any(|e| {
            let m = e.message.to_lowercase();
            m.contains("multiplicity")
        });
        assert_eq!(
            got_error,
            expect_error,
            "{scenario}: errors = {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Compatibility matrix — arg mult `[1]` fits every supertype
    /// range. None should error.
    #[test]
    fn mult_pure_one_arg_is_compatible_with_supertype_params() {
        run_mult_check(
            Multiplicity::PureOne,
            Multiplicity::PureOne,
            false,
            "[1] arg into [1] param",
        );
        run_mult_check(
            Multiplicity::ZeroOrOne,
            Multiplicity::PureOne,
            false,
            "[1] arg into [0..1] param",
        );
        run_mult_check(
            Multiplicity::OneOrMany,
            Multiplicity::PureOne,
            false,
            "[1] arg into [1..*] param",
        );
        run_mult_check(
            Multiplicity::ZeroOrMany,
            Multiplicity::PureOne,
            false,
            "[1] arg into [*] param",
        );
    }

    /// `[0..1]` arg is broader than `[1]` — must error when the
    /// param insists on `[1]`. Compatible with `[0..1]` and `[*]`.
    #[test]
    fn mult_zero_or_one_arg_against_various_params() {
        run_mult_check(
            Multiplicity::PureOne,
            Multiplicity::ZeroOrOne,
            true,
            "[0..1] arg into [1] param — must error",
        );
        run_mult_check(
            Multiplicity::ZeroOrOne,
            Multiplicity::ZeroOrOne,
            false,
            "[0..1] arg into [0..1] param",
        );
        run_mult_check(
            Multiplicity::OneOrMany,
            Multiplicity::ZeroOrOne,
            true,
            "[0..1] arg into [1..*] param — lower-bound mismatch",
        );
        run_mult_check(
            Multiplicity::ZeroOrMany,
            Multiplicity::ZeroOrOne,
            false,
            "[0..1] arg into [*] param",
        );
    }

    /// `[1..*]` arg fits `[1..*]` and `[*]` but not `[1]` or `[0..1]`.
    #[test]
    fn mult_one_or_many_arg_against_various_params() {
        run_mult_check(
            Multiplicity::PureOne,
            Multiplicity::OneOrMany,
            true,
            "[1..*] arg into [1] param — must error",
        );
        run_mult_check(
            Multiplicity::ZeroOrOne,
            Multiplicity::OneOrMany,
            true,
            "[1..*] arg into [0..1] param — must error",
        );
        run_mult_check(
            Multiplicity::OneOrMany,
            Multiplicity::OneOrMany,
            false,
            "[1..*] arg into [1..*] param",
        );
        run_mult_check(
            Multiplicity::ZeroOrMany,
            Multiplicity::OneOrMany,
            false,
            "[1..*] arg into [*] param",
        );
    }

    /// `[*]` (zero-or-many) is the broadest — only fits `[*]`.
    #[test]
    fn mult_zero_or_many_arg_against_various_params() {
        run_mult_check(
            Multiplicity::PureOne,
            Multiplicity::ZeroOrMany,
            true,
            "[*] arg into [1] param — must error",
        );
        run_mult_check(
            Multiplicity::ZeroOrOne,
            Multiplicity::ZeroOrMany,
            true,
            "[*] arg into [0..1] param — must error",
        );
        run_mult_check(
            Multiplicity::OneOrMany,
            Multiplicity::ZeroOrMany,
            true,
            "[*] arg into [1..*] param — must error",
        );
        run_mult_check(
            Multiplicity::ZeroOrMany,
            Multiplicity::ZeroOrMany,
            false,
            "[*] arg into [*] param",
        );
    }

    /// Bounded ranges. `[2..2]` = exactly two; fits `[2..2]`, `[1..*]`,
    /// `[2..3]`, `[*]` but not `[1]` or `[0..1]`.
    #[test]
    fn mult_fixed_range_arg_against_various_params() {
        let two = Multiplicity::Range {
            lower: 2,
            upper: Some(2),
        };
        run_mult_check(
            Multiplicity::PureOne,
            two.clone(),
            true,
            "[2] arg into [1] param — must error",
        );
        run_mult_check(
            Multiplicity::ZeroOrOne,
            two.clone(),
            true,
            "[2] arg into [0..1] param — must error",
        );
        run_mult_check(
            Multiplicity::OneOrMany,
            two.clone(),
            false,
            "[2] arg into [1..*] param",
        );
        run_mult_check(two.clone(), two.clone(), false, "[2] arg into [2] param");
        run_mult_check(
            Multiplicity::Range {
                lower: 2,
                upper: Some(3),
            },
            two.clone(),
            false,
            "[2] arg into [2..3] param",
        );
        run_mult_check(
            Multiplicity::ZeroOrMany,
            two,
            false,
            "[2] arg into [*] param",
        );
    }

    /// Passing a collection literal `[1, 2]` (multiplicity `[2..2]`)
    /// to a parameter expecting a single value `Integer[1]` must
    /// error. Observed in the IDE as: "If a constant `1` I changed
    /// to a collection `[1, 2]`, no error is brought up."
    #[test]
    fn infer_function_call_with_collection_in_scalar_position_emits_error() {
        let (model, func_id) = model_with_takes_int();

        // Body: `takesInt([1, 2])` — multiplicity [2..2] vs param [1].
        let coll = untyped(
            ExprKind::Collection {
                elements: vec![
                    untyped(
                        ExprKind::IntegerLiteral(1),
                        SourceInfo::new("call.pure", 5, 11, 5, 12),
                    ),
                    untyped(
                        ExprKind::IntegerLiteral(2),
                        SourceInfo::new("call.pure", 5, 14, 5, 15),
                    ),
                ],
            },
            SourceInfo::new("call.pure", 5, 10, 5, 16),
        );
        let mut body = vec![untyped(
            ExprKind::FunctionCall(crate::types::FunctionCallData {
                function: Some(func_id),
                function_name: SmolStr::new("takesInt"),
                arguments: vec![coll],
            }),
            SourceInfo::new("call.pure", 5, 1, 5, 17),
        )];
        let mut errors = Vec::new();
        infer_function_body(&model, &[], &mut body, &mut errors);

        let mismatch = errors.iter().find(|e| {
            let m = e.message.to_lowercase();
            m.contains("multiplicity") || m.contains("[2..2]") || m.contains("[1]")
        });
        assert!(
            mismatch.is_some(),
            "expected a multiplicity-mismatch error for takesInt([1,2]), got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Function body's last expression must satisfy the declared
    /// return *type*. If the function says it returns `Integer[1]`
    /// but the body's tail is a `Float`, the compiler must error
    /// (else the user can write nonsense like
    /// `function foo(): Integer[1] { $x * 1.5 }`).
    #[test]
    fn function_body_return_type_mismatch_emits_error() {
        let model = model_with_bootstrap();
        let body = vec![ValueSpec {
            kind: Box::new(ExprKind::FloatLiteral(1.5)),
            source_info: SourceInfo::new("x.pure", 3, 5, 3, 8),
            type_info: Some(Box::new(ResolvedType {
                type_expr: named_type(bootstrap::FLOAT_ID),
                multiplicity: Multiplicity::PureOne,
            })),
        }];
        let expected_return = (named_type(bootstrap::INTEGER_ID), Multiplicity::PureOne);

        let mut errors = Vec::new();
        check_body_return_signature(
            &model,
            &SmolStr::new("test::foo"),
            &SourceInfo::new("x.pure", 1, 1, 4, 1),
            &body,
            &expected_return.0,
            &expected_return.1,
            &mut errors,
        );

        let mismatch = errors.iter().find(|e| {
            let m = e.message.to_lowercase();
            m.contains("return") && (m.contains("integer") || m.contains("float"))
        });
        assert!(
            mismatch.is_some(),
            "expected a return-type-mismatch error, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Function body's last expression must satisfy the declared
    /// return *multiplicity*. `function foo(): Integer[1] { … }`
    /// must error if the body returns `Integer[0..1]` (e.g. via
    /// `head()`).
    #[test]
    fn function_body_return_multiplicity_mismatch_emits_error() {
        let model = model_with_bootstrap();
        // High-confidence shape — `Variable` reference, mult [0..1].
        let body = vec![ValueSpec {
            kind: Box::new(ExprKind::Variable {
                name: SmolStr::new("x"),
            }),
            source_info: SourceInfo::new("x.pure", 3, 5, 3, 7),
            type_info: Some(Box::new(ResolvedType {
                type_expr: named_type(bootstrap::INTEGER_ID),
                multiplicity: Multiplicity::ZeroOrOne,
            })),
        }];
        let expected_return = (named_type(bootstrap::INTEGER_ID), Multiplicity::PureOne);

        let mut errors = Vec::new();
        check_body_return_signature(
            &model,
            &SmolStr::new("test::foo"),
            &SourceInfo::new("x.pure", 1, 1, 4, 1),
            &body,
            &expected_return.0,
            &expected_return.1,
            &mut errors,
        );

        let mismatch = errors.iter().find(|e| {
            let m = e.message.to_lowercase();
            m.contains("return") && (m.contains("multiplicity") || m.contains("[0..1]"))
        });
        assert!(
            mismatch.is_some(),
            "expected a return-multiplicity-mismatch error, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// A `$x` reference where `x` is not in scope (no parameter, no
    /// preceding `let`, not a lambda binding) must surface a
    /// compile-time error. Today the inference layer silently
    /// returns `None` from `lookup_var`, leaving `type_info` empty
    /// and producing no diagnostic — observed in the IDE as "I
    /// renamed `$stop` to `$sto` and got no error."
    #[test]
    fn infer_undeclared_variable_emits_error() {
        let model = model_with_bootstrap();

        let mut body = vec![untyped(
            ExprKind::Variable {
                name: SmolStr::new("undeclared"),
            },
            SourceInfo::new("test.pure", 3, 5, 3, 16),
        )];
        let mut errors = Vec::new();

        // No params, no enclosing scope — `$undeclared` cannot resolve.
        infer_function_body(&model, &[], &mut body, &mut errors);

        assert!(
            !errors.is_empty(),
            "expected at least one error for `$undeclared`, got none"
        );
        let undeclared_err = errors
            .iter()
            .find(|e| e.message.contains("undeclared"))
            .unwrap_or_else(|| {
                panic!(
                    "expected an error mentioning the undeclared variable name, \
                     got messages: {:?}",
                    errors.iter().map(|e| &e.message).collect::<Vec<_>>()
                )
            });
        // Source location must point at the `$undeclared` use, not
        // the (synthetic) function header — otherwise the IDE
        // squiggle lands on the wrong line.
        assert_eq!(undeclared_err.source_info.start_line, 3);
        assert_eq!(undeclared_err.source_info.start_column, 5);
    }

    /// Mixed-numeric collection literals widen to the LUB. `[1, 1.5]`
    /// must infer as `Number`, not `Integer` — otherwise dispatch
    /// picks `times(Integer[*])` for `1 * 1.5` and the chained
    /// `head()` cascade silently propagates the wrong inner type.
    #[test]
    fn infer_collection_lub_widens_integer_and_float() {
        let model = model_with_bootstrap();
        let mut body = vec![untyped(
            ExprKind::Collection {
                elements: vec![
                    untyped(
                        ExprKind::IntegerLiteral(1),
                        SourceInfo::new("c.pure", 1, 2, 1, 3),
                    ),
                    untyped(
                        ExprKind::FloatLiteral(1.5),
                        SourceInfo::new("c.pure", 1, 5, 1, 8),
                    ),
                ],
            },
            SourceInfo::new("c.pure", 1, 1, 1, 9),
        )];
        let mut errors = Vec::new();
        infer_function_body(&model, &[], &mut body, &mut errors);

        let ti = body[0]
            .type_info
            .as_ref()
            .expect("collection must have type_info");
        // LUB(Integer, Float) = Number — that's what the platform
        // arithmetic dispatch needs to see.
        assert_eq!(
            ti.type_expr,
            named_type(bootstrap::NUMBER_ID),
            "expected Number, got {:?}",
            ti.type_expr,
        );
    }

    /// Passing a non-primitive (a class instance) where a primitive
    /// param is declared must error. Observed as "if I try to pass
    /// `pair(1, 1.3)` to `Integer`, I get no error" — the
    /// `both_primitive` gate was suppressing it.
    #[test]
    fn infer_function_call_with_class_arg_emits_type_error() {
        use crate::model::{Element as ModelElement, ElementNode, ModelChunk};
        use crate::nodes::class::Class;
        use crate::nodes::function::Function;

        let mut model = model_with_bootstrap();
        let pkg = model.get_or_create_package(&[SmolStr::new("meta"), SmolStr::new("test")]);
        let chunk_id: u16 = 1;
        let mut chunk = ModelChunk::new(chunk_id);
        let si = SourceInfo::new("synth.pure", 1, 1, 3, 1);

        // Synthetic class `Box`.
        let box_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("Box"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: pkg,
            },
            ModelElement::Class(Class {
                type_parameters: Vec::new(),
                multiplicity_parameters: Vec::new(),
                type_variable_parameters: Vec::new(),
                super_types: Vec::new(),
                properties: Vec::new(),
                qualified_properties: Vec::new(),
                constraints: Vec::new(),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let box_id = crate::ids::ElementId::InstanceId {
            chunk_id,
            local_idx: box_idx,
        };

        // Synthetic `takesInt(n: Integer[1]): Integer[1]`.
        let fn_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("takesInt_Integer_1__Integer_1_"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: pkg,
            },
            ModelElement::Function(Function {
                function_name: SmolStr::new("takesInt"),
                is_native: false,
                parameters: std::sync::Arc::from(vec![Parameter {
                    name: SmolStr::new("n"),
                    type_expr: named_type(bootstrap::INTEGER_ID),
                    multiplicity: Multiplicity::PureOne,
                    source_info: si.clone(),
                }]),
                return_type: named_type(bootstrap::INTEGER_ID),
                return_multiplicity: Multiplicity::PureOne,
                body: std::sync::Arc::from(Vec::new()),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let fn_id = crate::ids::ElementId::InstanceId {
            chunk_id,
            local_idx: fn_idx,
        };
        model.chunks.push(chunk);
        model.register_element(pkg, box_id);
        model.register_element(pkg, fn_id);

        // Body: `takesInt($b)` where $b: Box[1] — class arg into
        // primitive-typed param. Must error.
        let mut body = vec![untyped(
            ExprKind::FunctionCall(crate::types::FunctionCallData {
                function: Some(fn_id),
                function_name: SmolStr::new("takesInt"),
                arguments: vec![untyped(
                    ExprKind::Variable {
                        name: SmolStr::new("b"),
                    },
                    SourceInfo::new("call.pure", 5, 10, 5, 12),
                )],
            }),
            SourceInfo::new("call.pure", 5, 1, 5, 13),
        )];
        let outer_params = vec![Parameter {
            name: SmolStr::new("b"),
            type_expr: named_type(box_id),
            multiplicity: Multiplicity::PureOne,
            source_info: SourceInfo::new("call.pure", 1, 1, 1, 1),
        }];
        let mut errors = Vec::new();
        infer_function_body(&model, &outer_params, &mut body, &mut errors);

        let mismatch = errors.iter().find(|e| {
            let m = &e.message;
            m.contains("Box") && m.contains("Integer")
        });
        assert!(
            mismatch.is_some(),
            "expected type-mismatch error mentioning Box and Integer, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
    }

    /// Function declared `Integer[1]` but body returns a class
    /// instance — the body-return signature check must error.
    /// Symmetric to `infer_function_call_with_class_arg_emits_type_error`
    /// for the return position.
    #[test]
    fn function_body_returning_class_when_integer_declared_emits_error() {
        use crate::model::{Element as ModelElement, ElementNode, ModelChunk};
        use crate::nodes::class::Class;

        let mut model = model_with_bootstrap();
        let pkg = model.get_or_create_package(&[SmolStr::new("meta"), SmolStr::new("test")]);
        let chunk_id: u16 = 1;
        let mut chunk = ModelChunk::new(chunk_id);
        let si = SourceInfo::new("synth.pure", 1, 1, 3, 1);
        let box_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("Box"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: pkg,
            },
            ModelElement::Class(Class {
                type_parameters: Vec::new(),
                multiplicity_parameters: Vec::new(),
                type_variable_parameters: Vec::new(),
                super_types: Vec::new(),
                properties: Vec::new(),
                qualified_properties: Vec::new(),
                constraints: Vec::new(),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        let box_id = crate::ids::ElementId::InstanceId {
            chunk_id,
            local_idx: box_idx,
        };
        model.chunks.push(chunk);
        model.register_element(pkg, box_id);

        // Body's tail expression is a `$b: Box[1]` reference,
        // already type-info'd to Box[1].
        let body = vec![ValueSpec {
            kind: Box::new(ExprKind::Variable {
                name: SmolStr::new("b"),
            }),
            source_info: SourceInfo::new("c.pure", 3, 5, 3, 7),
            type_info: Some(Box::new(ResolvedType {
                type_expr: named_type(box_id),
                multiplicity: Multiplicity::PureOne,
            })),
        }];
        let mut errors = Vec::new();
        check_body_return_signature(
            &model,
            &SmolStr::new("test::f"),
            &SourceInfo::new("c.pure", 1, 1, 4, 1),
            &body,
            &named_type(bootstrap::INTEGER_ID),
            &Multiplicity::PureOne,
            &mut errors,
        );

        let mismatch = errors.iter().find(|e| {
            let m = e.message.to_lowercase();
            m.contains("return") && m.contains("box") && m.contains("integer")
        });
        assert!(
            mismatch.is_some(),
            "expected return-type mismatch mentioning Box and Integer, got: {:?}",
            errors.iter().map(|e| &e.message).collect::<Vec<_>>()
        );
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
