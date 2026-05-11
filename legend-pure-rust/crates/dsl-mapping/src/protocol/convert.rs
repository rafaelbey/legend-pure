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

//! AST → protocol conversion impls for Mapping DSL types.
//!
//! Pattern: `impl From<&ast::*> for Protocol*`. Two-step `AST →
//! Protocol → JSON`, same idiom as `crates/protocol/src/v1/convert.rs`.
//!
//! **c1 scope**: container (`MappingDef → ProtocolMapping`) +
//! includes (`MappingInclude → ProtocolMappingInclude`) + class-
//! mapping headers (`ClassMapping → ProtocolClassMapping::*Header`).
//! Body-specific data per variant (filter, src class, parameters,
//! lambda, …) lands in c2-c6.

use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_protocol::v1::source_info::SourceInformation;
use legend_pure_parser_protocol::v1::value_spec::LambdaFunction;

use crate::ast::{
    AggregateSpecification, AggregateView, AggregationAwareClassMappingBody,
    AggregationFunctionSpec, ClassMapping, ClassMappingBody, EnumSourceValue, EnumValueMapping,
    EnumerationClassMappingBody, LocalPropertyDecl, MappingDef, MappingInclude,
    NestedClassMapping, OperationClassMappingBody, PureClassMappingBody, PurePropertyMapping,
    StoreSubstitution,
};
use crate::protocol::aggregation_aware::{
    ProtocolAggregateFunction, ProtocolAggregateSetImplementationContainer,
    ProtocolAggregateSpecification, ProtocolAggregationAwareClassMapping,
    ProtocolGroupByFunction,
};
use crate::protocol::class_mapping::{ProtocolClassMapping, ProtocolClassMappingHeader};
use crate::protocol::enumeration::{
    ProtocolEnumValueMapping, ProtocolEnumValueMappingEnumSourceValue,
    ProtocolEnumValueMappingIntegerSourceValue, ProtocolEnumValueMappingSourceValue,
    ProtocolEnumValueMappingStringSourceValue, ProtocolEnumerationMapping,
};
use crate::protocol::include::{ProtocolMappingInclude, ProtocolMappingIncludeMapping};
use crate::protocol::operation::{
    MappingOperation, ProtocolMergeOperationClassMapping, ProtocolOperationClassMapping,
};
use crate::protocol::pure::{
    ProtocolLocalMappingPropertyInfo, ProtocolPropertyMapping, ProtocolPropertyPointer,
    ProtocolPureInstanceClassMapping, ProtocolPurePropertyMapping,
};
use crate::protocol::ProtocolMapping;
use legend_pure_parser_protocol::v1::value_spec::ProtocolPackageableElementPtr;

/// Convert an AST `MappingDef` to its protocol JSON representation.
///
/// Container-shape only — class-mapping bodies serialize via the
/// header stub introduced in c1. Body data fills in c2-c6.
impl From<&MappingDef> for ProtocolMapping {
    fn from(m: &MappingDef) -> Self {
        let package_path = match &m.package {
            Some(pkg) => pkg.to_string(),
            None => String::new(),
        };
        // Split: Enumeration-bodied class mappings route into
        // `enumeration_mappings`; everything else into `class_mappings`.
        // Java's `Mapping.enumerationMappings` is a sibling list to
        // `classMappings`, NOT under the `ClassMapping` discriminator.
        let mut class_mappings = Vec::new();
        let mut enumeration_mappings = Vec::new();
        for cm in &m.class_mappings {
            match &cm.body {
                ClassMappingBody::Enumeration(body) => {
                    enumeration_mappings.push(enumeration_mapping_from(cm, body));
                }
                _ => class_mappings.push(ProtocolClassMapping::from(cm)),
            }
        }
        Self {
            package_path,
            name: m.name.value.to_string(),
            class_mappings,
            enumeration_mappings,
            included_mappings: m.includes.iter().map(ProtocolMappingInclude::from).collect(),
            source_information: Some(source_info_from(&m.source_info)),
        }
    }
}

