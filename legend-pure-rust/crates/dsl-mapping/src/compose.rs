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

//! Composer: emit `###Mapping` source from [`MappingDef`] AST.
//!
//! Round-trip contract: `parse(compose(m)) == m` modulo `source_info`
//! fields. Verified by `tests/compose_smoke.rs`.

use legend_pure_parser_ast::annotation::PackageableElementPtr;
use legend_pure_parser_ast::expression::Expression;

use crate::ast::{
    AggregateSpecification, AggregateView, AggregationAwareClassMappingBody,
    AggregationFunctionSpec, ClassMapping, ClassMappingBody, EnumSourceValue, EnumValueMapping,
    EnumerationClassMappingBody, MappingDef, MappingInclude, NestedClassMapping,
    OperationClassMappingBody, PureClassMappingBody, PurePropertyMapping, StoreSubstitution,
    XStoreClassMappingBody, XStorePropertyMapping,
};

/// Compose a single [`MappingDef`] back to its `Mapping pkg::M ( … )`
/// source. Does not include the `###Mapping` section header — use
/// [`compose_mapping_section`] for that.
#[must_use]
pub fn compose_mapping(m: &MappingDef) -> String {
    let mut out = String::new();
    write_mapping(&mut out, m);
    out
}

/// Compose a list of mappings as a complete `###Mapping` section.
#[must_use]
pub fn compose_mapping_section(mappings: &[&MappingDef]) -> String {
    let mut out = String::from("###Mapping\n");
    for m in mappings {
        write_mapping(&mut out, m);
        out.push('\n');
    }
    out
}

fn write_mapping(out: &mut String, m: &MappingDef) {
    out.push_str("Mapping ");
    write_fqn(out, m.package.as_ref(), m.name.value.as_str());
    out.push_str("\n(\n");
    for inc in &m.includes {
        write_include(out, inc);
    }
    for cm in &m.class_mappings {
        write_class_mapping(out, cm);
    }
    out.push_str(")\n");
}

fn write_include(out: &mut String, inc: &MappingInclude) {
    out.push_str("  include ");
    write_ptr(out, &inc.included);
    if !inc.store_substitutions.is_empty() {
        out.push_str(" [");
        for (i, sub) in inc.store_substitutions.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_substitution(out, sub);
        }
        out.push(']');
    }
    out.push('\n');
}

fn write_substitution(out: &mut String, sub: &StoreSubstitution) {
    write_ptr(out, &sub.source);
    out.push_str(" -> ");
    write_ptr(out, &sub.target);
}

fn write_class_mapping(out: &mut String, cm: &ClassMapping) {
    out.push_str("  ");
    if cm.is_root {
        out.push('*');
    }
    write_ptr(out, &cm.class);
    if let Some(id) = &cm.id {
        out.push('[');
        out.push_str(id.as_str());
        out.push(']');
    }
    if let Some(sup) = &cm.extends {
        out.push_str(" extends [");
        out.push_str(sup.as_str());
        out.push(']');
    }
    out.push_str(" : ");
    match &cm.body {
        ClassMappingBody::Pure(body) => {
            out.push_str("Pure");
            if let Some(name) = &cm.mapping_name {
                out.push(' ');
                out.push_str(name.as_str());
            }
            out.push_str("\n  {\n");
            write_pure_body(out, body);
            out.push_str("  }\n");
        }
        ClassMappingBody::Enumeration(body) => {
            out.push_str("EnumerationMapping");
            if let Some(name) = &cm.mapping_name {
                out.push(' ');
                out.push_str(name.as_str());
            }
            out.push_str("\n  {\n");
            write_enumeration_body(out, body);
            out.push_str("  }\n");
        }
        ClassMappingBody::Operation(body) => {
            out.push_str("Operation");
            if let Some(name) = &cm.mapping_name {
                out.push(' ');
                out.push_str(name.as_str());
            }
            out.push_str("\n  {\n");
            write_operation_body(out, body);
            out.push_str("  }\n");
        }
        ClassMappingBody::AggregationAware(body) => {
            out.push_str("AggregationAware");
            if let Some(name) = &cm.mapping_name {
                out.push(' ');
                out.push_str(name.as_str());
            }
            out.push_str("\n  {\n");
            write_aggregation_aware_body(out, body);
            out.push_str("  }\n");
        }
        ClassMappingBody::XStore(body) => {
            out.push_str("XStore");
            if let Some(name) = &cm.mapping_name {
                out.push(' ');
                out.push_str(name.as_str());
            }
            out.push_str("\n  {\n");
            write_xstore_body(out, body);
            out.push_str("  }\n");
        }
        ClassMappingBody::Foreign(body) => {
            // Foreign body owns its own grammar text; print the
            // `parserName` and optional mapping name, then delegate.
            // The body's `compose` emits the surrounding `{ … }`
            // braces (matches the trait contract).
            out.push_str(body.kind());
            if let Some(name) = &cm.mapping_name {
                out.push(' ');
                out.push_str(name.as_str());
            }
            out.push('\n');
            body.compose(out);
            out.push('\n');
        }
    }
}

