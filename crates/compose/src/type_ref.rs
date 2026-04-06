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

//! Type reference composer — renders `TypeReference` as Pure grammar text.
//!
//! Handles generic type arguments (`<T>`) and type variable values (`(200)`).

use legend_pure_parser_ast::type_ref::{TypeReference, TypeVariableValue};

use crate::expression::compose_package;
use crate::identifier::escape_pure_string;
use crate::writer::IndentWriter;

/// Composes a type reference as `Path<TypeArgs>(TypeVarValues)`.
pub fn compose_type_reference(w: &mut IndentWriter, tr: &TypeReference) {
    compose_package(w, &tr.path);

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


