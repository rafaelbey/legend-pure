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
use legend_pure_parser_ast::type_ref::{self as ast_type, FUNCTION_TYPE_SENTINEL, Package};
use smol_str::SmolStr;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::{Element, PureModel};
use crate::types::{ConstValue, Multiplicity, TypeExpr};

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
    pub variable_types: HashMap<SmolStr, (crate::types::TypeExpr, crate::types::Multiplicity)>,
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

    // Lower value arguments
    let value_arguments: Vec<ConstValue> = type_ref
        .type_variable_values
        .iter()
        .map(lower_const_value)
        .collect();

    Some(TypeExpr::Named {
        element: element_id,
        type_arguments,
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
        let param_mult = param_ref
            .multiplicity_arguments
            .first()
            .map(|ma| match ma {
                ast_type::MultiplicityArgument::Concrete(m, _) => lower_multiplicity(m),
                ast_type::MultiplicityArgument::Identifier(_, _) => Multiplicity::ZeroOrMany,
            })
            .unwrap_or(Multiplicity::PureOne);

        parameters.push((param_type, param_mult));
    }

    // Return type is the last type_argument
    let return_ref = &type_ref.type_arguments[n - 1];
    let return_type =
        resolve_type_ref(return_ref, ctx, errors).unwrap_or(TypeExpr::Generic("Any".into()));

    // Return multiplicity is in the top-level multiplicity_arguments[0]
    let return_multiplicity = type_ref
        .multiplicity_arguments
        .first()
        .map(|ma| match ma {
            ast_type::MultiplicityArgument::Concrete(m, _) => lower_multiplicity(m),
            ast_type::MultiplicityArgument::Identifier(_, _) => Multiplicity::ZeroOrMany,
        })
        .unwrap_or(Multiplicity::PureOne);

    Some(TypeExpr::FunctionType {
        parameters,
        return_type: Box::new(return_type),
        return_multiplicity,
    })
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
        ast_type::TypeSpec::Relation(_rt) => {
            // TODO: Resolve each column type and intern the RelationType.
            // For now, relation type resolution is deferred — the parser and
            // composer handle relation types correctly; the compiler will be
            // updated when the relation interning infrastructure is wired up.
            None
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
    let cache_entry = match result {
        Some(id) => ResolveResult::Found(id),
        None => {
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
        }
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
    // Step 1: Try bootstrap/model root types (String, Integer, etc.)
    if let Some(id) = ctx.model.resolve_by_path(std::slice::from_ref(name)) {
        return Some(id);
    }

    // Step 2: Search import scopes using the AST Package directly
    let mut candidates: Vec<(&ImportScope, ElementId)> = Vec::new();
    for scope in ctx.import_scopes {
        if let Some(id) = ctx.model.resolve_in_package(&scope.package, name) {
            candidates.push((scope, id));
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
        ast_type::Multiplicity::ZeroOrMany | ast_type::Multiplicity::Variable(_) => {
            Multiplicity::ZeroOrMany
        }
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
pub(crate) fn resolve_stereotypes(
    stereotypes: &[ast_ann::StereotypePtr],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<StereotypeRef> {
    stereotypes
        .iter()
        .filter_map(|s| {
            let profile_id = resolve_element_ptr(&s.profile, &s.source_info, ctx, errors)?;
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
/// is skipped.
pub(crate) fn resolve_tagged_values(
    tagged_values: &[ast_ann::TaggedValue],
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<TaggedValueRef> {
    tagged_values
        .iter()
        .filter_map(|tv| {
            let profile_id = resolve_element_ptr(&tv.tag.profile, &tv.source_info, ctx, errors)?;
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
        resolve_unqualified_cached(ptr.name(), source_info, ctx, errors)
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
        let display = SmolStr::new(format!("{}::{}", pkg, name));
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
        if root_filtered.len() == 1 {
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

        if all_candidates.is_empty() {
            // No function with this simple name and arg count in any import scope
            errors.push(CompilationError {
                message: format!("Cannot resolve function '{name}'"),
                source_info: source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: name.clone() },
            });
            None
        } else if all_candidates.len() == 1 {
            Some(all_candidates[0])
        } else {
            // Multiple overloads with same param count — narrow by type
            let narrowed = narrow_candidates_by_type(
                &all_candidates,
                lowered_args,
                ctx.model,
                &ctx.variable_types,
            );
            if narrowed.len() == 1 {
                return Some(narrowed[0]);
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

// ---------------------------------------------------------------------------
// Type-based dispatch helpers
// ---------------------------------------------------------------------------

/// Infers a type `ElementId` from a lowered `ValueSpec` by examining
/// its `ExprKind` structure. Returns `None` for expressions whose type
/// cannot be statically determined (treated as `Any` — matches everything).
/// Type alias for variable scope: name → (type, multiplicity).
type VarTypes = HashMap<SmolStr, (crate::types::TypeExpr, crate::types::Multiplicity)>;

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
        ExprKind::Lambda { .. } => None, // FunctionType — no ElementId
        ExprKind::FunctionCall { function, .. } => {
            // Use the return type of the resolved function
            function.and_then(|fid| {
                if let Element::Function(f) = model.get_element(fid) {
                    match &f.return_type {
                        crate::types::TypeExpr::Named { element, .. } => Some(*element),
                        _ => None,
                    }
                } else {
                    None
                }
            })
        }
        ExprKind::Variable { name } => {
            // Look up declared type from function params / let / lambda
            var_types.get(name).and_then(|(te, _)| match te {
                crate::types::TypeExpr::Named { element, .. } => Some(*element),
                _ => None,
            })
        }
        ExprKind::EnumValue { enum_element, .. } => Some(*enum_element),
        // Property access, collection, etc. — type unknown
        _ => None,
    }
}

/// Checks if `arg_type` is compatible with `param_type` in the type hierarchy.
///
/// Compatible means: same type, or arg_type is a subtype of param_type.
/// Returns true if we can't determine (either side is `None`/`Any`/generic).
fn is_type_compatible(
    arg_type: Option<ElementId>,
    param_type: &crate::types::TypeExpr,
    model: &crate::model::PureModel,
) -> bool {
    use crate::bootstrap;

    let param_eid = match param_type {
        crate::types::TypeExpr::Named { element, .. } => *element,
        crate::types::TypeExpr::Generic(_) => return true, // Generic matches anything
        crate::types::TypeExpr::FunctionType { .. } => return true, // Can't check structurally
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

/// Checks if `child` is a subtype of `parent` by walking the supertype chain.
fn is_subtype(child: ElementId, parent: ElementId, model: &crate::model::PureModel) -> bool {
    if child == parent {
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
        {
            if *sup_eid == parent || is_subtype(*sup_eid, parent, model) {
                return true;
            }
        }
    }
    false
}

/// Infers multiplicity from a lowered `ValueSpec` by examining `ExprKind`.
/// Returns `None` for expressions whose multiplicity can't be determined.
fn infer_multiplicity_from_valuespec(
    vs: &crate::types::ValueSpec,
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Option<crate::types::Multiplicity> {
    use crate::types::{ExprKind, Multiplicity};

    match vs.kind.as_ref() {
        // All literals produce exactly one value
        ExprKind::IntegerLiteral(_)
        | ExprKind::FloatLiteral(_)
        | ExprKind::DecimalLiteral(_)
        | ExprKind::StringLiteral(_)
        | ExprKind::BooleanLiteral(_)
        | ExprKind::DateLiteral(_) => Some(Multiplicity::PureOne),

        // Enum values are always [1]
        ExprKind::EnumValue { .. } => Some(Multiplicity::PureOne),

        // Lambda is [1]
        ExprKind::Lambda { .. } => Some(Multiplicity::PureOne),

        // Collection → [*]
        ExprKind::Collection { .. } => Some(Multiplicity::ZeroOrMany),

        // Function call → return multiplicity of the resolved function
        ExprKind::FunctionCall { function, .. } => function.and_then(|fid| {
            if let Element::Function(f) = model.get_element(fid) {
                Some(f.return_multiplicity.clone())
            } else {
                None
            }
        }),

        // Variable → look up declared multiplicity
        ExprKind::Variable { name } => var_types.get(name).map(|(_, m)| m.clone()),

        // Property access, etc. — unknown
        _ => None,
    }
}

/// Checks if `arg_mult` is compatible with `param_mult`.
///
/// Compatible means: the arg's multiplicity fits within the param's range.
/// `[1]` fits into `[0..1]`, `[0..1]`, `[1..*]`, `[*]`.
/// `[*]` only fits into `[*]`.
fn is_multiplicity_compatible(
    arg_mult: &Option<crate::types::Multiplicity>,
    param_mult: &crate::types::Multiplicity,
) -> bool {
    let Some(am) = arg_mult else {
        // Unknown — assume compatible
        return true;
    };

    // Check if arg's range is a subset of param's range
    let (arg_lo, arg_hi) = mult_bounds(am);
    let (param_lo, param_hi) = mult_bounds(param_mult);

    // arg's lower >= param's lower AND arg's upper <= param's upper
    arg_lo >= param_lo && arg_hi <= param_hi
}

/// Returns (lower, upper) bounds for a multiplicity.
/// `None` upper means unbounded (represented as u32::MAX).
fn mult_bounds(m: &crate::types::Multiplicity) -> (u32, u32) {
    use crate::types::Multiplicity;
    match m {
        Multiplicity::PureOne => (1, 1),
        Multiplicity::ZeroOrOne => (0, 1),
        Multiplicity::ZeroOrMany => (0, u32::MAX),
        Multiplicity::OneOrMany => (1, u32::MAX),
        Multiplicity::Range { lower, upper } => (*lower, upper.unwrap_or(u32::MAX)),
    }
}

/// Multiplicity specificity score — more specific = higher.
/// `[1]` = 4, `[0..1]` = 3, `[1..*]` = 2, `[*]` = 1
fn mult_specificity(m: &crate::types::Multiplicity) -> i32 {
    use crate::types::Multiplicity;
    match m {
        Multiplicity::PureOne => 4,
        Multiplicity::ZeroOrOne => 3,
        Multiplicity::OneOrMany => 2,
        Multiplicity::Range { upper: Some(_), .. } => 3, // bounded range
        Multiplicity::Range { upper: None, .. } => 1,    // unbounded
        Multiplicity::ZeroOrMany => 1,
    }
}

/// Narrows function candidates by checking argument types AND multiplicities
/// against parameter types/multiplicities.
///
/// Scoring: type exact match (+3), type subtype (+1), multiplicity exact (+2),
/// multiplicity specificity bonus (+1). Type incompatibility eliminates a candidate.
/// If all candidates are eliminated, returns the original set.
fn narrow_candidates_by_type(
    candidates: &[ElementId],
    lowered_args: &[crate::types::ValueSpec],
    model: &crate::model::PureModel,
    var_types: &VarTypes,
) -> Vec<ElementId> {
    if candidates.len() <= 1 {
        return candidates.to_vec();
    }

    // Infer types and multiplicities of each argument
    let arg_types: Vec<Option<ElementId>> = lowered_args
        .iter()
        .map(|vs| infer_type_from_valuespec(vs, model, var_types))
        .collect();

    let arg_mults: Vec<Option<crate::types::Multiplicity>> = lowered_args
        .iter()
        .map(|vs| infer_multiplicity_from_valuespec(vs, model, var_types))
        .collect();

    // Score each candidate
    let mut scored: Vec<(ElementId, i32)> = candidates
        .iter()
        .filter_map(|&eid| {
            let Element::Function(f) = model.get_element(eid) else {
                return None;
            };
            let mut score: i32 = 0;
            let mut compatible = true;
            for (i, param) in f.parameters.iter().enumerate() {
                // --- Type scoring ---
                let arg_type = arg_types.get(i).copied().flatten();
                if !is_type_compatible(arg_type, &param.type_expr, model) {
                    compatible = false;
                    break;
                }
                if let Some(at) = arg_type {
                    if let crate::types::TypeExpr::Named { element: pe, .. } = &param.type_expr {
                        if at == *pe {
                            score += 3; // exact type match
                        } else if is_subtype(at, *pe, model) {
                            score += 1; // subtype match
                        }
                    }
                }

                // --- Multiplicity scoring ---
                let arg_mult = arg_mults.get(i).cloned().flatten();
                if !is_multiplicity_compatible(&arg_mult, &param.multiplicity) {
                    compatible = false;
                    break;
                }
                if let Some(ref am) = arg_mult {
                    if *am == param.multiplicity {
                        score += 4; // exact multiplicity match
                    } else {
                        // Prefer narrower param multiplicity that still fits
                        score += mult_specificity(&param.multiplicity);
                    }
                } else {
                    // Unknown arg mult — use param specificity as tiebreaker
                    score += mult_specificity(&param.multiplicity);
                }
            }
            if compatible { Some((eid, score)) } else { None }
        })
        .collect();

    if scored.is_empty() {
        // Narrowing eliminated everything — fall back to original
        return candidates.to_vec();
    }

    // Sort by score descending, pick the best
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    let best_score = scored[0].1;

    // Return all candidates with the best score
    scored
        .into_iter()
        .filter(|(_, s)| *s == best_score)
        .map(|(eid, _)| eid)
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}