/// Convert an AST `MappingInclude` to its Java-parity protocol shape.
///
/// Java's `MappingIncludeMapping` JSON carries only a single
/// `(source, target)` substitution pair, even though the grammar
/// accepts a comma-separated list. Mirror that lossy behaviour:
/// populate the pair only when the AST holds exactly one
/// substitution. See `protocol::include` doc for the parity reference.
impl From<&MappingInclude> for ProtocolMappingInclude {
    fn from(inc: &MappingInclude) -> Self {
        let included_mapping = ptr_to_fqn(&inc.included);
        let (source_database_path, target_database_path) =
            single_substitution(&inc.store_substitutions);
        Self::MappingIncludeMapping(ProtocolMappingIncludeMapping {
            included_mapping,
            source_database_path,
            target_database_path,
            source_information: Some(source_info_from(&inc.source_info)),
        })
    }
}

/// Convert an AST `ClassMapping` to a protocol `ClassMapping` header.
///
/// **c1 scope**: dispatch only — every body kind serializes via the
/// header stub. Subsequent commits replace the header with body-
/// flattened structs per variant.
impl From<&ClassMapping> for ProtocolClassMapping {
    fn from(cm: &ClassMapping) -> Self {
        let header = ProtocolClassMappingHeader {
            id: cm.id.as_ref().map(ToString::to_string),
            class: ptr_to_fqn(&cm.class),
            extends_class_mapping_id: cm.extends.as_ref().map(ToString::to_string),
            root: cm.is_root,
            source_information: Some(source_info_from(&cm.source_info)),
        };
        match &cm.body {
            ClassMappingBody::Pure(body) => ProtocolClassMapping::PureInstance(
                pure_instance_from_body(body, header, ptr_to_fqn(&cm.class)),
            ),
            ClassMappingBody::Operation(body) => {
                if body.validation_function.is_some() {
                    ProtocolClassMapping::MergeOperation(merge_operation_from_body(body, header))
                } else {
                    ProtocolClassMapping::Operation(operation_from_body(body, header))
                }
            }
            ClassMappingBody::AggregationAware(body) => {
                ProtocolClassMapping::AggregationAware(Box::new(
                    aggregation_aware_from_body(body, header, cm),
                ))
            }
            ClassMappingBody::RelationFunction(_) => ProtocolClassMapping::Relation(header),
            // Enumeration / XStore / Foreign bodies don't route to
            // `classMappings` in Java — `enumerationMappings` and
            // `associationMappings` are sibling lists. We don't yet
            // emit those in c1; carry them through the `PureInstance`
            // variant so the dispatcher stays exhaustive. c4/c7 will
            // split them out properly.
            ClassMappingBody::Enumeration(_)
            | ClassMappingBody::XStore(_)
            | ClassMappingBody::Foreign(_) => ProtocolClassMapping::PureInstance(
                ProtocolPureInstanceClassMapping {
                    header,
                    src_class: None,
                    source_class_source_information: None,
                    property_mappings: Vec::new(),
                    filter: None,
                },
            ),
        }
    }
}

/// Build a `ProtocolPureInstanceClassMapping` from a `PureClassMappingBody`.
///
/// `target_class_fqn` carries the outer class FQN — it's needed by
/// each `PropertyPointer.class` field on every property mapping
/// since the AST stores it once on the parent `ClassMapping`.
fn pure_instance_from_body(
    body: &PureClassMappingBody,
    header: ProtocolClassMappingHeader,
    target_class_fqn: String,
) -> ProtocolPureInstanceClassMapping {
    let src_class = body.src_class.as_ref().map(ptr_to_fqn);
    let source_class_source_information = body
        .src_class
        .as_ref()
        .map(|p| source_info_from(&p.source_info));
    let property_mappings = body
        .property_mappings
        .iter()
        .map(|pm| pure_property_mapping_from_ast(pm, &target_class_fqn))
        .collect();
    let filter = body.filter.as_ref().map(lambda_wrap);
    ProtocolPureInstanceClassMapping {
        header,
        src_class,
        source_class_source_information,
        property_mappings,
        filter,
    }
}

