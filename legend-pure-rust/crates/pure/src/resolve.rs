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

//! Type and annotation resolution utilities.
//!
//! Converts AST references (paths, type refs, annotations) into resolved
//! Pure semantic types by looking up declarations and bootstrap elements.

use std::collections::HashMap;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation as ast_ann;
use legend_pure_parser_ast::element::PackageableElement;
use legend_pure_parser_ast::type_ref::{
    self as ast_type, FUNCTION_TYPE_SENTINEL, Package, RELATION_TYPE_SENTINEL,
};
use smol_str::SmolStr;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::types::{ConstValue, FunctionCallData, Multiplicity, TypeExpr};

// ---------------------------------------------------------------------------
// Import Scope — uses the AST Package type directly
// ---------------------------------------------------------------------------

/// An import package scope entry for resolution.
///
/// Wraps the AST `Package` directly so the resolver can walk the already-
/// parsed package tree against the model — zero string splitting or
/// concatenation needed.
#[derive(Debug, Clone)]
pub(crate) struct ImportScope {
    /// The AST `Package` — the parser already built this recursive tree.
    /// Used by `model.resolve_in_package()` for zero-allocation lookups.
    pub package: Package,
}

impl ImportScope {
    /// Creates an `ImportScope` from an AST `Package` (explicit imports).
    pub fn from_package(package: Package) -> Self {
        Self { package }
    }

    /// Creates an `ImportScope` from a path string (for auto-imports).
    ///
    /// This is the only place where a string is parsed into a `Package` —
    /// once at startup.
    pub fn from_path_str(path: &str) -> Self {
        let dummy = SourceInfo::new("<auto-import>", 0, 0, 0, 0);
        let mut pkg: Option<Package> = None;
        for segment in path.split("::") {
            let name = SmolStr::new(segment);
            pkg = Some(match pkg {
                None => Package::root(name, dummy.clone()),
                Some(parent) => parent.child(name, dummy.clone()),
            });
        }
        let Some(package) = pkg else {
            unreachable!("auto-import path must not be empty");
        };
        Self { package }
    }
}

// ---------------------------------------------------------------------------
// Resolve Cache — per-section memoization
// ---------------------------------------------------------------------------

/// Cached result of an unqualified name resolution within a section scope.
#[derive(Debug, Clone)]
pub(crate) enum ResolveResult {
    /// Successfully resolved to a single element.
    Found(ElementId),
    /// Resolution failed (unresolved or ambiguous) — stores original error
    /// so subsequent lookups re-emit the correct error kind.
    Failed(CompilationError),
}

// ---------------------------------------------------------------------------
// Type Resolution Context
// ---------------------------------------------------------------------------

/// Everything needed to resolve an AST type reference to a `TypeExpr`.
///
/// Holds a reference to the compiled model (which contains both bootstrap
/// primitives and user-declared elements in its package tree), plus the
/// import scope for the current section.
pub(crate) struct ResolutionContext<'a> {
    /// The model — its package tree contains all bootstrap and user-declared
    /// elements, so `resolve_in_package()` handles both.
    pub model: &'a PureModel,
    /// Import scopes for the current section (explicit + auto-imports).
    pub import_scopes: &'a [ImportScope],
    /// Per-section cache: unqualified name → resolved result.
    /// Avoids re-scanning imports for the same name within one section.
    pub resolve_cache: &'a mut HashMap<SmolStr, ResolveResult>,
    /// Type parameters in scope for the current element (e.g., `["T", "V"]`
    /// for `Class<T, V>` or function `<T|m>`). Names here resolve to
    /// `TypeExpr::Generic(name)` instead of going through import lookup.
    pub type_parameters: &'a [SmolStr],
    /// Variable types in scope. Maps variable name → (type, multiplicity).
    /// Populated from function parameters, let bindings, and lambda parameters.
    /// Used by dispatch to infer argument types for variable references.
    ///
    /// Lambda parameters whose type the compiler could not infer (no
    /// source annotation, no caller-side expectation) are stored with
    /// `TypeExpr::Unresolved`. `resolve_function_call`'s ambiguity branch
    /// reads this to upgrade "Ambiguous function call" cascades into a
    /// single `CannotInferLambdaParameterTypes` diagnostic.
    pub variable_types: HashMap<SmolStr, (crate::types::TypeExpr, crate::types::Multiplicity)>,
    /// Inline-island lowerers, keyed by `tag()`. Dispatched when
    /// [`crate::lower::lower_expression`] encounters an
    /// `Expression::Island(_)` node. Empty slice = no islands lower
    /// (the historical default — produces the legacy "Island
    /// expression lowering not yet implemented" diagnostic).
    pub island_lowerers: &'a [Box<dyn crate::island_lower::IslandLowerer>],
}

// ---------------------------------------------------------------------------
// Type Reference Resolution
// ---------------------------------------------------------------------------

/// Resolves an AST `TypeReference` to a Pure `TypeExpr`.
///
/// Resolution order (matches Java `ImportStub.resolvePackageableElement`):
/// 1. If qualified (has package): resolve via the AST Package tree directly
/// 2. If unqualified: check memo cache, then bootstrap, then import packages,
///    then root package fallback
///
/// Returns `None` and pushes a `CompilationError` if the type cannot be resolved.
pub(crate) fn resolve_type_ref(
    type_ref: &ast_type::TypeReference,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<TypeExpr> {
    // Handle the {FunctionType} sentinel: the parser encodes function types
    // like `{String[1]->Boolean[1]}` as a TypeReference with this name when
    // they appear inside type argument positions (e.g., `Function<{->Z[y]}>`)
    if type_ref.name == FUNCTION_TYPE_SENTINEL {
        return resolve_function_type_sentinel(type_ref, ctx, errors);
    }

    // Handle the (RelationType) sentinel: the parser encodes structural
    // relation types like `(a:Integer, b:String)` (when they appear in
    // parameter type position via `Parameter::type_ref`) as a synthetic
    // `TypeReference` whose `type_arguments` are per-column pseudo-refs
    // (`name=col_name, type_arguments=[col_type], multiplicity_arguments[0]=mult`).
    // Decode them back into `TypeExpr::Relation(columns)`.
    if type_ref.name == RELATION_TYPE_SENTINEL {
        return resolve_relation_type_sentinel(type_ref, ctx, errors);
    }

    // `?` wildcard type — used inside `SortInfo<(?:?)⊆T>`-style column
    // specs in over.pure's OLAP overloads (and `eval.pure`'s
    // `ColSpec<(?:Z)⊆T>`). Resolve to a generic placeholder; the
    // narrower / type-checker treats `Generic`-typed positions as
    // permissive so the wildcard doesn't restrict overload matching.
    if type_ref.name.as_str() == "?" {
        return Some(TypeExpr::Generic(SmolStr::new("?")));
    }

    let element_id = if let Some(pkg) = &type_ref.package {
        // Qualified — resolve directly via the AST Package tree
        if let Some(id) = ctx.model.resolve_in_package(pkg, &type_ref.name) {
            id
        } else {
            let display = SmolStr::new(type_ref.full_path());
            errors.push(CompilationError {
                message: format!("Cannot resolve element '{display}'"),
                source_info: type_ref.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: display },
            });
            return None;
        }
    } else {
        // Unqualified — first check if it's a type parameter in scope
        if ctx.type_parameters.iter().any(|tp| tp == &type_ref.name) {
            return Some(TypeExpr::Generic(type_ref.name.clone()));
        }
        // Otherwise go through import-aware resolution with memoization
        resolve_unqualified_cached(&type_ref.name, &type_ref.source_info, ctx, errors)?
    };

    // Recursively resolve type arguments
    let type_arguments: Vec<TypeExpr> = type_ref
        .type_arguments
        .iter()
        .filter_map(|arg| resolve_type_ref(arg, ctx, errors))
        .collect();

    // Lower multiplicity arguments — `Holder<String|*>`'s `*` becomes
    // `Multiplicity::ZeroOrMany`; `Holder<String|m>`'s `m` becomes
    // `Multiplicity::Variable("m")`. Position-aligned with the class's
    // `multiplicity_parameters` so `compute_type_arg_bindings` can map
    // declared `m` → use-site value when substituting on property
    // access.
    let multiplicity_arguments: Vec<Multiplicity> = type_ref
        .multiplicity_arguments
        .iter()
        .map(|ma| match ma {
            ast_type::MultiplicityArgument::Identifier(name, _) => {
                Multiplicity::Variable(name.clone())
            }
            ast_type::MultiplicityArgument::Concrete(m, _) => lower_multiplicity(m),
        })
        .collect();

    // Lower value arguments
    let value_arguments: Vec<ConstValue> = type_ref
        .type_variable_values
        .iter()
        .map(lower_const_value)
        .collect();

    // Eager generic-class type-arg completeness check: `Pair[1]`
    // (where `Pair<U, V>` declares two type parameters) and any
    // other generic class referenced without `<…>` is malformed —
    // with no `T` bindings every downstream check (dispatch,
    // body-return, generic substitution) silently degrades. Sound
    // here because `create_shell` (Pass 1) populates
    // `Class.type_parameters` from the AST, so the target's declared
    // arity is visible regardless of topological hydration order.
    // Recursion on `type_arguments` above covers nested refs.
    if let Some(Element::Class(class)) = ctx.model.try_get_element(element_id) {
        let declared = class.type_parameters.len();
        let supplied = type_arguments.len();
        if declared > 0 && supplied != declared {
            let class_fqn = SmolStr::new(
                crate::purem::fqn_path::element_fqn_path(ctx.model, element_id).join("::"),
            );
            errors.push(CompilationError {
                message: format!(
                    "Reference to generic class '{class_fqn}' is missing required \
                     type arguments (expected {declared}, got {supplied})"
                ),
                source_info: type_ref.source_info.clone(),
                kind: CompilationErrorKind::InvalidAnnotation {
                    element_name: class_fqn.clone(),
                    reason: SmolStr::new(format!(
                        "missing type arguments on '{class_fqn}': expected \
                         {declared}, got {supplied}"
                    )),
                },
            });
        }
    }

    Some(TypeExpr::Named {
        element: element_id,
        type_arguments,
        multiplicity_arguments,
        value_arguments,
    })
}

