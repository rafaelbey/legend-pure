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

//! Element composers for all top-level Pure element types.
//!
//! Each element follows the canonical formatting from the Java
//! `PureGrammarComposer` to ensure roundtrip compatibility.

use legend_pure_parser_ast::annotation::{StereotypePtr, TaggedValue};
use legend_pure_parser_ast::element::{
    AggregationKind, AssociationDef, ClassDef, Constraint, Element, EnumDef, FunctionDef,
    FunctionTest, FunctionTestAssertion, FunctionTestData, FunctionTestDataValue, MeasureDef,
    ProfileDef, Property, QualifiedProperty, UnitDef,
};

use crate::expression::{
    compose_body, compose_element_ptr, compose_expression, compose_package, compose_parameter,
};
use crate::identifier::{escape_pure_string, maybe_quote};
use crate::type_ref::compose_type_reference;
use crate::writer::IndentWriter;

// ---------------------------------------------------------------------------
// Element dispatcher
// ---------------------------------------------------------------------------

/// Composes any element.
pub fn compose_element(w: &mut IndentWriter, elem: &Element) {
    match elem {
        Element::Class(c) => compose_class(w, c),
        Element::Enumeration(e) => compose_enumeration(w, e),
        Element::Function(f) => compose_function(w, f),
        Element::Profile(p) => compose_profile(w, p),
        Element::Association(a) => compose_association(w, a),
        Element::Measure(m) => compose_measure(w, m),
    }
}

// ---------------------------------------------------------------------------
// Annotation helpers
// ---------------------------------------------------------------------------

/// Writes stereotypes inline: `<<profile.stereo, profile2.stereo2>>`.
fn compose_stereotypes_inline(w: &mut IndentWriter, stereotypes: &[StereotypePtr]) {
    if stereotypes.is_empty() {
        return;
    }
    w.write("<<");
    for (i, s) in stereotypes.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_element_ptr(w, &s.profile);
        w.write(".");
        w.write(&maybe_quote(&s.value));
    }
    w.write(">> ");
}

/// Writes tagged values inline: `{profile.tag = 'value', ...}`.
fn compose_tagged_values_inline(w: &mut IndentWriter, tagged_values: &[TaggedValue]) {
    if tagged_values.is_empty() {
        return;
    }
    w.write("{");
    for (i, tv) in tagged_values.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_element_ptr(w, &tv.tag.profile);
        w.write(".");
        w.write(&maybe_quote(&tv.tag.value));
        w.write(" = '");
        w.write(&escape_pure_string(&tv.value));
        w.write("'");
    }
    w.write("} ");
}



/// Writes the fully qualified name: `pkg::name`.
fn compose_qualified_name(
    w: &mut IndentWriter,
    package: Option<&legend_pure_parser_ast::type_ref::Package>,
    name: &str,
) {
    if let Some(pkg) = package {
        compose_package(w, pkg);
        w.write("::");
    }
    w.write(&maybe_quote(name));
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

fn compose_profile(w: &mut IndentWriter, p: &ProfileDef) {
    w.write("Profile ");
    compose_qualified_name(w, p.package.as_ref(), &p.name);
    w.newline();
    w.write_line("{");
    w.push_indent();
    if !p.stereotypes.is_empty() {
        w.write("stereotypes: [");
        for (i, s) in p.stereotypes.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            w.write(&maybe_quote(&s.value));
        }
        w.write_line("];");
    }
    if !p.tags.is_empty() {
        w.write("tags: [");
        for (i, t) in p.tags.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            w.write(&maybe_quote(&t.value));
        }
        w.write_line("];");
    }
    w.pop_indent();
    w.write_line("}");
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

fn compose_enumeration(w: &mut IndentWriter, e: &EnumDef) {
    w.write("Enum ");
    compose_stereotypes_inline(w, &e.stereotypes);
    compose_tagged_values_inline(w, &e.tagged_values);
    compose_qualified_name(w, e.package.as_ref(), &e.name);
    w.newline();
    w.write_line("{");
    w.push_indent();
    for (i, v) in e.values.iter().enumerate() {
        compose_stereotypes_inline(w, &v.stereotypes);
        compose_tagged_values_inline(w, &v.tagged_values);
        w.write(&maybe_quote(&v.name));
        if i < e.values.len() - 1 {
            w.write(",");
        }
        w.newline();
    }
    w.pop_indent();
    w.write_line("}");
}