fn write_xstore_body(out: &mut String, body: &XStoreClassMappingBody) {
    for (i, pm) in body.property_mappings.iter().enumerate() {
        write_xstore_property_mapping(out, pm);
        if i + 1 < body.property_mappings.len() {
            out.push(',');
        }
        out.push('\n');
    }
}

fn write_xstore_property_mapping(out: &mut String, pm: &XStorePropertyMapping) {
    out.push_str("    ");
    out.push_str(pm.property_name.as_str());
    if let Some(src) = &pm.source_set_impl_id {
        out.push('[');
        out.push_str(src.as_str());
        if let Some(tgt) = &pm.target_set_impl_id {
            out.push_str(", ");
            out.push_str(tgt.as_str());
        }
        out.push(']');
    }
    out.push_str(" : ");
    write_expression(out, &pm.cross_expression);
}

fn write_aggregation_aware_body(out: &mut String, body: &AggregationAwareClassMappingBody) {
    out.push_str("    Views : [\n");
    for (i, v) in body.views.iter().enumerate() {
        write_aggregate_view(out, v);
        if i + 1 < body.views.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("    ],\n");
    write_nested_class_mapping(out, "mainMapping", &body.main_mapping);
}

fn write_aggregate_view(out: &mut String, v: &AggregateView) {
    out.push_str("      (\n");
    write_model_operation(out, &v.model_operation);
    out.push_str(",\n");
    write_nested_class_mapping(out, "aggregateMapping", &v.aggregate_mapping);
    out.push_str("      )");
}

fn write_model_operation(out: &mut String, spec: &AggregateSpecification) {
    out.push_str("        ~modelOperation : {\n");
    out.push_str("          ~canAggregate ");
    out.push_str(if spec.can_aggregate { "true" } else { "false" });
    out.push_str(",\n");
    out.push_str("          ~groupByFunctions (\n");
    for (i, e) in spec.group_by_functions.iter().enumerate() {
        out.push_str("            ");
        write_expression(out, e);
        if i + 1 < spec.group_by_functions.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("          ),\n");
    out.push_str("          ~aggregateValues (\n");
    for (i, av) in spec.aggregate_values.iter().enumerate() {
        write_aggregate_value(out, av);
        if i + 1 < spec.aggregate_values.len() {
            out.push(',');
        }
        out.push('\n');
    }
    out.push_str("          )\n");
    out.push_str("        }");
}

fn write_aggregate_value(out: &mut String, av: &AggregationFunctionSpec) {
    out.push_str("            ( ~mapFn: ");
    write_expression(out, &av.map_fn);
    out.push_str(", ~aggregateFn: ");
    write_expression(out, &av.aggregate_fn);
    out.push_str(" )");
}

fn write_nested_class_mapping(out: &mut String, keyword: &str, n: &NestedClassMapping) {
    out.push_str("    ~");
    out.push_str(keyword);
    out.push_str(" : ");
    out.push_str(n.parser_name.as_str());
    match &n.body {
        ClassMappingBody::Pure(b) => {
            out.push_str("\n    {\n");
            write_pure_body(out, b);
            out.push_str("    }\n");
        }
        ClassMappingBody::Enumeration(b) => {
            out.push_str("\n    {\n");
            write_enumeration_body(out, b);
            out.push_str("    }\n");
        }
        ClassMappingBody::Operation(b) => {
            out.push_str("\n    {\n");
            write_operation_body(out, b);
            out.push_str("    }\n");
        }
        ClassMappingBody::AggregationAware(b) => {
            out.push_str("\n    {\n");
            write_aggregation_aware_body(out, b);
            out.push_str("    }\n");
        }
        ClassMappingBody::XStore(b) => {
            out.push_str("\n    {\n");
            write_xstore_body(out, b);
            out.push_str("    }\n");
        }
        ClassMappingBody::Foreign(b) => {
            // Foreign body owns its `{ … }` braces.
            out.push('\n');
            b.compose(out);
            out.push('\n');
        }
    }
}

fn write_operation_body(out: &mut String, body: &OperationClassMappingBody) {
    out.push_str("    ");
    write_ptr(out, &body.operation);
    out.push('(');
    for (i, p) in body.parameters.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(p.id.as_str());
    }
    out.push_str(")\n");
}

fn write_enumeration_body(out: &mut String, body: &EnumerationClassMappingBody) {
    for (i, vm) in body.value_mappings.iter().enumerate() {
        write_enum_value_mapping(out, vm);
        if i + 1 < body.value_mappings.len() {
            out.push(',');
        }
        out.push('\n');
    }
}

fn write_enum_value_mapping(out: &mut String, vm: &EnumValueMapping) {
    out.push_str("    ");
    out.push_str(vm.enum_value_name.as_str());
    out.push_str(" : ");
    if vm.source_values.len() == 1 {
        write_enum_source_value(out, &vm.source_values[0]);
    } else {
        out.push('[');
        for (i, sv) in vm.source_values.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            write_enum_source_value(out, sv);
        }
        out.push(']');
    }
}

