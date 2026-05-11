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
use legend_pure_parser_protocol::v1::source_info::SourceInformation;

use crate::ast::{
    ClassMapping, ClassMappingBody, MappingDef, MappingInclude, StoreSubstitution,
};
use crate::protocol::class_mapping::{ProtocolClassMapping, ProtocolClassMappingHeader};
use crate::protocol::include::{ProtocolMappingInclude, ProtocolMappingIncludeMapping};
use crate::protocol::ProtocolMapping;

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
        Self {
            package_path,
            name: m.name.value.to_string(),
            class_mappings: m.class_mappings.iter().map(ProtocolClassMapping::from).collect(),
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
            ClassMappingBody::Pure(_) => ProtocolClassMapping::PureInstance(header),
            ClassMappingBody::Operation(body) => {
                if body.validation_function.is_some() {
                    ProtocolClassMapping::MergeOperation(header)
                } else {
                    ProtocolClassMapping::Operation(header)
                }
            }
            ClassMappingBody::AggregationAware(_) => ProtocolClassMapping::AggregationAware(header),
            ClassMappingBody::RelationFunction(_) => ProtocolClassMapping::Relation(header),
            // Enumeration / XStore / Foreign bodies don't route to
            // `classMappings` in Java — `enumerationMappings` and
            // `associationMappings` are sibling lists. We don't yet
            // emit those in c1; treat them as `PureInstance` headers
            // for now so the dispatcher stays exhaustive. c4/c7 will
            // split them out properly.
            ClassMappingBody::Enumeration(_)
            | ClassMappingBody::XStore(_)
            | ClassMappingBody::Foreign(_) => ProtocolClassMapping::PureInstance(header),
        }
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