// ---------------------------------------------------------------------------
// Class
// ---------------------------------------------------------------------------

fn compose_class(w: &mut IndentWriter, c: &ClassDef) {
    w.write("Class ");
    compose_stereotypes_inline(w, &c.stereotypes);
    compose_tagged_values_inline(w, &c.tagged_values);
    compose_qualified_name(w, c.package.as_ref(), &c.name);

    // Type parameters
    if !c.type_parameters.is_empty() {
        w.write("<");
        for (i, tp) in c.type_parameters.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            w.write(&maybe_quote(tp));
        }
        w.write(">");
    }

    // Super types
    if !c.super_types.is_empty() {
        w.write(" extends ");
        for (i, st) in c.super_types.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            compose_type_reference(w, st);
        }
    }
    w.newline();

    // Constraints
    if !c.constraints.is_empty() {
        w.write_line("[");
        w.push_indent();
        for (i, con) in c.constraints.iter().enumerate() {
            compose_constraint(w, con);
            if i < c.constraints.len() - 1 {
                w.write(",");
            }
            w.newline();
        }
        w.pop_indent();
        w.write_line("]");
    }

    // Properties
    w.write_line("{");
    w.push_indent();
    for prop in &c.properties {
        compose_property(w, prop);
    }
    for qprop in &c.qualified_properties {
        compose_qualified_property(w, qprop);
    }
    w.pop_indent();
    w.write_line("}");
}

fn compose_constraint(w: &mut IndentWriter, c: &Constraint) {
    let is_complex = c.enforcement_level.is_some() || c.external_id.is_some() || c.message.is_some();

    if let Some(name) = &c.name {
        if is_complex {
            // Named complex constraint
            w.write(&maybe_quote(name));
            w.newline();
            w.write_line("(");
            w.push_indent();
            if let Some(ext_id) = &c.external_id {
                w.write("~externalId: '");
                w.write(&escape_pure_string(ext_id));
                w.write_line("'");
            }
            w.write("~function: ");
            compose_expression(w, &c.function_definition);
            w.newline();
            if let Some(level) = &c.enforcement_level {
                w.write("~enforcementLevel: ");
                w.write_line(level);
            }
            if let Some(msg) = &c.message {
                w.write("~message: ");
                compose_expression(w, msg);
                w.newline();
            }
            w.pop_indent();
            w.write(")");
        } else {
            // Named simple constraint: `name: expr`
            w.write(&maybe_quote(name));
            w.write(": ");
            compose_expression(w, &c.function_definition);
        }
    } else {
        // Unnamed constraint: just the expression
        compose_expression(w, &c.function_definition);
    }
}

/// Composes a class property with annotations, aggregation, type, and default value.
fn compose_property(w: &mut IndentWriter, p: &Property) {
    compose_stereotypes_inline(w, &p.stereotypes);
    compose_tagged_values_inline(w, &p.tagged_values);

    // Aggregation kind
    if let Some(agg) = &p.aggregation {
        w.write(match agg {
            AggregationKind::None => "(none) ",
            AggregationKind::Shared => "(shared) ",
            AggregationKind::Composite => "(composite) ",
        });
    }

    w.write(&maybe_quote(&p.name));
    w.write(": ");
    compose_type_reference(w, &p.type_ref);
    w.write(&p.multiplicity.to_string());

    // Default value
    if let Some(dv) = &p.default_value {
        w.write(" = ");
        compose_expression(w, dv);
    }

    w.write_line(";");
}

fn compose_qualified_property(w: &mut IndentWriter, qp: &QualifiedProperty) {
    compose_stereotypes_inline(w, &qp.stereotypes);
    compose_tagged_values_inline(w, &qp.tagged_values);
    w.write(&maybe_quote(&qp.name));

    // Parameters
    w.write("(");
    for (i, p) in qp.parameters.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_parameter(w, p);
    }
    w.write(") ");

    // Body
    if qp.body.len() == 1 {
        w.write("{");
        compose_expression(w, &qp.body[0]);
        w.write("}");
    } else if qp.body.len() > 1 {
        w.write_line("{");
        compose_body(w, &qp.body);
        w.write("}");
    }

    // Return type
    w.write(": ");
    compose_type_reference(w, &qp.return_type);
    w.write(&qp.return_multiplicity.to_string());
    w.write_line(";");
}

// ---------------------------------------------------------------------------
// Association
// ---------------------------------------------------------------------------