fn pure_property_mapping_from_ast(
    pm: &PurePropertyMapping,
    target_class_fqn: &str,
) -> ProtocolPropertyMapping {
    let property = ProtocolPropertyPointer {
        class: target_class_fqn.to_string(),
        property: pm.property_name.to_string(),
        source_information: Some(source_info_from(&pm.source_info)),
    };
    let local_mapping_property = pm
        .local_property
        .as_ref()
        .map(local_mapping_property_from);
    let enum_mapping_id = pm.transformer.as_ref().map(ToString::to_string);
    let transform = lambda_wrap(&pm.transform);
    // Java's `explodeProperty` is `Boolean` (nullable). Emit `Some(true)`
    // when the `*` modifier was set; `None` (absent) otherwise — round-
    // trip stable with Java JSON that omits the field for non-exploding
    // mappings.
    let explode_property = pm.explode.then_some(true);
    ProtocolPropertyMapping::PurePropertyMapping(ProtocolPurePropertyMapping {
        property,
        source: None,
        target: None,
        local_mapping_property,
        enum_mapping_id,
        transform,
        explode_property,
        source_information: Some(source_info_from(&pm.source_info)),
    })
}

fn local_mapping_property_from(local: &LocalPropertyDecl) -> ProtocolLocalMappingPropertyInfo {
    use legend_pure_parser_ast::type_ref::Multiplicity as AstMult;
    use legend_pure_parser_protocol::v1::multiplicity::Multiplicity as ProtoMult;

    // Render the type path as a `::`-joined FQN (Java's
    // `LocalMappingPropertyInfo.type` is a plain string).
    let type_path = if let Some(pkg) = &local.type_ref.package {
        format!("{pkg}::{}", local.type_ref.name)
    } else {
        local.type_ref.name.to_string()
    };
    let multiplicity = match &local.multiplicity {
        AstMult::PureOne => ProtoMult {
            lower_bound: 1,
            upper_bound: Some(1),
        },
        AstMult::ZeroOrOne => ProtoMult {
            lower_bound: 0,
            upper_bound: Some(1),
        },
        AstMult::OneOrMany => ProtoMult {
            lower_bound: 1,
            upper_bound: None,
        },
        AstMult::ZeroOrMany => ProtoMult {
            lower_bound: 0,
            upper_bound: None,
        },
        AstMult::Range { lower, upper } => ProtoMult {
            lower_bound: *lower,
            upper_bound: *upper,
        },
        // `Variable(name)` doesn't have a numeric form. Java protocol
        // doesn't carry variable multiplicities on
        // `LocalMappingPropertyInfo` — emit a [0..*] placeholder that
        // serializes cleanly. (No platform fixture currently uses
        // variable multiplicity on a local mapping property.)
        AstMult::Variable(_) => ProtoMult {
            lower_bound: 0,
            upper_bound: None,
        },
    };
    ProtocolLocalMappingPropertyInfo {
        type_path,
        multiplicity,
        source_information: Some(source_info_from(&local.source_info)),
    }
}

/// Wrap a bare `Expression` (filter / transform body) as a
/// no-parameter `LambdaFunction`. Java's
/// `PureInstanceClassMappingParseTreeWalker.visitLambda:111-117`
/// builds the same shape — `body = [valueSpec]`, `parameters = []`.
fn lambda_wrap(expr: &Expression) -> LambdaFunction {
    LambdaFunction {
        body: vec![legend_pure_parser_protocol::v1::convert::convert_expression_typed(expr)],
        parameters: Vec::new(),
        source_information: Some(source_info_from(expr.source_info())),
    }
}

/// Build a `ProtocolOperationClassMapping` from the simple
/// `parameters` form of `OperationClassMappingBody`.
fn operation_from_body(
    body: &OperationClassMappingBody,
    header: ProtocolClassMappingHeader,
) -> ProtocolOperationClassMapping {
    ProtocolOperationClassMapping {
        header,
        parameters: body.parameters.iter().map(|p| p.id.to_string()).collect(),
        operation: MappingOperation::from_function_fqn(&ptr_to_fqn(&body.operation)),
    }
}