/// Decodes a `{FunctionType}` sentinel `TypeReference` into `TypeExpr::FunctionType`.
///
/// The parser encodes function types `{ParamType[m], ... -> RetType[m]}` as a
/// `TypeReference` with name `{FunctionType}`:
/// - `type_arguments[0..n-1]` — parameter types, each with its multiplicity in
///   `multiplicity_arguments[0]`
/// - `type_arguments[n-1]` — the return type
/// - top-level `multiplicity_arguments[0]` — the return multiplicity
#[allow(clippy::unnecessary_wraps)]
fn resolve_function_type_sentinel(
    type_ref: &ast_type::TypeReference,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<TypeExpr> {
    if type_ref.type_arguments.is_empty() {
        // Malformed sentinel — no type arguments at all
        return Some(TypeExpr::FunctionType {
            parameters: vec![],
            return_type: Box::new(TypeExpr::Generic("Any".into())),
            return_multiplicity: Multiplicity::ZeroOrMany,
        });
    }

    let n = type_ref.type_arguments.len();

    // Parameter types are all entries except the last
    let mut parameters = Vec::with_capacity(n.saturating_sub(1));
    for param_ref in &type_ref.type_arguments[..n - 1] {
        let param_type =
            resolve_type_ref(param_ref, ctx, errors).unwrap_or(TypeExpr::Generic("Any".into()));

        // Each parameter's multiplicity is stored in its multiplicity_arguments[0]
        let param_mult =
            param_ref
                .multiplicity_arguments
                .first()
                .map_or(Multiplicity::PureOne, |ma| match ma {
                    ast_type::MultiplicityArgument::Concrete(m, _) => lower_multiplicity(m),
                    ast_type::MultiplicityArgument::Identifier(_, _) => Multiplicity::ZeroOrMany,
                });

        parameters.push((param_type, param_mult));
    }

    // Return type is the last type_argument
    let return_ref = &type_ref.type_arguments[n - 1];
    let return_type =
        resolve_type_ref(return_ref, ctx, errors).unwrap_or(TypeExpr::Generic("Any".into()));

    // Return multiplicity is in the top-level multiplicity_arguments[0]
    let return_multiplicity =
        type_ref
            .multiplicity_arguments
            .first()
            .map_or(Multiplicity::PureOne, |ma| match ma {
                ast_type::MultiplicityArgument::Concrete(m, _) => lower_multiplicity(m),
                ast_type::MultiplicityArgument::Identifier(_, _) => Multiplicity::ZeroOrMany,
            });

    Some(TypeExpr::FunctionType {
        parameters,
        return_type: Box::new(return_type),
        return_multiplicity,
    })
}

/// Decodes a `(RelationType)` sentinel `TypeReference` into
/// `TypeExpr::Relation(columns)`.
///
/// The parser encodes a structural relation type
/// `(name1:Type1[mult1], name2:Type2[mult2], …)` (when it appears in
/// a parameter type position via `Parameter::type_ref`) as a synthetic
/// `TypeReference` with name `(RelationType)`. Each column is itself
/// a `TypeReference` whose `name` is the column name, whose
/// `type_arguments[0]` is the column's type, and whose
/// `multiplicity_arguments[0]` is the column's multiplicity.
fn resolve_relation_type_sentinel(
    type_ref: &ast_type::TypeReference,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<TypeExpr> {
    let mut columns = Vec::with_capacity(type_ref.type_arguments.len());
    for col_ref in &type_ref.type_arguments {
        let type_expr = col_ref
            .type_arguments
            .first()
            .and_then(|t| resolve_type_ref(t, ctx, errors))
            .unwrap_or(TypeExpr::Generic("Any".into()));
        let multiplicity =
            col_ref
                .multiplicity_arguments
                .first()
                .map_or(Multiplicity::ZeroOrOne, |ma| match ma {
                    ast_type::MultiplicityArgument::Concrete(m, _) => lower_multiplicity(m),
                    ast_type::MultiplicityArgument::Identifier(_, _) => Multiplicity::ZeroOrMany,
                });
        columns.push(crate::types::RelationColumnTypeExpr {
            name: col_ref.name.clone(),
            type_expr,
            multiplicity,
        });
    }
    Some(TypeExpr::Relation(columns))
}

/// Resolves an AST `TypeSpec` (type, unit reference, or relation type) to a Pure `TypeExpr`.
///
/// For regular types, delegates to [`resolve_type_ref`].
/// For unit references (`Measure~UnitName`), resolves to the specific Unit
/// element by looking up its `Measure~UnitName` FQN.
/// For relation types, resolves each column type and interns the relation.
pub(crate) fn resolve_type_spec(
    type_spec: &ast_type::TypeSpec,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<TypeExpr> {
    match type_spec {
        ast_type::TypeSpec::Type(tr) => resolve_type_ref(tr, ctx, errors),
        ast_type::TypeSpec::Unit(ur) => {
            // Unit FQN in the model: "pkg::Measure~UnitName"
            // The element name stored in the model is "Measure~UnitName"
            let unit_element_name = SmolStr::new(format!("{}~{}", ur.measure.name, ur.unit));

            let element_id = if let Some(pkg) = &ur.measure.package {
                // Qualified — resolve via the AST Package tree
                ctx.model.resolve_in_package(pkg, &unit_element_name)
            } else {
                // Unqualified — look in root package
                ctx.model
                    .resolve_by_path(std::slice::from_ref(&unit_element_name))
            };

            if let Some(id) = element_id {
                Some(TypeExpr::Named {
                    element: id,
                    type_arguments: vec![],
                    multiplicity_arguments: Vec::new(),
                    value_arguments: vec![],
                })
            } else {
                let display = SmolStr::new(format!("{}~{}", ur.measure.full_path(), ur.unit));
                errors.push(CompilationError {
                    message: format!("Cannot resolve unit '{display}'"),
                    source_info: ur.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement { path: display },
                });
                None
            }
        }
        ast_type::TypeSpec::Relation(rt) => {
            // `@(cols)` and `Relation<(cols)>` resolve to
            // `RelationType<Relation(cols)>`: the outer wrapper is the
            // M3 `RelationType` Class so dispatch on
            // `RelationType<Any>[1]`-typed signatures still narrows,
            // and the inner `TypeExpr::Relation(...)` carries the
            // column shape (name, type, multiplicity per column) for
            // consumers that want it.
            let segments: [SmolStr; 5] = [
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("relation"),
                SmolStr::new("RelationType"),
            ];
            let rt_element = ctx.model.resolve_by_path(&segments)?;
            let columns = resolve_relation_columns(&rt.columns, ctx, errors);
            Some(TypeExpr::Named {
                element: rt_element,
                type_arguments: vec![TypeExpr::Relation(columns)],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
            })
        }
        ast_type::TypeSpec::Function(ft) => {
            // Function types are structural: {ParamType[mult] -> RetType[mult]}.
            // Resolve each parameter type and the return type.
            let parameters: Vec<(TypeExpr, Multiplicity)> = ft
                .parameters
                .iter()
                .map(|p| {
                    let te = resolve_type_ref(&p.type_ref, ctx, errors)
                        .unwrap_or(TypeExpr::Generic("Any".into()));
                    let mult = lower_multiplicity(&p.multiplicity);
                    (te, mult)
                })
                .collect();
            let return_type = resolve_type_ref(&ft.return_type, ctx, errors)
                .unwrap_or(TypeExpr::Generic("Any".into()));
            let return_multiplicity = lower_multiplicity(&ft.return_multiplicity);
            Some(TypeExpr::FunctionType {
                parameters,
                return_type: Box::new(return_type),
                return_multiplicity,
            })
        }
    }
}

/// Resolve AST `RelationColumn`s into the typed
/// [`RelationColumnTypeExpr`] used inside `TypeExpr::Relation`.
///
/// Mirrors what `lower_relation_columns` does in `lower.rs` for the
/// `RelationLiteral` lowering, but stays at the type-expression
/// level — used for resolving `(cols)` / `Relation<(cols)>` /
/// `TDS<(cols)>` syntactic shapes when they appear in a type position.
fn resolve_relation_columns(
    cols: &[ast_type::RelationColumn],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<crate::types::RelationColumnTypeExpr> {
    cols.iter()
        .filter_map(|c| {
            let type_expr = resolve_type_ref(&c.type_ref, ctx, errors)?;
            let multiplicity = c
                .multiplicity
                .as_ref()
                .map_or(Multiplicity::ZeroOrOne, lower_multiplicity);
            Some(crate::types::RelationColumnTypeExpr {
                name: c.name.clone(),
                type_expr,
                multiplicity,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Core Name Resolution (Import-Aware, Memoized)
// ---------------------------------------------------------------------------

/// Memoized unqualified name resolution.
///
/// Checks the per-section cache first. On a cache miss, delegates to
/// `resolve_unqualified` and caches the result.
fn resolve_unqualified_cached(
    name: &SmolStr,
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ElementId> {
    // Check memo cache
    if let Some(cached) = ctx.resolve_cache.get(name) {
        return match cached {
            ResolveResult::Found(id) => Some(*id),
            ResolveResult::Failed(cached_error) => {
                // Re-emit the same error kind for this occurrence
                errors.push(CompilationError {
                    message: cached_error.message.clone(),
                    source_info: source_info.clone(),
                    kind: cached_error.kind.clone(),
                });
                None
            }
        };
    }

    // Resolve and cache
    let pre_errors = errors.len();
    let result = resolve_unqualified(name, source_info, ctx, errors);
    let cache_entry = if let Some(id) = result {
        ResolveResult::Found(id)
    } else {
        // Cache the error that was just emitted (if any)
        let cached = errors
            .get(pre_errors)
            .cloned()
            .unwrap_or_else(|| CompilationError {
                message: format!("Cannot resolve element '{name}'"),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: name.clone() },
            });
        ResolveResult::Failed(cached)
    };
    ctx.resolve_cache.insert(name.clone(), cache_entry);
    result
}

/// Resolves an unqualified name through import packages.
///
/// Uses `model.resolve_in_package()` with the AST `Package` directly —
/// no string concatenation, splitting, or segment vectors needed.
fn resolve_unqualified(
    name: &SmolStr,
    source_info: &SourceInfo,
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ElementId> {
    // Step 1: Primitives + `Any` / `Nil` always shadow imports. They
    // sit at root as part of the bootstrap and the language treats
    // `Boolean[1]`, `Integer[1]`, `Any[*]` etc. as reserved — an
    // imported same-named class (e.g.
    // `meta::relational::metamodel::datatype::Boolean` brought in by
    // `import meta::relational::metamodel::datatype::*`) must NOT win
    // over the primitive when a Pure source writes the bare name.
    if let Some(id) = ctx.model.resolve_by_path(std::slice::from_ref(name)) {
        if let Some(elem) = ctx.model.try_get_element(id) {
            if matches!(
                elem,
                crate::model::Element::PrimitiveType(_)
                    | crate::model::Element::Class(_) // captures `Any` / `Nil` only via the
                                                      // `is_root_class_alias` filter below
            ) {
                use crate::ids::ElementId as Eid;
                let is_any_or_nil = matches!(
                    id,
                    Eid::InstanceId { chunk_id: 0, local_idx: 0 } // ANY
                    | Eid::InstanceId { chunk_id: 0, local_idx: 1 } // NIL
                );
                if matches!(elem, crate::model::Element::PrimitiveType(_)) || is_any_or_nil {
                    return Some(id);
                }
            }
        }
    }

    // Step 2: Search the file's explicit + auto imports.
    //
    // Imports take precedence over the *non-primitive* root fallback.
    // Bootstrap registers M3 metamodel classes (e.g.
    // `meta::pure::metamodel::relation::Column<U,V>`,
    // `meta::pure::metamodel::function::Function`) as aliases at root —
    // that's how unqualified references resolve when no import brings a
    // sibling into scope. But when an import *does* bring a sibling
    // (e.g. `import meta::relational::metamodel::*` exposes the concrete
    // `Column` next to the M3 generic), the user means the imported one.
    // Without this ordering, bare `Column` in
    // `platform_store_relational/functions.pure` short-circuited to the
    // M3 `Column<U,V>` and downstream code saw it carrying no type args.
    let mut candidates: Vec<(&ImportScope, ElementId)> = Vec::new();
    for scope in ctx.import_scopes {
        if let Some(id) = ctx.model.resolve_in_package(&scope.package, name) {
            candidates.push((scope, id));
        }
    }

    // Step 3: Fall back to root-level M3 metaclass aliases (`Function`,
    // `Class`, `Property`, `Column`, …) when no import contributed.
    if candidates.is_empty() {
        if let Some(id) = ctx.model.resolve_by_path(std::slice::from_ref(name)) {
            return Some(id);
        }
    }

    match candidates.len() {
        0 => {
            errors.push(CompilationError {
                message: format!("Cannot resolve element '{name}'"),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: name.clone() },
            });
            None
        }
        1 => Some(candidates[0].1),
        _ => {
            // Deduplicate by ElementId — different imports may resolve to same element
            candidates.dedup_by_key(|c| c.1);
            if candidates.len() == 1 {
                return Some(candidates[0].1);
            }
            // Build path strings only for the error message
            let candidate_paths: Vec<SmolStr> = candidates
                .iter()
                .map(|(scope, _)| SmolStr::new(format!("{}::{name}", scope.package)))
                .collect();
            errors.push(CompilationError {
                message: format!(
                    "'{}' has been found more than one time in the imports: [{}]",
                    name,
                    candidate_paths
                        .iter()
                        .map(SmolStr::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::AmbiguousImport {
                    name: name.clone(),
                    candidates: candidate_paths,
                },
            });
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Multiplicity Lowering
// ---------------------------------------------------------------------------

/// Converts an AST `Multiplicity` to the Pure `Multiplicity`.
///
/// This is a direct 1:1 mapping — the AST and Pure enums are structurally
/// identical, but the Pure variant drops source location metadata.
pub(crate) fn lower_multiplicity(m: &ast_type::Multiplicity) -> Multiplicity {
    match m {
        ast_type::Multiplicity::ZeroOrOne => Multiplicity::ZeroOrOne,
        ast_type::Multiplicity::PureOne => Multiplicity::PureOne,
        ast_type::Multiplicity::ZeroOrMany => Multiplicity::ZeroOrMany,
        ast_type::Multiplicity::Variable(name) => Multiplicity::Variable(name.clone()),
        ast_type::Multiplicity::OneOrMany => Multiplicity::OneOrMany,
        ast_type::Multiplicity::Range { lower, upper } => Multiplicity::Range {
            lower: *lower,
            upper: *upper,
        },
    }
}

// ---------------------------------------------------------------------------
// Const Value Lowering
// ---------------------------------------------------------------------------

/// Converts an AST `TypeVariableValue` to a Pure `ConstValue`.
pub(crate) fn lower_const_value(v: &ast_type::TypeVariableValue) -> ConstValue {
    match v {
        ast_type::TypeVariableValue::Integer(i, _) => ConstValue::Integer(*i),
        ast_type::TypeVariableValue::String(s, _) => ConstValue::String(s.clone()),
    }
}

// ---------------------------------------------------------------------------
// Annotation Resolution
// ---------------------------------------------------------------------------

/// Resolves AST `StereotypePtr` references to Pure `StereotypeRef`s.
///
/// Stereotypes reference a Profile element + a stereotype name within it.
/// If the Profile cannot be resolved, an error is pushed and the stereotype
/// is skipped.
///
/// `resolve_element_ptr`'s last-resort fallback is to return a child
/// package of root — legitimate for positions where a Package is a
/// PackageableElement value (e.g. `elementPath(meta)`), but a Package
/// can never be a valid stereotype profile. When the fallback fires
/// for a stereotype, [`resolve_element_ptr`] also *pops* the original
/// `UnresolvedElement` error it previously pushed. We re-emit it here
/// so the user sees one clear "couldn't find the profile" diagnostic
/// instead of a misleading "Package isn't a Profile" downstream.
pub(crate) fn resolve_stereotypes(
    stereotypes: &[ast_ann::StereotypePtr],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<StereotypeRef> {
    stereotypes
        .iter()
        .filter_map(|s| {
            let profile_id = resolve_element_ptr(&s.profile, &s.source_info, ctx, errors)?;
            if matches!(profile_id, ElementId::Package(_)) {
                errors.push(CompilationError {
                    message: format!(
                        "Cannot resolve stereotype profile '{}'",
                        s.profile.name()
                    ),
                    source_info: s.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: s.profile.name().clone(),
                    },
                });
                return None;
            }
            // Eager kind + name-existence check. Sound here because
            // `create_shell` (Pass 1) populates `Profile.stereotypes`
            // from the AST, so the referenced profile's declared
            // stereotype list is visible regardless of topological
            // hydration order.
            match ctx.model.try_get_element(profile_id) {
                Some(Element::Profile(profile)) => {
                    if !profile.stereotypes.iter().any(|name| name == &s.value) {
                        let profile_stereos: Vec<&str> =
                            profile.stereotypes.iter().map(SmolStr::as_str).collect();
                        errors.push(CompilationError {
                            message: format!(
                                "Stereotype '{}' does not exist in the Profile. \
                                 Available stereotypes: [{}]",
                                s.value,
                                profile_stereos.join(", ")
                            ),
                            source_info: s.source_info.clone(),
                            kind: CompilationErrorKind::InvalidAnnotation {
                                element_name: ctx.model.element_name(profile_id).clone(),
                                reason: SmolStr::new(format!(
                                    "stereotype '{}' not found",
                                    s.value
                                )),
                            },
                        });
                    }
                }
                Some(_) => {
                    let target_name = ctx.model.element_name(profile_id).clone();
                    errors.push(CompilationError {
                        message: format!(
                            "Stereotype target '{target_name}' is not a Profile"
                        ),
                        source_info: s.source_info.clone(),
                        kind: CompilationErrorKind::InvalidAnnotation {
                            element_name: target_name.clone(),
                            reason: SmolStr::new(format!(
                                "'{target_name}' is not a Profile"
                            )),
                        },
                    });
                }
                None => {} // Resolution error already reported upstream.
            }
            Some(StereotypeRef {
                profile: profile_id,
                value: s.value.clone(),
            })
        })
        .collect()
}

/// Resolves AST `TaggedValue` references to Pure `TaggedValueRef`s.
///
/// Tagged values reference a Profile element + a tag name + a string value.
/// If the Profile cannot be resolved, an error is pushed and the tagged value
/// is skipped. Same Package-fallback caveat as
/// [`resolve_stereotypes`] — a Package is never a valid tag profile,
/// so we re-emit the resolver's popped error and skip.
pub(crate) fn resolve_tagged_values(
    tagged_values: &[ast_ann::TaggedValue],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<TaggedValueRef> {
    tagged_values
        .iter()
        .filter_map(|tv| {
            let profile_id = resolve_element_ptr(&tv.tag.profile, &tv.source_info, ctx, errors)?;
            if matches!(profile_id, ElementId::Package(_)) {
                errors.push(CompilationError {
                    message: format!(
                        "Cannot resolve tagged-value profile '{}'",
                        tv.tag.profile.name()
                    ),
                    source_info: tv.source_info.clone(),
                    kind: CompilationErrorKind::UnresolvedElement {
                        path: tv.tag.profile.name().clone(),
                    },
                });
                return None;
            }
            // Eager kind + tag-name check — symmetric with
            // `resolve_stereotypes` and sound for the same reason
            // (Pass-1 shell populates `Profile.tags`).
            match ctx.model.try_get_element(profile_id) {
                Some(Element::Profile(profile)) => {
                    if !profile.tags.iter().any(|name| name == &tv.tag.value) {
                        let profile_tags: Vec<&str> =
                            profile.tags.iter().map(SmolStr::as_str).collect();
                        errors.push(CompilationError {
                            message: format!(
                                "Tag '{}' does not exist in the Profile. \
                                 Available tags: [{}]",
                                tv.tag.value,
                                profile_tags.join(", ")
                            ),
                            source_info: tv.source_info.clone(),
                            kind: CompilationErrorKind::InvalidAnnotation {
                                element_name: ctx.model.element_name(profile_id).clone(),
                                reason: SmolStr::new(format!(
                                    "tag '{}' not found",
                                    tv.tag.value
                                )),
                            },
                        });
                    }
                }
                Some(_) => {
                    let target_name = ctx.model.element_name(profile_id).clone();
                    errors.push(CompilationError {
                        message: format!(
                            "Tag target '{target_name}' is not a Profile"
                        ),
                        source_info: tv.source_info.clone(),
                        kind: CompilationErrorKind::InvalidAnnotation {
                            element_name: target_name.clone(),
                            reason: SmolStr::new(format!(
                                "'{target_name}' is not a Profile"
                            )),
                        },
                    });
                }
                None => {} // Resolution error already reported upstream.
            }
            Some(TaggedValueRef {
                profile: profile_id,
                tag: tv.tag.value.clone(),
                value: tv.value.clone(),
            })
        })
        .collect()
}

/// Resolves a `PackageableElementPtr` to an `ElementId`.
///
/// Uses the AST `Package` directly for qualified refs, or goes through
/// import-aware resolution for unqualified refs.
pub(crate) fn resolve_element_ptr(
    ptr: &ast_ann::PackageableElementPtr,
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ElementId> {
    // Handle root package literal `::` — empty name with no package
    if ptr.name().is_empty() && ptr.package().is_none() {
        return Some(ElementId::Package(ctx.model.root_package));
    }

    // Handle `Root` as an alias for the root package (Java `M3Paths.Root`).
    if ptr.package().is_none() && ptr.name() == "Root" {
        return Some(ElementId::Package(ctx.model.root_package));
    }

    if let Some(pkg) = ptr.package() {
        // Qualified — resolve directly via the AST Package tree
        if let Some(id) = ctx.model.resolve_in_package(pkg, ptr.name()) {
            Some(id)
        } else {
            let display = SmolStr::new(format!("{}::{}", pkg, ptr.name()));
            errors.push(CompilationError {
                message: format!("Cannot resolve element '{display}'"),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: display },
            });
            None
        }
    } else {
        // Unqualified — go through import-aware resolution
        let result = resolve_unqualified_cached(ptr.name(), source_info, ctx, errors);
        if result.is_some() {
            return result;
        }

        // Fallback: try resolving as a child package of root.
        // In Pure, packages are PackageableElements and can be used as values
        // (e.g., `elementPath(meta)`, `[::, meta, meta::pure]`).
        let root = ctx.model.get_package(ctx.model.root_package);
        for &child_id in &root.children_packages {
            if ctx.model.get_package(child_id).name == *ptr.name() {
                // Remove the error that resolve_unqualified_cached added
                if errors.last().is_some_and(|e| {
                    matches!(&e.kind, CompilationErrorKind::UnresolvedElement { path } if path == ptr.name())
                }) {
                    errors.pop();
                }
                return Some(ElementId::Package(child_id));
            }
        }

        None
    }
}

/// Resolves a function call by simple name.
///
/// Unlike `resolve_element_ptr` (which matches by exact element name),
/// this searches `Function.function_name` — the simple name — across
/// import scopes.
///
/// `arg_count` is the number of arguments at the call site (from the AST).
/// `lowered_args` are the already-compiled argument expressions, used for
/// type-based narrowing when multiple overloads share the same param count.
#[tracing::instrument(
    name = "resolve_function_call",
    level = "debug",
    skip(lowered_args, ctx, errors),
    fields(
        name = %ptr.name(),
        package = ?ptr.package().map(std::string::ToString::to_string),
        arg_count,
        n_imports = ctx.import_scopes.len(),
    ),
)]
#[allow(clippy::too_many_lines, clippy::single_match_else)]
pub(crate) fn resolve_function_call(
    ptr: &ast_ann::PackageableElementPtr,
    arg_count: usize,
    lowered_args: &[crate::types::ValueSpec],
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ElementId> {
    let name = ptr.name();

    if let Some(pkg) = ptr.package() {
        // Qualified — search in the specified package by simple name
        if let Some(pkg_id) = ctx.model.resolve_package(pkg) {
            let candidates = ctx.model.resolve_functions_by_name_in_package(pkg_id, name);
            let filtered: Vec<_> = candidates
                .into_iter()
                .filter(|&eid| {
                    if let Element::Function(f) = ctx.model.get_element(eid) {
                        f.parameters.len() == arg_count
                    } else {
                        false
                    }
                })
                .collect();
            let narrowed =
                narrow_candidates_by_type(&filtered, lowered_args, ctx.model, &ctx.variable_types);
            if narrowed.len() == 1 {
                return Some(narrowed[0]);
            }
            // 0 or >1 candidates after narrowing — fall through to exact match
        }
        // Fall back to exact element match (e.g., mangled name used directly)
        if let Some(id) = ctx.model.resolve_in_package(pkg, name) {
            return Some(id);
        }
        let display = SmolStr::new(format!("{pkg}::{name}"));
        errors.push(CompilationError {
            message: format!("Cannot resolve element '{display}'"),
            source_info: source_info.clone(),
            kind: CompilationErrorKind::UnresolvedElement { path: display },
        });
        None
    } else {
        // Unqualified — search import scopes by function_name
        // Step 1: Try root package
        let root_candidates = ctx
            .model
            .resolve_functions_by_name_in_package(ctx.model.root_package, name);
        let root_filtered: Vec<_> = root_candidates
            .into_iter()
            .filter(|&eid| {
                if let Element::Function(f) = ctx.model.get_element(eid) {
                    f.parameters.len() == arg_count
                } else {
                    false
                }
            })
            .collect();
        tracing::debug!(
            n_root_filtered = root_filtered.len(),
            "step 1: root-package lookup"
        );
        if root_filtered.len() == 1 {
            tracing::debug!(eid = ?root_filtered[0], "step 1 short-circuit: single root match");
            return Some(root_filtered[0]);
        }

        // Step 2: Search import scopes — collect overloads per package
        let mut all_candidates: Vec<ElementId> = Vec::new();
        let mut contributing_packages: Vec<SmolStr> = Vec::new();
        for scope in ctx.import_scopes {
            if let Some(pkg_id) = ctx.model.resolve_package(&scope.package) {
                let found = ctx.model.resolve_functions_by_name_in_package(pkg_id, name);
                // Filter by parameter count
                let filtered: Vec<_> = found
                    .into_iter()
                    .filter(|&eid| {
                        if let Element::Function(f) = ctx.model.get_element(eid) {
                            f.parameters.len() == arg_count
                        } else {
                            false
                        }
                    })
                    .collect();
                if !filtered.is_empty() {
                    let pkg_name = SmolStr::new(format!("{}::{name}", scope.package));
                    if !contributing_packages.contains(&pkg_name) {
                        contributing_packages.push(pkg_name);
                    }
                    all_candidates.extend(filtered);
                }
            }
        }
        // Dedup: the same function ElementId can be discovered through
        // multiple import scopes (e.g. an explicit `import pkg::*` plus
        // an auto-import covering the same package). Without this, a
        // single overload becomes N candidates and trips the
        // multi-overload narrowing path even though there's nothing to
        // disambiguate. ElementId doesn't implement Ord, so dedup using
        // a HashSet-style pass that preserves insertion order.
        {
            let mut seen: std::collections::HashSet<ElementId> =
                std::collections::HashSet::with_capacity(all_candidates.len());
            all_candidates.retain(|eid| seen.insert(*eid));
        }
        tracing::debug!(
            n_candidates = all_candidates.len(),
            packages = ?contributing_packages,
            "step 2: import-scope lookup"
        );

        if all_candidates.is_empty() {
            tracing::warn!("no candidates — UnresolvedElement");
            // No function with this simple name and arg count in any import scope
            errors.push(CompilationError {
                message: format!("Cannot resolve function '{name}'"),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: name.clone() },
            });
            None
        } else if all_candidates.len() == 1 {
            tracing::debug!(eid = ?all_candidates[0], "single candidate from imports");
            Some(all_candidates[0])
        } else {
            // Multiple overloads with same param count — narrow by type
            let narrowed = narrow_candidates_by_type(
                &all_candidates,
                lowered_args,
                ctx.model,
                &ctx.variable_types,
            );
            tracing::debug!(
                n_narrowed = narrowed.len(),
                from = all_candidates.len(),
                "narrowed by type"
            );
            if narrowed.len() == 1 {
                return Some(narrowed[0]);
            }
            tracing::warn!(
                n_narrowed = narrowed.len(),
                from = all_candidates.len(),
                "AmbiguousImport — narrowing did not yield single candidate"
            );
            // Suppress the cascade when this ambiguity is caused by
            // reading lambda parameters typed `TypeExpr::Unresolved`.
            // `lower_lambda_parameters` already pushed a single
            // `CannotInferLambdaParameterTypes` at the lambda's source
            // span; the redundant "Ambiguous function call" diagnostic
            // would only bury it.
            if any_arg_reads_unresolved(lowered_args, &ctx.variable_types) {
                tracing::debug!(
                    "suppressed AmbiguousImport — caused by Unresolved lambda parameter \
                     (lambda-level CannotInferLambdaParameterTypes already emitted)"
                );
                return None;
            }
            errors.push(CompilationError {
                message: format!(
                    "Ambiguous function call '{name}': found {} overloads with {} args \
                     (narrowed from {} candidates)",
                    narrowed.len(),
                    arg_count,
                    all_candidates.len(),
                ),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::AmbiguousImport {
                    name: name.clone(),
                    candidates: contributing_packages,
                },
            });
            None
        }
    }
}

/// Recursively collects the names of variables whose stored type is
/// `TypeExpr::Unresolved` — i.e. lambda parameters that the compiler
/// could not infer. Operator dispatch lowers `$x + $y` into a
/// `FunctionCall` whose only argument is a `Collection { elements:
/// [Variable, Variable] }`, so the walk descends `Collection`s and
/// `FunctionCall` args. `seen` keeps names unique while preserving
/// discovery order via `out`.
fn collect_unresolved_param_reads(
    vs: &crate::types::ValueSpec,
    var_types: &VarTypes,
    out: &mut Vec<SmolStr>,
    seen: &mut std::collections::HashSet<SmolStr>,
) {
    use crate::types::{ExprKind, TypeExpr};
    match vs.kind.as_ref() {
        ExprKind::Variable { name } => {
            if matches!(var_types.get(name), Some((TypeExpr::Unresolved, _)))
                && seen.insert(name.clone())
            {
                out.push(name.clone());
            }
        }
        ExprKind::Collection { elements } => {
            for e in elements {
                collect_unresolved_param_reads(e, var_types, out, seen);
            }
        }
        ExprKind::FunctionCall(data)
        | ExprKind::PropertyCall(data)
        | ExprKind::QualifiedPropertyCall(data) => {
            for a in &data.arguments {
                collect_unresolved_param_reads(a, var_types, out, seen);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Type-based dispatch helpers
// ---------------------------------------------------------------------------

/// Infers a type `ElementId` from a lowered `ValueSpec` by examining
/// its `ExprKind` structure. Returns `None` for expressions whose type
/// cannot be statically determined (treated as `Any` — matches everything).
/// Type alias for variable scope: name → (type, multiplicity).
pub(crate) type VarTypes = HashMap<SmolStr, (crate::types::TypeExpr, crate::types::Multiplicity)>;

#[allow(clippy::too_many_lines)]
fn infer_type_from_valuespec(
    vs: &crate::types::ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Option<ElementId> {
    use crate::bootstrap;
    use crate::types::ExprKind;

    match vs.kind.as_ref() {
        ExprKind::IntegerLiteral(_) => Some(bootstrap::INTEGER_ID),
        ExprKind::FloatLiteral(_) => Some(bootstrap::FLOAT_ID),
        ExprKind::DecimalLiteral(_) => Some(bootstrap::DECIMAL_ID),
        ExprKind::StringLiteral(_) => Some(bootstrap::STRING_ID),
        ExprKind::BooleanLiteral(_) => Some(bootstrap::BOOLEAN_ID),
        ExprKind::DateLiteral(dv) => {
            use crate::types::DateValue;
            match dv {
                DateValue::StrictDate { .. } => Some(bootstrap::STRICT_DATE_ID),
                DateValue::DateTime { .. } => Some(bootstrap::DATE_TIME_ID),
                DateValue::StrictTime { .. } => Some(bootstrap::STRICT_TIME_ID),
            }
        }
        ExprKind::TypeReference { type_expr } => {
            // A `@Foo` expression is a type-token value. For dispatch,
            // expose the wrapped type's element so `cast<T>(_, @Class<Any>)`
            // can bind T to `Class<Any>` and surface the concrete element.
            if let crate::types::TypeExpr::Named { element, .. } = type_expr {
                Some(*element)
            } else {
                None
            }
        }
        ExprKind::PropertyCall(data) | ExprKind::QualifiedPropertyCall(data) => {
            // Property / qualified-property invocation: resolve via
            // class property lookup, not function dispatch. `arguments[0]`
            // is the receiver. Mirrors the Java `propertyExpression`
            // post-processor.
            let target = data.arguments.first()?;

            let target_te = infer_typeexpr_from_valuespec(target, model, var_types);
            if let Some(te) = &target_te
                && let Some(cols) = extract_relation_columns(te)
            {
                for col in cols {
                    if col.name == data.function_name {
                        return match &col.type_expr {
                            crate::types::TypeExpr::Named { element, .. } => Some(*element),
                            // Unbound generic: report `None` (unknown), not
                            // `Any`. The narrower distinguishes "couldn't
                            // infer" from "positively top type" — the former
                            // accepts every param via the unknown-arg-permits
                            // branch in `is_type_compatible`; the latter would
                            // be rejected against non-Any params.
                            crate::types::TypeExpr::Generic(_) => None,
                            _ => None,
                        };
                    }
                }
                // Property not found in columns — fall through to the
                // class-property path so the user gets a coherent
                // diagnostic instead of a silent None.
            }

            let target_eid = infer_type_from_valuespec(target, model, var_types)?;
            let receiver_type_args = extract_receiver_type_args(target, var_types);
            let (prop_ty_owned, type_params_owned) =
                find_property_with_inheritance(target_eid, &data.function_name, model)?;
            let resolved =
                substitute_class_generics(&prop_ty_owned, &type_params_owned, &receiver_type_args);
            match resolved {
                crate::types::TypeExpr::Named { element, .. } => Some(element),
                // Unbound generic from class-property substitution: report
                // `None` (unknown). See sibling note above.
                crate::types::TypeExpr::Generic(_) => None,
                _ => None,
            }
        }
        ExprKind::FunctionCall(FunctionCallData {
            function,
            function_name,
            arguments,
        }) => {
            // Use the return type of the resolved function, with generic
            // type variables (`T`) bound from call-site arguments.
            if let Some(fid) = function
                && let Element::Function(f) = model.get_element(*fid)
            {
                let bindings = infer_generic_bindings(&f.parameters, arguments, model, var_types);
                let substituted = substitute_type(&f.return_type, &bindings.ty);
                match &substituted {
                    crate::types::TypeExpr::Named { element, .. } => return Some(*element),
                    // Unbound type variable: report `None` (unknown), not
                    // `Any`. Narrowing relies on the distinction —
                    // unknown-arg permits every overload (via
                    // `is_type_compatible`'s None branch) so Phase 2 rank
                    // can disambiguate by remaining args. Widening to Any
                    // would instead reject every non-Any param and force
                    // the empty-fallback ambiguity.
                    crate::types::TypeExpr::Generic(_) => return None,
                    _ => {}
                }
            }
            // Well-known function return types — these operators always return
            // a specific type regardless of resolution status.
            match function_name.as_str() {
                // Comparison/equality/logical/assert family → Boolean
                "equal" | "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual"
                | "is" | "in" | "contains" | "startsWith" | "endsWith" | "isEmpty"
                | "isNotEmpty" | "and" | "or" | "not" | "assert" | "assertFalse"
                | "assertEquals" | "assertNotEquals" | "assertNotEmpty" | "assertEmpty"
                | "assertSize" | "assertSameElements" | "assertIs" | "assertIsNot"
                | "assertContains" | "assertNotContains" | "assertNotSize" => {
                    return Some(bootstrap::BOOLEAN_ID);
                }
                // String operations
                "toString" | "format" | "toRepresentation" | "joinStrings" => {
                    return Some(bootstrap::STRING_ID);
                }
                _ => {}
            }
            // `new(Class, name, keys)` → return type is the class itself
            // `dynamicNew(Class, keys)` → same pattern
            // `copy(src, keys)` → return type is the receiver
            if matches!(function_name.as_str(), "new" | "dynamicNew")
                && let Some(first_arg) = arguments.first()
                && let ExprKind::PackageableElementRef { element } = first_arg.kind.as_ref()
            {
                return Some(*element);
            }
            // When the function is unresolved and there's no dedicated rule
            // above, give up. The "fallback from first arg" heuristic that
            // used to live here masked real dispatch bugs whenever a
            // non-type-preserving function failed to resolve; generic
            // substitution replaces it for resolved type-preserving calls.
            None
        }
        ExprKind::Variable { name } => {
            // Look up declared type from function params / let / lambda
            var_types.get(name).and_then(|(te, _)| match te {
                crate::types::TypeExpr::Named { element, .. } => Some(*element),
                _ => None,
            })
        }
        ExprKind::EnumValue { enum_element, .. } => Some(*enum_element),
        ExprKind::Collection { elements } => {
            // Compute the LUB (least upper bound) of all element types.
            // [1, 2, 3] → Integer, [1, 2.5] → Number, ["a", "b"] → String.
            let mut lub: Option<ElementId> = None;
            for (i, elem) in elements.iter().enumerate() {
                let elem_type = infer_type_from_valuespec(elem, model, var_types);
                tracing::trace!(
                    i,
                    elem_kind = format!("{:?}", elem.kind).chars().take(80).collect::<String>(),
                    elem_type = ?elem_type.map(|e| model.element_name(e).to_string()),
                    "Collection LUB element"
                );
                if let Some(elem_type) = elem_type {
                    lub = Some(match lub {
                        None => elem_type,
                        Some(current) => {
                            let merged = least_upper_bound(current, elem_type, model);
                            tracing::trace!(
                                lhs = %model.element_name(current),
                                rhs = %model.element_name(elem_type),
                                merged = %model.element_name(merged),
                                "LUB merge"
                            );
                            merged
                        }
                    });
                }
            }
            lub
        }
        ExprKind::PackageableElementRef { element } => {
            // Bare element ref has its M3 metatype — a Class value has
            // metatype `meta::pure::metamodel::type::Class`, etc. Enables
            // dispatch on overloads like `dynamicNew(Class<Any>[1], ...)`
            // vs `dynamicNew(GenericType[1], ...)`.
            crate::bootstrap::metatype_of(model, model.get_element(*element))
        }
        // Relation-grammar column-spec literals: surface the
        // discriminator-driven classifier so overload narrowing on
        // `ColSpec<T>[1]` / `FuncColSpec<…,T>[1]` / `AggColSpec<…,T>[1]`
        // (and their array variants) can dispatch correctly.
        ExprKind::ColSpecLiteral { kind, .. } => {
            let class_name = match kind {
                crate::types::ColSpecLiteralKind::Plain => "ColSpec",
                crate::types::ColSpecLiteralKind::Func => "FuncColSpec",
                crate::types::ColSpecLiteralKind::Agg => "AggColSpec",
            };
            model.resolve_by_path(&[
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("relation"),
                SmolStr::new(class_name),
            ])
        }
        ExprKind::ColSpecArrayLiteral { kind, .. } => {
            let class_name = match kind {
                crate::types::ColSpecLiteralKind::Plain => "ColSpecArray",
                crate::types::ColSpecLiteralKind::Func => "FuncColSpecArray",
                crate::types::ColSpecLiteralKind::Agg => "AggColSpecArray",
            };
            model.resolve_by_path(&[
                SmolStr::new("meta"),
                SmolStr::new("pure"),
                SmolStr::new("metamodel"),
                SmolStr::new("relation"),
                SmolStr::new(class_name),
            ])
        }
        // PropertyCall / QualifiedPropertyCall arms live above — property
        // invocation is handled before the FunctionCall arm.
        _ => None,
    }
}

/// Sibling of [`infer_type_from_valuespec`] that returns the FULL
/// `TypeExpr` (preserving `type_arguments`) instead of just an
/// `ElementId`. Used where parametric bindings matter — most
/// importantly inside [`infer_generic_bindings`], where treating
/// `$l1: List<String>` as `Named{List, []}` (the cheap path's loss)
/// caused `T` in `class<T>(T[*]):Class<T>[1]` to bind to bare `List`
/// rather than `List<String>`. With `type_arguments` preserved, the
/// substituted return type comes out `Class<List<String>>`, and the
/// `new(Class<T>, '')` arm below can read T from arg[0]'s inferred
/// shape.
///
/// Mirrors `infer_type_from_valuespec`'s structure but returns
/// `Option<TypeExpr>`. Most arms just call back into the cheap fn
/// and wrap; the value-add is in the arms that carry parametric
/// bindings: `Variable` (read declared type from scope, including
/// `type_arguments`), `FunctionCall` (substitute T-bindings into the
/// resolved function's return type, preserving the result's
/// `type_arguments` — this is what makes `$l1->class()` produce
/// `Class<List<String>>` instead of bare `Class`),
/// `PropertyAccess` (substitute class-level generics).
pub(crate) fn infer_typeexpr_from_valuespec(
    vs: &crate::types::ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Option<crate::types::TypeExpr> {
    use crate::types::{ExprKind, TypeExpr};
    let bare = |eid: ElementId| TypeExpr::Named {
        element: eid,
        type_arguments: vec![],
        multiplicity_arguments: Vec::new(),
        value_arguments: vec![],
    };
    // Honour pre-set `type_info` first — this is the canonical
    // "lowering captures parametric type info; consumers read from
    // type_info" pattern documented in `reference_type_info_capture.md`.
    // `lower_new_instance` (for `^Class<T>(...)`) and
    // `lower_packageable_element_ref` (for bare `P` references that
    // need `Class<P>` parametric capture) both populate this slot;
    // reading it before kind-based inference avoids re-deriving what
    // the AST layer already knows.
    if let Some(rt) = vs.type_info.as_deref() {
        return Some(rt.type_expr.clone());
    }
    match vs.kind.as_ref() {
        // Variable: look up the declared TypeExpr and return it as-is —
        // this is the load-bearing case for Lane B. `var_types` stores
        // (TypeExpr, Multiplicity); the stored TypeExpr already carries
        // type_arguments from the parameter declaration.
        ExprKind::Variable { name } => var_types.get(name).map(|(te, _)| te.clone()),
        // TypeReference: the wrapped type literally is the value's type.
        ExprKind::TypeReference { type_expr } => Some(type_expr.clone()),
        // FunctionCall: substitute T-bindings from call-site args into
        // the resolved function's return type. This is what flows
        // `<String>` from `$l1: List<String>` through
        // `class<T>(T[*]):Class<T>[1]` to a `Class<List<String>>` result.
        ExprKind::FunctionCall(FunctionCallData {
            function,
            arguments,
            ..
        }) => {
            let fid = (*function)?;
            let crate::model::Element::Function(f) = model.get_element(fid) else {
                return None;
            };
            let bindings = infer_generic_bindings(&f.parameters, arguments, model, var_types);
            Some(substitute_type(&f.return_type, &bindings.ty))
        }
        // Property / qualified-property invocation: class property
        // lookup, not function dispatch. `arguments[0]` is the receiver.
        ExprKind::PropertyCall(data) | ExprKind::QualifiedPropertyCall(data) => {
            let target = data.arguments.first()?;

            // Structural-relation receiver: when the receiver's TypeExpr
            // is (or wraps) a `Relation(cols)`, the "property" is a
            // column name. Returns the column's resolved `TypeExpr`
            // (full shape, preserving parametric layers). Mirrors the
            // narrower-side arm in `infer_type_from_valuespec`.
            let target_te = infer_typeexpr_from_valuespec(target, model, var_types);
            if let Some(te) = &target_te
                && let Some(cols) = extract_relation_columns(te)
            {
                for col in cols {
                    if col.name == data.function_name {
                        return Some(col.type_expr.clone());
                    }
                }
            }

            let target_eid = infer_type_from_valuespec(target, model, var_types)?;
            let receiver_type_args = extract_receiver_type_args(target, var_types);
            let (prop_ty_owned, type_params_owned) =
                find_property_with_inheritance(target_eid, &data.function_name, model)?;
            Some(substitute_class_generics(
                &prop_ty_owned,
                &type_params_owned,
                &receiver_type_args,
            ))
        }
        // Everything else — degrade to the cheap fn and wrap as a bare
        // Named TypeExpr (no type_args, no value_args). Literals,
        // PackageableElementRef, EnumValue, Collection LUB all fall
        // here. Lane B doesn't currently need type_args for any of
        // these; if a future caller does, this is the place to refine.
        _ => infer_type_from_valuespec(vs, model, var_types).map(bare),
    }
}

/// Walk the class hierarchy searching for `property` starting at `eid`.
///
/// Returns the property's declared `TypeExpr` together with the type
/// parameters of the *declaring* class (so the caller can substitute them
/// against receiver type-arguments). Walks `super_types` breadth-first —
/// `.package` on a `Package` value must find the `package` declaration
/// inherited from `PackageableElement`.
pub(crate) fn find_property_with_inheritance(
    eid: ElementId,
    property: &smol_str::SmolStr,
    model: &crate::model::PureModel,
) -> Option<(crate::types::TypeExpr, Vec<smol_str::SmolStr>)> {
    use crate::types::TypeExpr;
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(eid);
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current) {
            continue;
        }
        let Element::Class(c) = model.get_element(current) else {
            continue;
        };
        if let Some(p) = c.properties.iter().find(|p| p.name == *property) {
            return Some((p.type_expr.clone(), c.type_parameters.clone()));
        }
        if let Some(q) = c.qualified_properties.iter().find(|q| q.name == *property) {
            return Some((q.return_type.clone(), c.type_parameters.clone()));
        }
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                queue.push_back(*element);
            }
        }
    }
    None
}

/// Walks the class hierarchy looking for `property`'s declared
/// multiplicity (sibling to [`find_property_with_inheritance`] but
/// returning the multiplicity instead of the type).
pub(crate) fn find_property_multiplicity(
    eid: ElementId,
    property: &smol_str::SmolStr,
    model: &crate::model::PureModel,
) -> Option<crate::types::Multiplicity> {
    use crate::types::TypeExpr;
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(eid);
    while let Some(current) = queue.pop_front() {
        if !visited.insert(current) {
            continue;
        }
        let Element::Class(c) = model.get_element(current) else {
            continue;
        };
        if let Some(p) = c.properties.iter().find(|p| p.name == *property) {
            return Some(p.multiplicity.clone());
        }
        if let Some(q) = c.qualified_properties.iter().find(|q| q.name == *property) {
            return Some(q.return_multiplicity.clone());
        }
        for st in &c.super_types {
            if let TypeExpr::Named { element, .. } = st {
                queue.push_back(*element);
            }
        }
    }
    None
}

/// Multiplicity product for a property/QP access chain
/// `receiver.prop`: combines the receiver's multiplicity with the
/// property's declared multiplicity.
///
/// Rules (matching Java Pure's `propertyExpression` post-processor):
///   - Either side `[*]` → `[*]`.
///   - Either side `[0..*]` (zero-or-many) → `[*]`.
///   - Either side has lower bound 0 → result lower bound 0.
///   - Bounded × Bounded → product of bounds.
///   - Variable / Generic on either side → fall back to the *receiver*
///     multiplicity (best partial answer; better than `None` which
///     dispatch treats as permissively compatible with everything).
pub(crate) fn multiplicity_product(
    receiver_mult: &crate::types::Multiplicity,
    property_mult: &crate::types::Multiplicity,
) -> crate::types::Multiplicity {
    use crate::types::Multiplicity;
    if matches!(receiver_mult, Multiplicity::Variable(_))
        || matches!(property_mult, Multiplicity::Variable(_))
    {
        return receiver_mult.clone();
    }
    let (r_lo, r_hi) = mult_bounds(receiver_mult);
    let (p_lo, p_hi) = mult_bounds(property_mult);

    let lo = r_lo.saturating_mul(p_lo);
    let hi = r_hi.saturating_mul(p_hi);
    let hi_opt = if hi == u32::MAX { None } else { Some(hi) };

    match (lo, hi_opt) {
        (1, Some(1)) => Multiplicity::PureOne,
        (0, Some(1)) => Multiplicity::ZeroOrOne,
        (1, None) => Multiplicity::OneOrMany,
        (0, None) => Multiplicity::ZeroOrMany,
        (lower, upper) => Multiplicity::Range { lower, upper },
    }
}

/// Extracts the type arguments from the receiver expression's stored type.
/// Only handles `Variable` — the most common receiver. Other receivers
/// (chained calls, property chains) return an empty vec; callers treat
/// unknown type args the same as no type args (substitution is a no-op).
fn extract_receiver_type_args(
    target: &crate::types::ValueSpec,
    var_types: &VarTypes,
) -> Vec<crate::types::TypeExpr> {
    use crate::types::{ExprKind, TypeExpr};
    match target.kind.as_ref() {
        ExprKind::Variable { name } => {
            if let Some((TypeExpr::Named { type_arguments, .. }, _)) = var_types.get(name) {
                return type_arguments.clone();
            }
            vec![]
        }
        _ => vec![],
    }
}

/// Rewrites a property `TypeExpr` by substituting class-level type parameters
/// with the receiver's concrete type arguments.
///
/// `type_params` is the class's declared parameter list (e.g., `["T", "U"]`).
/// `type_args` is the concrete arguments at the use site (e.g., `[String, Integer]`).
/// Position-based: `type_params[i]` maps to `type_args[i]`.
fn substitute_class_generics(
    ty: &crate::types::TypeExpr,
    type_params: &[smol_str::SmolStr],
    type_args: &[crate::types::TypeExpr],
) -> crate::types::TypeExpr {
    let bindings: HashMap<SmolStr, crate::types::TypeExpr> = type_params
        .iter()
        .zip(type_args.iter())
        .map(|(name, te)| (name.clone(), te.clone()))
        .collect();
    substitute_type(ty, &bindings)
}

/// Checks if `arg_type` is compatible with `param_type` in the type hierarchy.
///
/// Compatible means: same type, or `arg_type` is a subtype of `param_type`.
/// Returns true if we can't determine (either side is `None`/`Any`/generic).
pub(crate) fn is_type_compatible(
    arg_type: Option<ElementId>,
    param_type: &crate::types::TypeExpr,
    model: &crate::model::PureModel,
) -> bool {
    use crate::bootstrap;

    let param_eid = match param_type {
        crate::types::TypeExpr::Named { element, .. } => *element,
        // Generic matches anything, FunctionType can't be checked structurally
        _ => return true,
    };

    // If param is Any, everything matches
    if param_eid == bootstrap::ANY_ID {
        return true;
    }

    let Some(arg_eid) = arg_type else {
        // Unknown arg type — assume compatible (can't eliminate)
        return true;
    };

    // Exact match
    if arg_eid == param_eid {
        return true;
    }

    // Walk supertype chain: is arg_eid a subtype of param_eid?
    is_subtype(arg_eid, param_eid, model)
}

/// Extract the structural relation columns from a `TypeExpr`, if any.
///
/// Recognises both shapes the resolver produces:
/// - `Relation(cols)` directly.
/// - `Named { RelationType_id, type_arguments: [Relation(cols)], … }`
///   (the canonical wrapper form `RelationType<Relation(cols)>`).
/// - `Named { _, type_arguments: [Named { RelationType_id, …, [Relation(cols)] }], … }`
///   (e.g. `TDS<RelationType<Relation(cols)>>`).
///
/// Returns `None` when no relation columns are reachable in this
/// type's outer-or-first-type-argument layers.
fn extract_relation_columns(
    te: &crate::types::TypeExpr,
) -> Option<&[crate::types::RelationColumnTypeExpr]> {
    use crate::types::TypeExpr;
    match te {
        TypeExpr::Relation(cols) => Some(cols.as_slice()),
        TypeExpr::Named { type_arguments, .. } => {
            type_arguments.first().and_then(extract_relation_columns)
        }
        _ => None,
    }
}

/// Checks if `arg_type_expr`'s relation columns are compatible with
/// `param_type`'s relation columns.
///
/// Used as a refinement on top of [`is_type_compatible`]'s outer-element
/// check: when the param expects a structural relation type with
/// specific columns and the arg carries different columns, narrow the
/// candidate out.
///
/// Compatible means:
/// - Param has no specific columns (empty Relation list, or no Relation
///   layer at all) — accept anything.
/// - Arg has no extractable columns — accept (can't eliminate).
/// - Both have columns — every param column must appear in arg with a
///   compatible type and a satisfiable multiplicity. Extra arg columns
///   are allowed (subset semantics — `Relation<X⊆T>`).
fn is_relation_columns_compatible(
    arg_te: &crate::types::TypeExpr,
    param: &crate::types::TypeExpr,
    model: &crate::model::PureModel,
) -> bool {
    let Some(param_cols) = extract_relation_columns(param) else {
        return true; // Param has no specific column requirement
    };
    if param_cols.is_empty() {
        return true; // RelationType<Any> — accepts anything
    }
    let Some(arg_cols) = extract_relation_columns(arg_te) else {
        return true; // Arg's columns unknown — can't eliminate
    };
    for pc in param_cols {
        let Some(ac) = arg_cols.iter().find(|c| c.name == pc.name) else {
            return false; // param expected a column the arg doesn't carry
        };
        // Type compatibility: walk to outer ElementIds for each side.
        let outer_eid = |t: &crate::types::TypeExpr| match t {
            crate::types::TypeExpr::Named { element, .. } => Some(*element),
            _ => None,
        };
        if !is_type_compatible(outer_eid(&ac.type_expr), &pc.type_expr, model) {
            return false;
        }
        if !is_multiplicity_compatible(Some(&ac.multiplicity), &pc.multiplicity) {
            return false;
        }
    }
    true
}

/// Checks if `child` is a subtype of `parent` by walking the supertype chain.
///
/// `pub` so external compiler extensions (Mapping DSL, future DSLs)
/// can implement type-compatibility checks without re-implementing
/// supertype traversal. Returns `true` when `child == parent` or when
/// `parent` appears anywhere in `child`'s transitive supertypes
/// (including `PrimitiveType::super_type`).
pub fn is_subtype(child: ElementId, parent: ElementId, model: &crate::model::PureModel) -> bool {
    if child == parent {
        return true;
    }

    // `Nil` is the bottom type — a subtype of every type by language
    // definition. Encoding this here lets `least_upper_bound(Nil, X)`
    // shortcut to `X` instead of falling through to `Any`, which is
    // what `[]`-as-accumulator chains (`fold(_, λ, [])`) need so V
    // binds from the lambda body's actual return rather than being
    // pinned to the empty-collection's `Nil` and LUB'd to `Any`.
    if child == crate::bootstrap::NIL_ID {
        return true;
    }

    let element = model.get_element(child);
    let super_types = match element {
        Element::Class(c) => &c.super_types,
        Element::PrimitiveType(p) => {
            // PrimitiveType has a single super_type
            if let Some(sup) = p.super_type {
                return sup == parent || is_subtype(sup, parent, model);
            }
            return false;
        }
        _ => return false,
    };

    for st in super_types {
        if let crate::types::TypeExpr::Named {
            element: sup_eid, ..
        } = st
            && (*sup_eid == parent || is_subtype(*sup_eid, parent, model))
        {
            return true;
        }
    }
    false
}

/// Computes the shortest distance (number of hops) from `child` to `parent`
/// in the type hierarchy. Returns `None` if `child` is not a subtype of `parent`.
fn type_distance(
    child: ElementId,
    parent: ElementId,
    model: &crate::model::PureModel,
) -> Option<usize> {
    if child == parent {
        return Some(0);
    }

    let element = model.get_element(child);
    let super_types = match element {
        Element::Class(c) => &c.super_types,
        Element::PrimitiveType(p) => {
            if let Some(sup) = p.super_type {
                return type_distance(sup, parent, model).map(|d| d + 1);
            }
            return None;
        }
        _ => return None,
    };

    let mut min_dist: Option<usize> = None;
    for st in super_types {
        if let crate::types::TypeExpr::Named {
            element: sup_eid, ..
        } = st
            && let Some(d) = type_distance(*sup_eid, parent, model)
        {
            let dist = d + 1;
            min_dist = Some(min_dist.map_or(dist, |m: usize| m.min(dist)));
        }
    }
    min_dist
}

/// Computes the least upper bound (LUB) of two types in the hierarchy.
///
/// Collects `a`'s ancestor chain, then walks `b`'s ancestors until a
/// common type is found. Falls back to `Any` if no common ancestor exists.
///
/// Exposed for the runtime `type()` native (`crates/runtime/.../meta.rs`)
/// to compute LUB across collection elements when no compile-time type
/// information is available.
#[must_use]
pub fn least_upper_bound_ids(
    a: ElementId,
    b: ElementId,
    model: &crate::model::PureModel,
) -> ElementId {
    least_upper_bound(a, b, model)
}

fn least_upper_bound(a: ElementId, b: ElementId, model: &crate::model::PureModel) -> ElementId {
    if a == b {
        return a;
    }
    // If one is subtype of the other, the supertype is the LUB
    if is_subtype(a, b, model) {
        return b;
    }
    if is_subtype(b, a, model) {
        return a;
    }

    // Collect a's ancestor chain (including a itself)
    let mut a_ancestors = Vec::new();
    collect_ancestors(a, model, &mut a_ancestors);

    // Walk b's ancestors and find the first match in a's chain
    let mut current = b;
    loop {
        if a_ancestors.contains(&current) {
            return current;
        }
        match get_first_supertype(current, model) {
            Some(sup) => current = sup,
            None => break,
        }
    }

    // Fallback: Any (top type)
    crate::bootstrap::ANY_ID
}

/// Collects all ancestors of `id` into `out` (including `id` itself).
fn collect_ancestors(id: ElementId, model: &crate::model::PureModel, out: &mut Vec<ElementId>) {
    out.push(id);
    if let Some(sup) = get_first_supertype(id, model) {
        collect_ancestors(sup, model, out);
    }
}

/// Returns the first (primary) supertype of an element, if any.
fn get_first_supertype(id: ElementId, model: &crate::model::PureModel) -> Option<ElementId> {
    let element = model.get_element(id);
    match element {
        Element::PrimitiveType(p) => p.super_type,
        Element::Class(c) => c.super_types.first().and_then(|st| {
            if let crate::types::TypeExpr::Named { element, .. } = st {
                Some(*element)
            } else {
                None
            }
        }),
        _ => None,
    }
}

/// Infers multiplicity from a lowered `ValueSpec` by examining `ExprKind`.
/// Returns `None` for expressions whose multiplicity can't be determined.
pub(crate) fn infer_multiplicity_from_valuespec(
    vs: &crate::types::ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Option<crate::types::Multiplicity> {
    use crate::types::{ExprKind, Multiplicity};

    match vs.kind.as_ref() {
        // All literals produce exactly one value; enum values are [1]; lambda is [1];
        // bare element refs and type references are single values
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_)
        | ExprKind::EnumValue { .. }
        | ExprKind::Lambda { .. }
        | ExprKind::PackageableElementRef { .. }
        | ExprKind::TypeReference { .. } => Some(Multiplicity::PureOne),

        // Collection: precise cardinality from element count.
        // `[]` is `[0..0]`, `[x]` is `[1]`, `[x, y]` is `[2..2]`, etc.
        // This precision matters for dispatch — `[]` must match an `[0..1]`
        // parameter but not a `[1]` parameter.
        ExprKind::Collection { elements } => {
            #[allow(clippy::cast_possible_truncation)]
            let n = elements.len() as u32;
            Some(match n {
                0 => Multiplicity::Range {
                    lower: 0,
                    upper: Some(0),
                },
                1 => Multiplicity::PureOne,
                _ => Multiplicity::Range {
                    lower: n,
                    upper: Some(n),
                },
            })
        }

        // Function call → return multiplicity of the resolved function,
        // with generic multiplicity variables (`m`) bound from arguments.
        ExprKind::FunctionCall(FunctionCallData {
            function,
            arguments,
            ..
        }) => function.and_then(|fid| {
            if let Element::Function(f) = model.get_element(fid) {
                let bindings = infer_generic_bindings(&f.parameters, arguments, model, var_types);
                Some(substitute_mult(&f.return_multiplicity, &bindings.mult))
            } else {
                None
            }
        }),

        // Variable → look up declared multiplicity
        ExprKind::Variable { name } => var_types.get(name).map(|(_, m)| m.clone()),

        // Property / qualified-property access — combine receiver
        // multiplicity with the property's declared multiplicity.
        // Without this, dispatch sees `obj.collProp` as
        // multiplicity-unknown and the `is_multiplicity_compatible`
        // permissive-on-None branch lets `[1]`-typed param overloads
        // win against `[*]`-typed ones, causing
        // `String[*]->contains(String[1])` to dispatch to the
        // `string::contains(String[1], String[1])` substring overload
        // instead of `collection::contains(Any[*], Any[1])`.
        ExprKind::PropertyCall(data) | ExprKind::QualifiedPropertyCall(data) => {
            let target = data.arguments.first()?;
            let recv_mult = infer_multiplicity_from_valuespec(target, model, var_types)?;
            let target_eid = infer_type_from_valuespec(target, model, var_types)?;
            let prop_mult = find_property_multiplicity(target_eid, &data.function_name, model)?;
            Some(multiplicity_product(&recv_mult, &prop_mult))
        }

        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Generic substitution (type variables + multiplicity variables)
// ---------------------------------------------------------------------------

// `GenericBindings` lives in `crate::inference::context`. The
// re-export here keeps the existing `crate::resolve::GenericBindings`
// import path working while the substitution machinery migrates
// gradually to the new module — see plan
// `~/.claude/plans/do-we-have-enought-quiet-swing.md`.
pub(crate) use crate::inference::GenericBindings;

/// Walks `(params, args)` pairs and collects generic-variable bindings.
///
/// For each parameter whose type is `Generic(name)`, binds `name` to the
/// argument's inferred `TypeExpr`. For each parameter whose multiplicity is
/// `Variable(name)`, binds `name` to the argument's inferred `Multiplicity`.
/// Also recurses into `Named { type_arguments, … }` so `@T[1]` binds `T` from
/// the `TypeReference`'s wrapped type.
///
/// If the same variable is bound by multiple arguments (e.g., `T` appears in
/// two params), the bindings are merged using LUB rather than first-wins.
pub(crate) fn infer_generic_bindings(
    params: &[crate::types::Parameter],
    args: &[crate::types::ValueSpec],
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> GenericBindings {
    use crate::types::{ExprKind, Multiplicity, TypeExpr};
    let mut bindings = GenericBindings::default();
    for (param, arg) in params.iter().zip(args.iter()) {
        // Bind the param's multiplicity variable from the arg's multiplicity.
        if let Multiplicity::Variable(name) = &param.multiplicity
            && let Some(arg_mult) = infer_multiplicity_from_valuespec(arg, model, var_types)
        {
            use std::collections::hash_map::Entry;
            match bindings.mult.entry(name.clone()) {
                Entry::Vacant(e) => {
                    e.insert(arg_mult);
                }
                Entry::Occupied(mut e) => {
                    // m already bound — compute LUB (widest range covering both).
                    let lub = mult_lub(e.get(), &arg_mult);
                    *e.get_mut() = lub;
                }
            }
        }
        // Derive the arg's "type expression" — preserve `type_arguments`
        // through the inference so generic substitution sees the FULL
        // parametric shape. The previous version dropped `type_arguments`
        // unconditionally, which made `class<T>(T[*]):Class<T>[1]` bind
        // T to the bare element rather than the parametric TypeExpr.
        // Now `$l1: List<String>` flows through as `Named{List, [String]}`
        // and the substituted return type comes out `Class<List<String>>`.
        let arg_type_expr: Option<TypeExpr> = infer_typeexpr_from_valuespec(arg, model, var_types);
        if let Some(arg_ty) = arg_type_expr {
            bind_type(&param.type_expr, &arg_ty, &mut bindings.ty, model);
        }
    }

    // Second pass: for lambda args against Function<{T->V}>[1] params, infer
    // the lambda body's return type (with the lambda params in scope) and bind
    // the FunctionType's return type variable.  This lets `map(coll, r | $r.x)`
    // propagate the property type through `V` so downstream calls like `->plus()`
    // can resolve unambiguously.
    //
    // Also handles `arg` shapes where the lambda is wrapped in a
    // `Collection` — the `match<T|m,n>(var, [λ1, λ2, …])` form. The
    // FunctionType slot's multiplicity allows multi-element collections,
    // so we walk every Lambda inside the Collection and bind from each
    // body in turn (LUB-merging through `bind_type`).
    for (param, arg) in params.iter().zip(args.iter()) {
        // The param type may be FunctionType directly, or Named<Function>[FunctionType]
        // (the common `Function<{T[1]->V[*]}>` spelling).
        let function_type = match &param.type_expr {
            TypeExpr::FunctionType { .. } => Some(&param.type_expr),
            TypeExpr::Named { type_arguments, .. } => type_arguments
                .iter()
                .find(|ta| matches!(ta, TypeExpr::FunctionType { .. })),
            _ => None,
        };
        let Some(function_type) = function_type else {
            continue;
        };
        // Collect every lambda contributing to this parameter slot — a
        // direct `Lambda` arg or every `Lambda` inside a `Collection` arg.
        let lambda_args: Vec<&crate::types::ValueSpec> = match arg.kind.as_ref() {
            ExprKind::Lambda { .. } => vec![arg],
            ExprKind::Collection { elements } => elements
                .iter()
                .filter(|e| matches!(e.kind.as_ref(), ExprKind::Lambda { .. }))
                .collect(),
            _ => continue,
        };
        for lambda_arg in lambda_args {
            bind_from_lambda_body(
                function_type,
                lambda_arg,
                model,
                var_types,
                &mut bindings,
            );
        }
    }

    bindings
}

/// Drill into a single lambda arg against an expected `FunctionType`
/// shape, infer the lambda body's last expression's type with the
/// lambda's params in scope, and `bind_type`-merge the result against
/// the FunctionType's `return_type` variable.
///
/// Extracted so both the direct-Lambda and Collection-of-Lambdas
/// branches in `infer_generic_bindings`'s second pass can share the
/// per-lambda binding logic.
fn bind_from_lambda_body(
    function_type: &crate::types::TypeExpr,
    arg: &crate::types::ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
    bindings: &mut GenericBindings,
) {
    use crate::types::{ExprKind, Multiplicity, TypeExpr};
    let TypeExpr::FunctionType {
        parameters: ft_params,
        return_type,
        ..
    } = function_type
    else {
        return;
    };
    let ExprKind::Lambda {
        parameters: lambda_params,
        body: lambda_body,
    } = arg.kind.as_ref()
    else {
        return;
    };
    // Extend var_types with the lambda's own params, preferring the
    // lambda's declared type when concrete (mirrors Java's "explicit
    // annotation wins" rule). Without this, an explicitly-typed param
    // like `{inc:MappingInclude[1], sub:Store[0..1] | …}` against
    // `Function<{T[1], V[m] -> V[m]}>[1]` would have `sub` bound to
    // `Generic("V")` whenever V hadn't bound from a sibling arg yet —
    // and the lambda body would then type its uses against `Generic`
    // rather than `Store[0..1]`.
    let mut extended = var_types.clone();
    for (lp, (ft_ty, ft_mult)) in lambda_params.iter().zip(ft_params.iter()) {
        let lambda_ty_concrete =
            !matches!(lp.type_expr, TypeExpr::Generic(_) | TypeExpr::Unresolved);
        let ty = if lambda_ty_concrete {
            lp.type_expr.clone()
        } else {
            substitute_type(ft_ty, &bindings.ty)
        };
        let mult = if !matches!(lp.multiplicity, Multiplicity::Variable(_)) {
            lp.multiplicity.clone()
        } else {
            substitute_mult(ft_mult, &bindings.mult)
        };
        extended.insert(lp.name.clone(), (ty, mult));
    }
    let Some(last_expr) = lambda_body.last() else {
        return;
    };
    let Some(body_eid) = infer_type_from_valuespec(last_expr, model, &extended) else {
        return;
    };
    let body_te = TypeExpr::Named {
        element: body_eid,
        type_arguments: vec![],
        multiplicity_arguments: Vec::new(),
        value_arguments: vec![],
    };
    bind_type(return_type, &body_te, &mut bindings.ty, model);
}

/// Recursively match `param_ty` against `arg_ty`, collecting type-variable
/// bindings. Handles:
/// - `Generic(T)` vs anything → bind `T := arg_ty`.
/// - `Named { type_arguments: [...] }` vs `Named { type_arguments: [...] }`
///   → recurse pairwise so `List<T>` against `List<String>` binds
///   `T := String`.
///
/// If `T` is already bound and a second arg binds `T` to a different type,
/// the binding is updated to the LUB of the two types rather than first-wins.
pub(crate) fn bind_type(
    param_ty: &crate::types::TypeExpr,
    arg_ty: &crate::types::TypeExpr,
    out: &mut HashMap<SmolStr, crate::types::TypeExpr>,
    model: &crate::model::PureModel,
) {
    use crate::types::TypeExpr;
    if matches!(arg_ty, TypeExpr::Unresolved) {
        return;
    }
    match param_ty {
        TypeExpr::Generic(name) => {
            use std::collections::hash_map::Entry;
            match out.entry(name.clone()) {
                Entry::Vacant(e) => {
                    e.insert(arg_ty.clone());
                }
                Entry::Occupied(mut e) => {
                    let lub = type_lub(e.get(), arg_ty, model);
                    *e.get_mut() = lub;
                }
            }
        }
        TypeExpr::Named {
            type_arguments: p_args,
            ..
        } => {
            if let TypeExpr::Named {
                type_arguments: a_args,
                ..
            } = arg_ty
            {
                for (p, a) in p_args.iter().zip(a_args.iter()) {
                    bind_type(p, a, out, model);
                }
            }
        }
        TypeExpr::FunctionType {
            parameters: p_params,
            return_type: p_ret,
            ..
        } => {
            if let TypeExpr::FunctionType {
                parameters: a_params,
                return_type: a_ret,
                ..
            } = arg_ty
            {
                for ((p_ty, _), (a_ty, _)) in p_params.iter().zip(a_params.iter()) {
                    bind_type(p_ty, a_ty, out, model);
                }
                bind_type(p_ret, a_ret, out, model);
            }
        }
        _ => {}
    }
}

/// Computes the least upper bound of two `TypeExpr`s.
/// For `Named` types, walks the type hierarchy via `least_upper_bound`.
/// For anything else (or mixed), falls back to `Any`.
fn type_lub(
    a: &crate::types::TypeExpr,
    b: &crate::types::TypeExpr,
    model: &crate::model::PureModel,
) -> crate::types::TypeExpr {
    use crate::bootstrap;
    use crate::types::TypeExpr;
    if a == b {
        return a.clone();
    }
    match (a, b) {
        (TypeExpr::Named { element: ea, .. }, TypeExpr::Named { element: eb, .. }) => {
            TypeExpr::Named {
                element: least_upper_bound(*ea, *eb, model),
                type_arguments: vec![],
                multiplicity_arguments: Vec::new(),
                value_arguments: vec![],
            }
        }
        // Can't compute structural LUB → Any
        _ => TypeExpr::Named {
            element: bootstrap::ANY_ID,
            type_arguments: vec![],
            multiplicity_arguments: Vec::new(),
            value_arguments: vec![],
        },
    }
}

/// Computes the least upper bound of two multiplicities — the smallest range
/// that covers both. Used when the same multiplicity variable `m` is bound
/// from multiple arguments.
pub(crate) fn mult_lub(
    a: &crate::types::Multiplicity,
    b: &crate::types::Multiplicity,
) -> crate::types::Multiplicity {
    use crate::types::Multiplicity;
    let (a_lo, a_hi) = mult_bounds(a);
    let (b_lo, b_hi) = mult_bounds(b);
    let lower = a_lo.min(b_lo);
    let upper = a_hi.max(b_hi);
    match (lower, upper) {
        (1, 1) => Multiplicity::PureOne,
        (0, 1) => Multiplicity::ZeroOrOne,
        (1, u32::MAX) => Multiplicity::OneOrMany,
        _ => Multiplicity::Range {
            lower,
            upper: if upper == u32::MAX { None } else { Some(upper) },
        },
    }
}

/// Rewrites a `TypeExpr` by substituting `Generic(name)` nodes with their
/// bound types. Recurses into `Named { type_arguments }`, `FunctionType`,
/// and `AlgebraUnion` so substitution works at any nesting depth.
pub(crate) fn substitute_type(
    ty: &crate::types::TypeExpr,
    bindings: &HashMap<SmolStr, crate::types::TypeExpr>,
) -> crate::types::TypeExpr {
    use crate::types::TypeExpr;
    match ty {
        TypeExpr::Generic(name) => bindings.get(name).cloned().unwrap_or_else(|| ty.clone()),
        TypeExpr::Named {
            element,
            type_arguments,
            multiplicity_arguments,
            value_arguments,
        } => TypeExpr::Named {
            element: *element,
            type_arguments: type_arguments
                .iter()
                .map(|t| substitute_type(t, bindings))
                .collect(),
            multiplicity_arguments: multiplicity_arguments.clone(),
            value_arguments: value_arguments.clone(),
        },
        TypeExpr::FunctionType {
            parameters,
            return_type,
            return_multiplicity,
        } => TypeExpr::FunctionType {
            parameters: parameters
                .iter()
                .map(|(t, m)| (substitute_type(t, bindings), m.clone()))
                .collect(),
            return_type: Box::new(substitute_type(return_type, bindings)),
            return_multiplicity: return_multiplicity.clone(),
        },
        TypeExpr::AlgebraUnion(a, b) => TypeExpr::AlgebraUnion(
            Box::new(substitute_type(a, bindings)),
            Box::new(substitute_type(b, bindings)),
        ),
        // Recurse into structural relation columns: each column's
        // `type_expr` may reference an outer-scope generic
        // (e.g. `wrapPrimitiveInTDS<T>(…):TDS<(value:T[0..1])>[1]`
        // resolves to `Named { TDS, [Named { RelationType,
        // [Relation([{name:"value", type_expr:Generic("T"), …}])] }] }`
        // — substituting `T=String` at the call site needs to flow
        // into the column's type_expr or `$x.value` later types as
        // `Generic("T")` instead of `String`).
        TypeExpr::Relation(cols) => TypeExpr::Relation(
            cols.iter()
                .map(|c| crate::types::RelationColumnTypeExpr {
                    name: c.name.clone(),
                    type_expr: substitute_type(&c.type_expr, bindings),
                    multiplicity: c.multiplicity.clone(),
                })
                .collect(),
        ),
        // `Unresolved` (type hole) passes through unchanged: substitution
        // has no bindings that could fill it — only re-lowering with
        // caller-side expectations could specialise an `Unresolved`.
        TypeExpr::Unresolved => ty.clone(),
    }
}

/// Substitutes a multiplicity variable with its bound value, if any.
/// Non-variable multiplicities are returned unchanged.
pub(crate) fn substitute_mult(
    m: &crate::types::Multiplicity,
    bindings: &HashMap<SmolStr, crate::types::Multiplicity>,
) -> crate::types::Multiplicity {
    use crate::types::Multiplicity;
    match m {
        Multiplicity::Variable(name) => bindings.get(name).cloned().unwrap_or_else(|| m.clone()),
        _ => m.clone(),
    }
}

/// Checks if `arg_mult` is compatible with `param_mult`.
///
/// Compatible means: the arg's multiplicity fits within the param's range.
/// `[1]` fits into `[0..1]`, `[0..1]`, `[1..*]`, `[*]`.
/// `[*]` only fits into `[*]`.
/// `pub` for the same reason as [`is_subtype`] — DSL extensions need
/// to enforce "transform multiplicity must subsume property
/// multiplicity"-style rules without duplicating range arithmetic.
pub fn is_multiplicity_compatible(
    arg_mult: Option<&crate::types::Multiplicity>,
    param_mult: &crate::types::Multiplicity,
) -> bool {
    let Some(am) = arg_mult else {
        // Unknown — assume compatible
        return true;
    };

    // Unbound multiplicity variable on the arg (e.g. PCT tests typed
    // `<T|m>` flow `m` through the result of `f->eval(...)`). The
    // narrower can't decide subset-of without binding `m`, so treat
    // it as permissive — same logic the type-side uses for
    // `TypeExpr::Generic` / `Unresolved`. Otherwise Variable's
    // `(0, MAX)` bound spuriously fails subset checks against
    // `PureOne` / `ZeroOrOne` params, eliminating every candidate
    // and triggering the empty-fallback that returns the original
    // candidate set unchanged → spurious `Ambiguous function call`.
    if matches!(am, crate::types::Multiplicity::Variable(_)) {
        return true;
    }

    // Symmetric — same rule for an unbound param multiplicity (e.g.
    // a generic signature with `T[m]`).
    if matches!(param_mult, crate::types::Multiplicity::Variable(_)) {
        return true;
    }

    // Check if arg's range is a subset of param's range
    let (arg_lo, arg_hi) = mult_bounds(am);
    let (param_lo, param_hi) = mult_bounds(param_mult);

    // arg's lower >= param's lower AND arg's upper <= param's upper
    arg_lo >= param_lo && arg_hi <= param_hi
}

/// Returns (lower, upper) bounds for a multiplicity.
/// `None` upper means unbounded (represented as `u32::MAX`).
fn mult_bounds(m: &crate::types::Multiplicity) -> (u32, u32) {
    use crate::types::Multiplicity;
    match m {
        Multiplicity::PureOne => (1, 1),
        Multiplicity::ZeroOrOne => (0, 1),
        Multiplicity::OneOrMany => (1, u32::MAX),
        Multiplicity::Range { lower, upper } => (*lower, upper.unwrap_or(u32::MAX)),
        // ZeroOrMany and Variable: both unbounded lower bound
        Multiplicity::ZeroOrMany | Multiplicity::Variable(_) => (0, u32::MAX),
    }
}

/// Multiplicity specificity score — more specific = higher.
/// `[1]` = 4, `[0..1]` = 3, `[1..*]` = 2, `[*]` = 1
fn mult_specificity(m: &crate::types::Multiplicity) -> i32 {
    use crate::types::Multiplicity;
    match m {
        Multiplicity::PureOne => 4,
        Multiplicity::OneOrMany => 2,
        Multiplicity::ZeroOrOne | Multiplicity::Range { upper: Some(_), .. } => 3, // bounded cases
        Multiplicity::Range { upper: None, .. }
        | Multiplicity::ZeroOrMany
        | Multiplicity::Variable(_) => 1, // unbounded cases
    }
}

/// Extracts the return multiplicity from a `Function<{...->V[m]}>` param type.
///
/// Returns `Some(m)` if the param type is `Named { type_arguments: [FunctionType { return_multiplicity: m, .. }] }`
/// or a direct `FunctionType`, otherwise `None`.
fn extract_function_type_return_mult(
    param_type: &crate::types::TypeExpr,
) -> Option<&crate::types::Multiplicity> {
    match param_type {
        crate::types::TypeExpr::Named { type_arguments, .. } => {
            type_arguments.iter().find_map(|ta| match ta {
                crate::types::TypeExpr::FunctionType {
                    return_multiplicity,
                    ..
                } => Some(return_multiplicity),
                _ => None,
            })
        }
        crate::types::TypeExpr::FunctionType {
            return_multiplicity,
            ..
        } => Some(return_multiplicity),
        _ => None,
    }
}

/// Checks if a lambda argument is compatible with a `Function<{...->V[m]}>` param.
///
/// When the arg is a `Lambda` and the param type contains a `FunctionType`
/// (via `Function<{T[1]->V[m]}>` or a direct `{...}` type), this checks
/// whether the lambda's body return multiplicity fits the expected `m`.
///
/// Returns `true` (compatible) if:
/// - The arg is not a lambda (not applicable)
/// - The param type doesn't contain a `FunctionType` (not applicable)
/// - The lambda's body return multiplicity can't be determined (assume OK)
/// - The lambda's body return multiplicity fits within the param's expected mult
fn is_lambda_compatible(
    arg_vs: &crate::types::ValueSpec,
    param_type: &crate::types::TypeExpr,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> bool {
    use crate::types::ExprKind;

    // Only applies when the arg is a Lambda
    let ExprKind::Lambda { body, .. } = arg_vs.kind.as_ref() else {
        return true;
    };

    // Extract the expected FunctionType from the param.
    // For `Function<{T[1]->V[m]}>`, the FunctionType is in type_arguments[0].
    let expected_ft = match param_type {
        crate::types::TypeExpr::Named {
            element,
            type_arguments,
            ..
        } => {
            let ft = type_arguments.iter().find_map(|ta| match ta {
                crate::types::TypeExpr::FunctionType {
                    return_multiplicity,
                    ..
                } => Some(return_multiplicity),
                _ => None,
            });
            if ft.is_none() && *element != crate::bootstrap::ANY_ID {
                // The arg is a Lambda but the param is a concrete Named type
                // without a FunctionType (e.g., String, Boolean) — incompatible.
                // Any is allowed because Function is a subtype of Any.
                return false;
            }
            ft
        }
        crate::types::TypeExpr::FunctionType {
            return_multiplicity,
            ..
        } => Some(return_multiplicity),
        // Generic params (T) — assume compatible, can't verify
        _ => None,
    };

    let Some(expected_mult) = expected_ft else {
        return true; // Any or Generic param — assume compatible
    };

    // Infer the lambda's body return multiplicity from the last expression
    let Some(last_expr) = body.last() else {
        return true; // Empty body — can't determine
    };

    let inferred = infer_multiplicity_from_valuespec(last_expr, model, var_types);
    is_multiplicity_compatible(inferred.as_ref(), expected_mult)
}

/// Narrows function candidates using a two-phase approach:
///
/// **Phase 1 — Filter**: Eliminate candidates whose parameter types or
/// multiplicities are incompatible with the inferred argument types.
/// A candidate is compatible if, for every parameter position:
///   - The arg type is unknown (treated as Any — matches everything), OR
///   - The arg type exactly matches or is a subtype of the param type
///   - The arg multiplicity is unknown (matches everything), OR
///   - The arg multiplicity fits within the param's multiplicity range
///
/// **Phase 2 — Rank**: Among compatible candidates, score each by
/// specificity to pick the best fit:
///   - Exact type match: +3 per param
///   - Subtype match: +1 per param
///   - Generic/Any param: +0 (matches but non-specific)
///   - Exact multiplicity match: +4 per param
///   - Compatible multiplicity: +specificity bonus (narrower = higher)
///
/// If all candidates are eliminated by filtering, returns the original set
/// (lets the ambiguity error surface with all candidates listed).
#[tracing::instrument(
    name = "narrow_candidates_by_type",
    level = "debug",
    skip(candidates, lowered_args, model, var_types),
    fields(n_candidates = candidates.len(), n_args = lowered_args.len()),
)]
#[allow(clippy::too_many_lines)]
pub(crate) fn narrow_candidates_by_type(
    candidates: &[ElementId],
    lowered_args: &[crate::types::ValueSpec],
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Vec<ElementId> {
    if candidates.len() <= 1 {
        return candidates.to_vec();
    }

    // Infer types and multiplicities of each argument once
    let arg_types: Vec<Option<ElementId>> = lowered_args
        .iter()
        .map(|vs| infer_type_from_valuespec(vs, model, var_types))
        .collect();

    let arg_mults: Vec<Option<crate::types::Multiplicity>> = lowered_args
        .iter()
        .map(|vs| infer_multiplicity_from_valuespec(vs, model, var_types))
        .collect();

    tracing::debug!(
        arg_types = ?arg_types
            .iter()
            .map(|t| t.map(|e| model.element_name(e).to_string()))
            .collect::<Vec<_>>(),
        arg_mults = ?arg_mults,
        "inferred operand types/multiplicities"
    );

    // === Phase 1: Filter — keep only compatible candidates ===
    let compatible: Vec<ElementId> = candidates
        .iter()
        .copied()
        .filter(|&eid| {
            let Element::Function(f) = model.get_element(eid) else {
                return false;
            };
            for (i, param) in f.parameters.iter().enumerate() {
                let arg_type = arg_types.get(i).copied().flatten();
                if !is_type_compatible(arg_type, &param.type_expr, model) {
                    return false;
                }
                let arg_mult = arg_mults.get(i).cloned().flatten();
                if !is_multiplicity_compatible(arg_mult.as_ref(), &param.multiplicity) {
                    return false;
                }
                // Lambda vs Function<{...->V[m]}> structural check:
                // when the arg is a lambda and the param expects a Function type,
                // verify the lambda's body return multiplicity matches.
                if let Some(arg_vs) = lowered_args.get(i)
                    && !is_lambda_compatible(arg_vs, &param.type_expr, model, var_types)
                {
                    return false;
                }
                // Structural-relation column check: when the param
                // expects a relation type with specific columns and the
                // arg's TypeExpr carries differing columns, eliminate.
                // No-op when either side lacks a Relation inner.
                if let Some(arg_te) = lowered_args
                    .get(i)
                    .and_then(|vs| vs.type_info.as_ref().map(|rt| &rt.type_expr))
                    && !is_relation_columns_compatible(arg_te, &param.type_expr, model)
                {
                    return false;
                }
            }
            true
        })
        .collect();

    if compatible.is_empty() {
        // Filtering eliminated everything — fall back to original set
        return candidates.to_vec();
    }
    if compatible.len() == 1 {
        return compatible;
    }

    // === Phase 2: Rank — score compatible candidates by specificity ===
    let mut scored: Vec<(ElementId, i32)> = compatible
        .iter()
        .map(|&eid| {
            let Element::Function(f) = model.get_element(eid) else {
                return (eid, 0);
            };
            let mut score: i32 = 0;
            for (i, param) in f.parameters.iter().enumerate() {
                // --- Type specificity ---
                if let Some(at) = arg_types.get(i).copied().flatten()
                    && let crate::types::TypeExpr::Named { element: pe, .. } = &param.type_expr
                {
                    if at == *pe {
                        score += 3; // exact type match
                    } else if is_subtype(at, *pe, model) {
                        score += 1; // subtype match
                    }
                    // else: compatible via Any/generic — +0
                }
                // If arg type is unknown, no type score — all candidates equal

                // --- Multiplicity specificity ---
                if let Some(ref am) = arg_mults.get(i).cloned().flatten() {
                    if *am == param.multiplicity {
                        score += 4; // exact multiplicity match
                    } else {
                        score += mult_specificity(&param.multiplicity);
                    }
                } else {
                    // Unknown arg mult — use param specificity as tiebreaker
                    score += mult_specificity(&param.multiplicity);
                }

                // --- Lambda FunctionType return multiplicity specificity ---
                // When the arg is a lambda and the param expects Function<{...->V[m]}>,
                // prefer the overload with the most specific return multiplicity.
                if let Some(arg_vs) = lowered_args.get(i)
                    && matches!(arg_vs.kind.as_ref(), crate::types::ExprKind::Lambda { .. })
                    && let Some(ft_mult) = extract_function_type_return_mult(&param.type_expr)
                {
                    score += mult_specificity(ft_mult);
                }

                // --- Concrete vs Function-wrapped type preference ---
                // When the arg is NOT a lambda and parameter type could be either
                // concrete (String) or Function-wrapped (Function<{->String}>),
                // prefer the concrete type.
                // When the arg IS a lambda, prefer the Function-wrapped param.
                let arg_is_lambda = lowered_args.get(i).is_some_and(|vs| {
                    matches!(vs.kind.as_ref(), crate::types::ExprKind::Lambda { .. })
                });
                let param_is_function_type =
                    extract_function_type_return_mult(&param.type_expr).is_some();

                if arg_is_lambda && param_is_function_type {
                    score += 3; // lambda arg matches Function param → strong preference
                } else if !arg_is_lambda && !param_is_function_type {
                    score += 2; // non-lambda arg matches concrete param → preference
                }
            }
            (eid, score)
        })
        .collect();

    // Sort by score descending, pick the best
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    let best_score = scored[0].1;

    // Collect all candidates tied at the best score
    let tied: Vec<ElementId> = scored
        .into_iter()
        .filter(|(_, s)| *s == best_score)
        .map(|(eid, _)| eid)
        .collect();

    if tied.len() <= 1 {
        return tied;
    }

    // === Phase 3: Most-specific parameter type tiebreaker ===
    // Among tied candidates, eliminate any overload whose parameter types
    // are all *supertypes* (or equal) of another overload's parameters.
    // E.g., elementToPath(PackageableElement) is dominated by elementToPath(Type)
    // because Type <: PackageableElement, so the Type overload is more specific.
    let mut dominated: Vec<bool> = vec![false; tied.len()];

    for i in 0..tied.len() {
        if dominated[i] {
            continue;
        }
        let Element::Function(fi) = model.get_element(tied[i]) else {
            continue;
        };
        for j in 0..tied.len() {
            if i == j || dominated[j] {
                continue;
            }
            let Element::Function(fj) = model.get_element(tied[j]) else {
                continue;
            };
            // Check if fj's params are ALL supertypes of (or same as) fi's params
            // → fi is more specific, so fj is dominated
            let fj_dominated = fi
                .parameters
                .iter()
                .zip(fj.parameters.iter())
                .all(|(pi, pj)| {
                    let i_eid = match &pi.type_expr {
                        crate::types::TypeExpr::Named { element, .. } => Some(*element),
                        _ => None,
                    };
                    let j_eid = match &pj.type_expr {
                        crate::types::TypeExpr::Named { element, .. } => Some(*element),
                        _ => None,
                    };
                    match (i_eid, j_eid) {
                        (Some(i_e), Some(j_e)) => i_e == j_e || is_subtype(i_e, j_e, model),
                        _ => true, // can't compare generics — don't eliminate
                    }
                });
            if fj_dominated {
                dominated[j] = true;
            }
        }
    }

    let result: Vec<ElementId> = tied
        .into_iter()
        .enumerate()
        .filter(|(idx, _)| !dominated[*idx])
        .map(|(_, eid)| eid)
        .collect();

    if result.is_empty() {
        // Shouldn't happen, but don't lose candidates
        candidates.to_vec()
    } else if result.len() > 1 {
        // Phase 4: Argument-type proximity tiebreaker.
        // When we know the arg types, find the overload whose parameter
        // types are the closest ancestors. This breaks ties between
        // independent branches like Type vs PackageableElement.
        let mut best_distance = usize::MAX;
        let mut best_idx = 0;
        let mut unique_best = true;

        for (idx, &eid) in result.iter().enumerate() {
            let Element::Function(f) = model.get_element(eid) else {
                continue;
            };
            let mut total_distance = 0usize;
            let mut can_score = true;
            for (pi, arg_type_opt) in f.parameters.iter().zip(arg_types.iter()) {
                let crate::types::TypeExpr::Named { element, .. } = &pi.type_expr else {
                    can_score = false;
                    break;
                };
                let param_eid = *element;
                let Some(arg_eid) = arg_type_opt else {
                    can_score = false;
                    break;
                };
                total_distance += type_distance(*arg_eid, param_eid, model).unwrap_or(100);
            }
            if can_score {
                if total_distance < best_distance {
                    best_distance = total_distance;
                    best_idx = idx;
                    unique_best = true;
                } else if total_distance == best_distance {
                    unique_best = false;
                }
            }
        }

        if unique_best && best_distance < usize::MAX {
            vec![result[best_idx]]
        } else if any_arg_reads_unresolved(lowered_args, var_types) {
            // Type-hole guard. When the only reason we got here is that
            // an argument reads a `TypeExpr::Unresolved` lambda
            // parameter, the declaration-order tiebreaker would silently
            // commit to whichever overload happened to be declared
            // first — masking the real cause (the user didn't annotate
            // the lambda). Surface the ambiguity instead so
            // `resolve_function_call` upgrades it to a
            // `CannotInferLambdaParameterTypes` diagnostic naming the
            // offending parameters.
            result
        } else {
            // Phase 5: Declaration-order tiebreaker.
            // When all other disambiguation fails, pick the first-declared
            // overload. The candidate ordering is preserved from registration
            // order, which reflects source declaration order.
            vec![result[0]]
        }
    } else {
        result
    }
}

/// True if any of `lowered_args` (recursively) reads a variable whose
/// stored type is `TypeExpr::Unresolved`. Used to short-circuit the
/// declaration-order tiebreaker so type-hole arguments surface as
/// ambiguity instead of silently committing to the first overload.
fn any_arg_reads_unresolved(args: &[crate::types::ValueSpec], var_types: &VarTypes) -> bool {
    let mut tmp_out: Vec<SmolStr> = Vec::new();
    let mut tmp_seen: std::collections::HashSet<SmolStr> = std::collections::HashSet::new();
    for arg in args {
        collect_unresolved_param_reads(arg, var_types, &mut tmp_out, &mut tmp_seen);
        if !tmp_out.is_empty() {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// `multiplicity_product` is the heart of property-chain
    /// multiplicity inference (`obj[m1].prop[m2]` → `[m1*m2]`).
    /// This test pins down the cases dispatch relies on.
    #[test]
    fn multiplicity_product_table() {
        use crate::types::Multiplicity::*;

        // Identity: [1] × [1] = [1]
        assert_eq!(
            multiplicity_product(&PureOne, &PureOne),
            PureOne
        );
        // [1] × [0..1] = [0..1]
        assert_eq!(
            multiplicity_product(&PureOne, &ZeroOrOne),
            ZeroOrOne
        );
        // [0..1] × [1] = [0..1]
        assert_eq!(
            multiplicity_product(&ZeroOrOne, &PureOne),
            ZeroOrOne
        );
        // [*] absorbs anything → [*]
        assert_eq!(
            multiplicity_product(&ZeroOrMany, &PureOne),
            ZeroOrMany
        );
        assert_eq!(
            multiplicity_product(&PureOne, &ZeroOrMany),
            ZeroOrMany
        );
        // [1..*] × [1] = [1..*] (lower 1*1=1, upper *=*).
        assert_eq!(
            multiplicity_product(&OneOrMany, &PureOne),
            OneOrMany
        );
        // [1..*] × [0..1] = [*] (lower 1*0=0, upper *).
        assert_eq!(
            multiplicity_product(&OneOrMany, &ZeroOrOne),
            ZeroOrMany
        );
        // [2] × [1] = [2]
        assert_eq!(
            multiplicity_product(
                &Range { lower: 2, upper: Some(2) },
                &PureOne,
            ),
            Range { lower: 2, upper: Some(2) },
        );
        // [2..3] × [1..*] = [2..*]
        assert_eq!(
            multiplicity_product(
                &Range { lower: 2, upper: Some(3) },
                &OneOrMany,
            ),
            Range { lower: 2, upper: None },
        );
    }

    #[test]
    fn lower_multiplicity_variants() {
        assert_eq!(
            lower_multiplicity(&ast_type::Multiplicity::PureOne),
            Multiplicity::PureOne
        );
        assert_eq!(
            lower_multiplicity(&ast_type::Multiplicity::ZeroOrOne),
            Multiplicity::ZeroOrOne
        );
        assert_eq!(
            lower_multiplicity(&ast_type::Multiplicity::ZeroOrMany),
            Multiplicity::ZeroOrMany
        );
        assert_eq!(
            lower_multiplicity(&ast_type::Multiplicity::OneOrMany),
            Multiplicity::OneOrMany
        );
        assert_eq!(
            lower_multiplicity(&ast_type::Multiplicity::Range {
                lower: 2,
                upper: Some(5)
            }),
            Multiplicity::Range {
                lower: 2,
                upper: Some(5)
            }
        );
    }

    #[test]
    fn lower_const_value_integer() {
        let src = SourceInfo::new("test", 1, 1, 1, 5);
        let v = ast_type::TypeVariableValue::Integer(255, src);
        assert_eq!(lower_const_value(&v), ConstValue::Integer(255));
    }

    #[test]
    fn lower_const_value_string() {
        let src = SourceInfo::new("test", 1, 1, 1, 5);
        let v = ast_type::TypeVariableValue::String("ok".to_string(), src);
        assert_eq!(lower_const_value(&v), ConstValue::String("ok".to_string()));
    }

    #[test]
    fn import_scope_from_path_str() {
        let scope = ImportScope::from_path_str("meta::pure::profiles");
        assert_eq!(scope.package.to_string(), "meta::pure::profiles");
    }

    /// Imports must shadow non-primitive root aliases.
    ///
    /// Bootstrap registers M3 metaclasses (`Function`, `Class`,
    /// `Column<U,V>`, …) under the root package so unqualified
    /// references resolve when no import brings a sibling. But when an
    /// `import x::*` exposes a same-named **class** at the use-site,
    /// the user means the imported one — not the M3 alias.
    /// `platform_store_relational/functions.pure` was the canary:
    /// `import meta::relational::metamodel::*` brings in a concrete
    /// `Column`, so `cols:Column[*]` must point at it, not the M3
    /// generic.
    #[test]
    fn unqualified_import_shadows_root_metaclass_alias() {
        use crate::ids::PackageId;
        use crate::model::{Element, ElementNode, ModelChunk, PureModel};
        use crate::nodes::class::Class;

        let mut model = PureModel::new();
        // Push a placeholder bootstrap chunk so `chunk_id=0` slot 0/1
        // (Any/Nil-coded by the resolver's primitive-shadow filter) are
        // not stepped on by user content.
        model.chunks.push(ModelChunk::new(0));

        // Two `Foo` classes:
        //  - one at root (M3 metaclass alias)
        //  - one inside `pkg::sub` (the imported sibling)
        let chunk_id = 1u16;
        let mut chunk = ModelChunk::new(chunk_id);
        let si = SourceInfo::new("t.pure", 1, 1, 1, 1);

        let root_foo_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("Foo"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: model.root_package,
            },
            Element::Class(Class {
                type_parameters: vec![SmolStr::new("T")],
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
        let pkg_id: PackageId =
            model.get_or_create_package(&[SmolStr::new("pkg"), SmolStr::new("sub")]);
        let import_foo_idx = chunk.alloc_element(
            ElementNode {
                name: SmolStr::new("Foo"),
                source_info: si.clone(),
                name_source_info: si.clone(),
                parent_package: pkg_id,
            },
            Element::Class(Class {
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
        model.chunks.push(chunk);
        let root_foo_id = ElementId::InstanceId {
            chunk_id,
            local_idx: root_foo_idx,
        };
        let import_foo_id = ElementId::InstanceId {
            chunk_id,
            local_idx: import_foo_idx,
        };
        model.register_element(model.root_package, root_foo_id);
        model.register_element(pkg_id, import_foo_id);

        // Resolve `Foo` with `pkg::sub` in scope. Imported sibling wins.
        let import_scope = ImportScope::from_path_str("pkg::sub");
        let scopes = vec![import_scope];
        let mut cache = HashMap::new();
        let type_params: Vec<SmolStr> = Vec::new();
        let ctx = ResolutionContext {
            model: &model,
            import_scopes: &scopes,
            resolve_cache: &mut cache,
            type_parameters: &type_params,
            variable_types: HashMap::new(),
            island_lowerers: &[],
        };
        let mut errors = Vec::new();
        let resolved =
            resolve_unqualified(&SmolStr::new("Foo"), &SourceInfo::new("t.pure", 1, 1, 1, 1), &ctx, &mut errors);
        assert_eq!(resolved, Some(import_foo_id));
        assert!(errors.is_empty());

        // Resolve `Foo` with no relevant imports. Falls through to the
        // root M3 alias.
        let mut empty_cache = HashMap::new();
        let no_scopes: Vec<ImportScope> = Vec::new();
        let ctx2 = ResolutionContext {
            model: &model,
            import_scopes: &no_scopes,
            resolve_cache: &mut empty_cache,
            type_parameters: &type_params,
            variable_types: HashMap::new(),
            island_lowerers: &[],
        };
        let mut errors2 = Vec::new();
        let resolved2 = resolve_unqualified(
            &SmolStr::new("Foo"),
            &SourceInfo::new("t.pure", 1, 1, 1, 1),
            &ctx2,
            &mut errors2,
        );
        assert_eq!(resolved2, Some(root_foo_id));
        assert!(errors2.is_empty());
    }
}
