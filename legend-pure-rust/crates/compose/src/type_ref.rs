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

//! Type reference and type spec composer — renders type information as Pure grammar text.
//!
//! Handles:
//! - Type references: `String`, `Map<K, V>`, `VARCHAR(200)`
//! - Unit references: `NewMeasure~UnitOne`
//! - Relation types: `(a:Integer, b:String)`
//! - Type specs (any of the above)

use legend_pure_parser_ast::type_ref::{
    RelationType, TypeReference, TypeSpec, TypeVariableValue, UnitReference,
};

use crate::expression::compose_package;
use crate::identifier::{escape_pure_string, maybe_quote};
use crate::writer::IndentWriter;

/// Composes a type reference as `pkg::Name<TypeArgs>(TypeVarValues)`.
pub fn compose_type_reference(w: &mut IndentWriter, tr: &TypeReference) {
    if let Some(pkg) = &tr.package {
        compose_package(w, pkg);
        w.write("::");
    }
    w.write(&maybe_quote(&tr.name));

    if !tr.type_arguments.is_empty() {
        w.write("<");
        for (i, arg) in tr.type_arguments.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            compose_type_reference(w, arg);
        }
        w.write(">");
    }

    if !tr.type_variable_values.is_empty() {
        w.write("(");
        for (i, val) in tr.type_variable_values.iter().enumerate() {
            if i > 0 {
                w.write(", ");
            }
            match val {
                TypeVariableValue::Integer(v, _) => w.write(&v.to_string()),
                TypeVariableValue::String(v, _) => {
                    w.write("'");
                    w.write(&escape_pure_string(v));
                    w.write("'");
                }
            }
        }
        w.write(")");
    }
}

/// Composes a unit reference as `Measure~Unit`.
pub fn compose_unit_reference(w: &mut IndentWriter, ur: &UnitReference) {
    compose_type_reference(w, &ur.measure);
    w.write("~");
    w.write(&maybe_quote(&ur.unit));
}

/// Composes a relation type as `(a:Integer, b:String[1])`.
pub fn compose_relation_type(w: &mut IndentWriter, rt: &RelationType) {
    w.write("(");
    for (i, col) in rt.columns.iter().enumerate() {
        if i > 0 {
            w.write(", ");
        }
        w.write(&maybe_quote(&col.name));
        w.write(":");
        compose_type_reference(w, &col.type_ref);
        if let Some(mult) = &col.multiplicity {
            w.write(&mult.to_string());
        }
    }
    w.write(")");
}

/// Composes a type spec (type, unit reference, or relation type).
pub fn compose_type_spec(w: &mut IndentWriter, ts: &TypeSpec) {
    match ts {
        TypeSpec::Type(tr) => compose_type_reference(w, tr),
        TypeSpec::Unit(ur) => compose_unit_reference(w, ur),
        TypeSpec::Relation(rt) => compose_relation_type(w, rt),
    }
}
