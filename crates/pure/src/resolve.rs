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
use legend_pure_parser_ast::type_ref as ast_type;
use smol_str::SmolStr;

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::error::{CompilationError, CompilationErrorKind};
use crate::ids::ElementId;
use crate::model::PureModel;
use crate::types::{ConstValue, Multiplicity, TypeExpr};

// ---------------------------------------------------------------------------
// Type Resolution Context
// ---------------------------------------------------------------------------

/// Everything needed to resolve an AST type reference to a `TypeExpr`.
///
/// Holds references to both user-declared elements and the compiled model
/// (for bootstrap primitives).
pub(crate) struct ResolutionContext<'a> {
    /// User-declared elements: FQN → `ElementId`.
    pub declarations: &'a HashMap<SmolStr, ElementId>,
    /// The model (for `resolve_by_path` — bootstrap primitives).
    pub model: &'a PureModel,
}

// ---------------------------------------------------------------------------
// Type Reference Resolution
// ---------------------------------------------------------------------------

/// Resolves an AST `TypeReference` to a Pure `TypeExpr`.
///
/// Resolution order:
/// 1. User-declared elements (by FQN string)
/// 2. Bootstrap/model elements (by path segments via `resolve_by_path`)
///
/// Returns `None` and pushes a `CompilationError` if the type cannot be resolved.
pub(crate) fn resolve_type_ref(
    type_ref: &ast_type::TypeReference,
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<TypeExpr> {
    let fqn = SmolStr::new(type_ref.full_path());

    // 1. Try user declarations
    let element_id = if let Some(&id) = ctx.declarations.get(&fqn) {
        id
    } else {
        // 2. Try model path resolution (catches bootstrap primitives like String, Integer)
        let segments = type_ref_segments(type_ref);
        if let Some(id) = ctx.model.resolve_by_path(&segments) {
            id
        } else {
            errors.push(CompilationError {
                message: format!("Cannot resolve type '{fqn}'"),
                source_info: type_ref.source_info.clone(),
                kind: CompilationErrorKind::UnresolvedElement { path: fqn },
            });
            return None;
        }
    };

    // Recursively resolve type arguments
    let type_arguments: Vec<TypeExpr> = type_ref.type_arguments.iter()
        .filter_map(|arg| resolve_type_ref(arg, ctx, errors))
        .collect();

    // Lower value arguments
    let value_arguments: Vec<ConstValue> = type_ref.type_variable_values.iter()
        .map(lower_const_value)
        .collect();

    Some(TypeExpr::Named {
        element: element_id,
        type_arguments,
        value_arguments,
    })
}

/// Resolves an AST `TypeSpec` (type or unit reference) to a Pure `TypeExpr`.
///
/// For regular types, delegates to [`resolve_type_ref`].
/// For unit references (`Measure~UnitName`), resolves to the specific Unit
/// element by looking up its `Measure~UnitName` FQN.
pub(crate) fn resolve_type_spec(
    type_spec: &ast_type::TypeSpec,
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<TypeExpr> {
    match type_spec {
        ast_type::TypeSpec::Type(tr) => resolve_type_ref(tr, ctx, errors),
        ast_type::TypeSpec::Unit(ur) => {
            // Build unit FQN: "pkg::Measure~UnitName"
            let measure_fqn = SmolStr::new(ur.measure.full_path());
            let unit_name = ur.unit.as_str();
            let unit_fqn = SmolStr::new(format!("{measure_fqn}~{unit_name}"));

            // Try user declarations first, then model
            let element_id = if let Some(&id) = ctx.declarations.get(&unit_fqn) {
                id
            } else {
                // Try model path resolution (for bootstrap or pre-compiled units)
                let segments: Vec<SmolStr> = unit_fqn.split("::").map(SmolStr::new).collect();
                if let Some(id) = ctx.model.resolve_by_path(&segments) {
                    id
                } else {
                    errors.push(CompilationError {
                        message: format!("Cannot resolve unit '{unit_fqn}'"),
                        source_info: ur.source_info.clone(),
                        kind: CompilationErrorKind::UnresolvedElement { path: unit_fqn },
                    });
                    return None;
                }
            };

            Some(TypeExpr::Named {
                element: element_id,
                type_arguments: vec![],
                value_arguments: vec![],
            })
        }
    }
}

/// Collects path segments from a `TypeReference`'s package + name.
fn type_ref_segments(type_ref: &ast_type::TypeReference) -> Vec<SmolStr> {
    let mut segments = Vec::new();
    if let Some(pkg) = &type_ref.package {
        segments.extend(crate::pipeline::collect_package_segments(pkg));
    }
    segments.push(type_ref.name.clone());
    segments
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
        ast_type::Multiplicity::PureOne => Multiplicity::PureOne,
        ast_type::Multiplicity::ZeroOrOne => Multiplicity::ZeroOrOne,
        ast_type::Multiplicity::ZeroOrMany => Multiplicity::ZeroOrMany,
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
// Expression Placeholder
// ---------------------------------------------------------------------------

/// Creates a placeholder `Expression` from an AST source location.
///
/// Full expression lowering is deferred to Phase 4+. For now, we preserve
/// the source location so diagnostics can still point to the right place.
pub(crate) fn placeholder_expression(source_info: &SourceInfo) -> crate::types::Expression {
    crate::types::Expression {
        source_info: source_info.clone(),
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
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<StereotypeRef> {
    stereotypes.iter()
        .filter_map(|s| {
            let fqn = element_ptr_fqn(&s.profile);
            let profile_id = resolve_element_by_fqn(&fqn, &s.source_info, ctx, errors)?;
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
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Vec<TaggedValueRef> {
    tagged_values.iter()
        .filter_map(|tv| {
            let fqn = element_ptr_fqn(&tv.tag.profile);
            let profile_id = resolve_element_by_fqn(&fqn, &tv.source_info, ctx, errors)?;
            Some(TaggedValueRef {
                profile: profile_id,
                tag: tv.tag.value.clone(),
                value: tv.value.clone(),
            })
        })
        .collect()
}

/// Resolves a fully qualified name to an `ElementId`.
///
/// Checks user declarations first, then the model's package tree.
fn resolve_element_by_fqn(
    fqn: &SmolStr,
    source_info: &SourceInfo,
    ctx: &ResolutionContext<'_>,
    errors: &mut Vec<CompilationError>,
) -> Option<ElementId> {
    if let Some(&id) = ctx.declarations.get(fqn) {
        return Some(id);
    }

    // Try model path resolution
    let segments: Vec<SmolStr> = fqn.split("::").map(SmolStr::new).collect();
    if let Some(id) = ctx.model.resolve_by_path(&segments) {
        return Some(id);
    }

    errors.push(CompilationError {
        message: format!("Cannot resolve element '{fqn}'"),
        source_info: source_info.clone(),
        kind: CompilationErrorKind::UnresolvedElement { path: fqn.clone() },
    });
    None
}

/// Builds a fully qualified name string from a `PackageableElementPtr`.
fn element_ptr_fqn(ptr: &ast_ann::PackageableElementPtr) -> SmolStr {
    match ptr.package() {
        Some(pkg) => {
            let segments = crate::pipeline::collect_package_segments(pkg);
            let mut fqn = String::new();
            for (i, seg) in segments.iter().enumerate() {
                if i > 0 {
                    fqn.push_str("::");
                }
                fqn.push_str(seg);
            }
            fqn.push_str("::");
            fqn.push_str(ptr.name());
            SmolStr::new(&fqn)
        }
        None => ptr.name().clone(),
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
            lower_multiplicity(&ast_type::Multiplicity::Range { lower: 2, upper: Some(5) }),
            Multiplicity::Range { lower: 2, upper: Some(5) }
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
    fn placeholder_expression_has_source() {
        let src = SourceInfo::new("test.pure", 10, 5, 10, 20);
        let expr = placeholder_expression(&src);
        assert_eq!(expr.source_info.start_line, 10);
        assert_eq!(expr.source_info.start_column, 5);
    }
}