/// Build a `ProtocolAggregationAwareClassMapping` from the AST body
/// + the outer `ClassMapping` (needed to thread the target class FQN
/// through the nested mapping conversion — nested mappings inherit
/// the outer class).
fn aggregation_aware_from_body(
    body: &AggregationAwareClassMappingBody,
    header: ProtocolClassMappingHeader,
    outer_cm: &ClassMapping,
) -> ProtocolAggregationAwareClassMapping {
    let main_set_implementation = Box::new(nested_to_class_mapping(&body.main_mapping, outer_cm));
    let aggregate_set_implementations = body
        .views
        .iter()
        .enumerate()
        .map(|(idx, view)| aggregate_container_from_view(idx, view, outer_cm))
        .collect();
    ProtocolAggregationAwareClassMapping {
        header,
        main_set_implementation,
        // Java's post-processor synthesises propertyMappings here from
        // mainSetImplementation. AST doesn't carry that synthesised
        // list — emit empty.
        property_mappings: Vec::new(),
        aggregate_set_implementations,
    }
}

fn aggregate_container_from_view(
    index: usize,
    view: &AggregateView,
    outer_cm: &ClassMapping,
) -> ProtocolAggregateSetImplementationContainer {
    ProtocolAggregateSetImplementationContainer {
        // Java's `index` is `Long`; we feed the 0-based source order.
        index: index as i64,
        set_implementation: Box::new(nested_to_class_mapping(&view.aggregate_mapping, outer_cm)),
        aggregate_specification: aggregate_specification_from(&view.model_operation),
    }
}

fn aggregate_specification_from(spec: &AggregateSpecification) -> ProtocolAggregateSpecification {
    ProtocolAggregateSpecification {
        can_aggregate: spec.can_aggregate,
        group_by_functions: spec
            .group_by_functions
            .iter()
            .map(|expr| ProtocolGroupByFunction {
                group_by_fn: lambda_wrap(expr),
            })
            .collect(),
        aggregate_values: spec
            .aggregate_values
            .iter()
            .map(aggregate_function_from)
            .collect(),
    }
}

fn aggregate_function_from(spec: &AggregationFunctionSpec) -> ProtocolAggregateFunction {
    ProtocolAggregateFunction {
        map_fn: lambda_wrap(&spec.map_fn),
        aggregate_fn: lambda_wrap(&spec.aggregate_fn),
    }
}

/// Convert a `NestedClassMapping` (`~mainMapping` / `~aggregateMapping`)
/// to a full `ProtocolClassMapping`. Threads the outer class FQN
/// through a synthetic `ClassMapping` so the nested body's conversion
/// path lands on the right Java discriminator with the right `class`
/// field.
fn nested_to_class_mapping(nested: &NestedClassMapping, outer_cm: &ClassMapping) -> ProtocolClassMapping {
    // Build a synthetic ClassMapping that inherits the outer's class
    // + spans the nested body. `id`/`extends`/`is_root` reset to
    // defaults — nested mappings carry none of those per the AST.
    let synthetic = ClassMapping {
        is_root: outer_cm.is_root,
        class: outer_cm.class.clone(),
        id: None,
        extends: None,
        mapping_name: None,
        body: nested.body.clone(),
        source_info: nested.source_info.clone(),
    };
    ProtocolClassMapping::from(&synthetic)
}