fn write_enum_source_value(out: &mut String, sv: &EnumSourceValue) {
    match sv {
        EnumSourceValue::String { value, .. } => {
            out.push('\'');
            out.push_str(value.as_str());
            out.push('\'');
        }
        EnumSourceValue::Integer { value, .. } => {
            out.push_str(&value.to_string());
        }
        EnumSourceValue::EnumRef {
            enumeration,
            value_name,
            ..
        } => {
            if let Some(pkg) = &enumeration.package {
                use std::fmt::Write as _;
                let _ = write!(out, "{pkg}");
                out.push_str("::");
            }
            out.push_str(enumeration.name.as_str());
            out.push('.');
            out.push_str(value_name.as_str());
        }
    }
}

fn write_pure_body(out: &mut String, body: &PureClassMappingBody) {
    if let Some(src) = &body.src_class {
        out.push_str("    ~src ");
        write_ptr(out, src);
        out.push('\n');
    }
    if let Some(filter) = &body.filter {
        out.push_str("    ~filter ");
        write_expression(out, filter);
        out.push('\n');
    }
    for (i, pm) in body.property_mappings.iter().enumerate() {
        write_property_mapping(out, pm);
        if i + 1 < body.property_mappings.len() {
            out.push(',');
        }
        out.push('\n');
    }
}

fn write_property_mapping(out: &mut String, pm: &PurePropertyMapping) {
    use std::fmt::Write as _;
    out.push_str("    ");
    if pm.local_property.is_some() {
        out.push('+');
    }
    out.push_str(pm.property_name.as_str());
    if pm.explode {
        // Java M3 grammar puts the explode marker after the property
        // header and before the value colon: `name *: transform`.
        // Mutually exclusive with the local-form `+name : Type[m] :`
        // (the parser rejects `+name *: …`).
        out.push_str(" *");
    }
    out.push_str(" : ");
    if let Some(local) = &pm.local_property {
        // Reuse the parser-compose crate's existing
        // type-reference and multiplicity composers so generics
        // (e.g. `Pair<String, Integer>[1]`) survive the round-trip
        // verbatim.
        let mut w = legend_pure_parser_compose::writer::IndentWriter::new();
        legend_pure_parser_compose::type_ref::compose_type_reference(&mut w, &local.type_ref);
        out.push_str(&w.finish());
        let _ = write!(out, "{}", local.multiplicity);
        out.push_str(" : ");
    }
    if let Some(name) = &pm.transformer {
        out.push_str("EnumerationMapping ");
        out.push_str(name.as_str());
        out.push_str(" : ");
    }
    write_expression(out, &pm.transform);
}

fn write_ptr(out: &mut String, ptr: &PackageableElementPtr) {
    use std::fmt::Write as _;
    if let Some(pkg) = &ptr.package {
        let _ = write!(out, "{pkg}");
        out.push_str("::");
    }
    out.push_str(ptr.name.as_str());
}

fn write_fqn(
    out: &mut String,
    package: Option<&legend_pure_parser_ast::type_ref::Package>,
    name: &str,
) {
    use std::fmt::Write as _;
    if let Some(pkg) = package {
        let _ = write!(out, "{pkg}");
        out.push_str("::");
    }
    out.push_str(name);
}

/// Emit a Pure expression via the existing compose crate. Round-trip
/// equality is asserted via re-parse in `tests/compose_smoke.rs`, so
/// any unhandled shape manifests as a failing test rather than silent
/// corruption.
fn write_expression(out: &mut String, expr: &Expression) {
    let mut w = legend_pure_parser_compose::writer::IndentWriter::new();
    legend_pure_parser_compose::expression::compose_expression(&mut w, expr);
    out.push_str(&w.finish());
}
