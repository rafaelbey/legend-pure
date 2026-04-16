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
use legend_pure_parser_ast::expression as ast_expr;
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
/// `call_args` carries the AST argument expressions from the call site.
/// Currently used for parameter-count filtering; will enable full
/// type-based dispatch when implemented.
pub(crate) fn resolve_function_call(
    ptr: &ast_ann::PackageableElementPtr,
    call_args: &[ast_expr::Expression],
    source_info: &SourceInfo,
    ctx: &mut ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ElementId> {
    let name = ptr.name();
    let arg_count = call_args.len();

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
            if filtered.len() == 1 {
                return Some(filtered[0]);
            }
            if let Some(&first) = filtered.first() {
                return Some(first);
            }
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
            // Multiple overloads with same param count — needs type-based dispatch
            errors.push(CompilationError {
                message: format!(
                    "Ambiguous function call '{name}': found {} overloads with {} args, \
                     type-based dispatch not yet implemented",
                    all_candidates.len(),
                    arg_count
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