/// Build a `ProtocolEnumerationMapping` from an outer `ClassMapping`
/// + its inner `EnumerationClassMappingBody`. The outer FQN supplies
/// the target enumeration; the inner body carries the per-value
/// source-value lists.
///
/// Called only from the `MappingDef → ProtocolMapping` filter that
/// routes Enumeration bodies into the sibling
/// `enumeration_mappings` list rather than `class_mappings`.
fn enumeration_mapping_from(
    cm: &ClassMapping,
    body: &EnumerationClassMappingBody,
) -> ProtocolEnumerationMapping {
    let enumeration_fqn = ptr_to_fqn(&cm.class);
    let id = cm.id.as_ref().map(ToString::to_string).unwrap_or_else(|| {
        // Java default: ID falls back to the enumeration's simple
        // name (last `::` segment). `cm.class.name` already holds it.
        cm.class.name.to_string()
    });
    ProtocolEnumerationMapping {
        id,
        enumeration: ProtocolPackageableElementPtr {
            full_path: enumeration_fqn,
            source_information: Some(source_info_from(&cm.class.source_info)),
        },
        enum_value_mappings: body
            .value_mappings
            .iter()
            .map(enum_value_mapping_from)
            .collect(),
        source_information: Some(source_info_from(&cm.source_info)),
    }
}

fn enum_value_mapping_from(vm: &EnumValueMapping) -> ProtocolEnumValueMapping {
    ProtocolEnumValueMapping {
        enum_value: vm.enum_value_name.to_string(),
        source_values: vm
            .source_values
            .iter()
            .map(enum_source_value_from)
            .collect(),
    }
}

fn enum_source_value_from(sv: &EnumSourceValue) -> ProtocolEnumValueMappingSourceValue {
    match sv {
        EnumSourceValue::String { value, .. } => ProtocolEnumValueMappingSourceValue::String(
            ProtocolEnumValueMappingStringSourceValue {
                value: value.to_string(),
            },
        ),
        EnumSourceValue::Integer { value, .. } => ProtocolEnumValueMappingSourceValue::Integer(
            ProtocolEnumValueMappingIntegerSourceValue {
                // Java's source-value is `Integer` (32-bit). Truncate
                // safely — Pure source integers exceeding i32 range
                // are extraordinarily rare in enum-mapping contexts
                // (these are enum source IDs, not numeric data).
                value: i32::try_from(*value).unwrap_or(0),
            },
        ),
        EnumSourceValue::EnumRef {
            enumeration,
            value_name,
            ..
        } => ProtocolEnumValueMappingSourceValue::Enum(
            ProtocolEnumValueMappingEnumSourceValue {
                enumeration: ptr_to_fqn(enumeration),
                value: value_name.to_string(),
            },
        ),
    }
}

/// Build a `ProtocolMergeOperationClassMapping` from the
/// `mergeParameters` form of `OperationClassMappingBody`. The caller
/// guarantees `body.validation_function.is_some()` (only the
/// dispatcher in `From<&ClassMapping>` routes here).
fn merge_operation_from_body(
    body: &OperationClassMappingBody,
    header: ProtocolClassMappingHeader,
) -> ProtocolMergeOperationClassMapping {
    let validation_function = body
        .validation_function
        .as_ref()
        .map(lambda_wrap)
        .unwrap_or_else(|| LambdaFunction {
            body: Vec::new(),
            parameters: Vec::new(),
            source_information: None,
        });
    ProtocolMergeOperationClassMapping {
        header,
        parameters: body.parameters.iter().map(|p| p.id.to_string()).collect(),
        operation: MappingOperation::from_function_fqn(&ptr_to_fqn(&body.operation)),
        validation_function,
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn ptr_to_fqn(p: &PackageableElementPtr) -> String {
    if let Some(pkg) = &p.package {
        format!("{pkg}::{}", p.name)
    } else {
        p.name.to_string()
    }
}

/// Java parity: when the AST has exactly one substitution, populate
/// `(source, target)`; otherwise both are `None` (Java drops them).
fn single_substitution(subs: &[StoreSubstitution]) -> (Option<String>, Option<String>) {
    if subs.len() != 1 {
        return (None, None);
    }
    let sub = &subs[0];
    (Some(ptr_to_fqn(&sub.source)), Some(ptr_to_fqn(&sub.target)))
}

fn source_info_from(si: &legend_pure_parser_ast::SourceInfo) -> SourceInformation {
    SourceInformation {
        source_id: si.source.to_string(),
        start_line: si.start_line,
        start_column: si.start_column,
        end_line: si.end_line,
        end_column: si.end_column,
    }
}