fn compose_association(w: &mut IndentWriter, a: &AssociationDef) {
    w.write("Association ");
    compose_stereotypes_inline(w, &a.stereotypes);
    compose_tagged_values_inline(w, &a.tagged_values);
    compose_qualified_name(w, a.package.as_ref(), &a.name);
    w.newline();
    w.write_line("{");
    w.push_indent();
    for prop in &a.properties {
        compose_property(w, prop);
    }
    for qprop in &a.qualified_properties {
        compose_qualified_property(w, qprop);
    }
    w.pop_indent();
    w.write_line("}");
}

// ---------------------------------------------------------------------------
// Measure
// ---------------------------------------------------------------------------

fn compose_measure(w: &mut IndentWriter, m: &MeasureDef) {
    w.write("Measure ");
    compose_qualified_name(w, m.package.as_ref(), &m.name);
    w.newline();
    w.write_line("{");
    w.push_indent();

    // Canonical unit (marked with *)
    if let Some(cu) = &m.canonical_unit {
        w.write("*");
        compose_unit(w, cu);
    }

    // Non-canonical units
    for u in &m.non_canonical_units {
        compose_unit(w, u);
    }

    w.pop_indent();
    w.write_line("}");
}

fn compose_unit(w: &mut IndentWriter, u: &UnitDef) {
    w.write(&maybe_quote(&u.name));
    if let (Some(param), Some(body)) = (&u.conversion_param, &u.conversion_body) {
        w.write(": ");
        w.write(&maybe_quote(param));
        w.write(" -> ");
        compose_expression(w, body);
    }
    w.write_line(";");
}

// ---------------------------------------------------------------------------
// Function
// ---------------------------------------------------------------------------

fn compose_function(w: &mut IndentWriter, f: &FunctionDef) {
    w.write("function ");
    compose_stereotypes_inline(w, &f.stereotypes);
    compose_tagged_values_inline(w, &f.tagged_values);
    compose_qualified_name(w, f.package.as_ref(), &f.name);

    // Parameters
    w.write("(");
    for (i, p) in f.parameters.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        compose_parameter(w, p);
    }
    w.write("): ");

    // Return type
    compose_type_reference(w, &f.return_type);
    w.write(&f.return_multiplicity.to_string());
    w.newline();

    // Body
    w.write_line("{");
    w.push_indent();
    compose_body(w, &f.body);
    w.pop_indent();
    w.write_line("}");

    // Function tests
    for test in &f.tests {
        compose_function_test(w, test);
    }
}

fn compose_function_test(w: &mut IndentWriter, test: &FunctionTest) {
    w.write_line("{");
    w.push_indent();

    if let Some(name) = &test.name {
        w.write_line(name);
        w.write_line("(");
        w.push_indent();

        // Data bindings
        for data in &test.data {
            compose_function_test_data(w, data);
            w.newline();
        }

        // Assertions
        for assertion in &test.assertions {
            compose_function_test_assertion(w, assertion);
            w.newline();
        }

        w.pop_indent();
        w.write_line(")");
    } else {
        // Data bindings
        for data in &test.data {
            compose_function_test_data(w, data);
            w.newline();
        }

        // Assertions
        for assertion in &test.assertions {
            compose_function_test_assertion(w, assertion);
            w.newline();
        }
    }

    w.pop_indent();
    w.write_line("}");
}

fn compose_function_test_data(w: &mut IndentWriter, data: &FunctionTestData) {
    compose_package(w, &data.store);
    w.write(": ");
    match &data.data {
        FunctionTestDataValue::Inline(content) => {
            if let Some(fmt) = &data.format {
                w.write("(");
                w.write(fmt);
                w.write(") '");
                w.write(content);
                w.write("'");
            } else {
                w.write("'");
                w.write(content);
                w.write("'");
            }
        }
        FunctionTestDataValue::Reference(path) => {
            compose_package(w, path);
        }
    }
    w.write(";");
}

fn compose_function_test_assertion(w: &mut IndentWriter, assertion: &FunctionTestAssertion) {
    w.write(&maybe_quote(&assertion.name));
    w.write(" | ");
    compose_expression(w, &assertion.invocation);
    w.write(" => ");
    if let Some(fmt) = &assertion.expected_format {
        w.write("(");
        w.write(fmt);
        w.write(") ");
    }
    compose_expression(w, &assertion.expected);
    w.write(";");
}
