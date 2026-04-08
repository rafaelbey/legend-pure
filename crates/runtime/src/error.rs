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

//! Runtime errors for the Pure interpreter.

use smol_str::SmolStr;
use thiserror::Error;

use crate::heap::ObjectId;
use crate::value::Value;

/// Errors that can occur during Pure expression evaluation.
#[derive(Debug, Error)]
pub enum PureRuntimeError {
    /// Type mismatch: expected one type, got another.
    #[error("Type mismatch: expected {expected}, got {actual}")]
    TypeMismatch {
        /// The type that was expected.
        expected: &'static str,
        /// The type that was actually found.
        actual: String,
    },

    /// Property not found on an object.
    #[error("Property '{property}' not found on {classifier}")]
    PropertyNotFound {
        /// The property name that was not found.
        property: SmolStr,
        /// The classifier of the object.
        classifier: SmolStr,
    },

    /// Variable not found in the current scope.
    #[error("Variable '{0}' not found")]
    VariableNotFound(SmolStr),

    /// Invalid object ID (stale or never existed).
    #[error("Invalid object ID: {0}")]
    InvalidObjectId(ObjectId),

    /// Downcast failed (compiled code expected a specific struct type).
    #[error("Downcast failed: expected {expected}, object is {actual}")]
    DowncastFailed {
        /// The type that was expected.
        expected: &'static str,
        /// The actual object type.
        actual: String,
    },

    /// Function not found.
    #[error("Function not found: {0}")]
    FunctionNotFound(SmolStr),

    /// Multiplicity violation: got wrong number of values.
    #[error("Multiplicity violation: expected {expected}, got {actual} values")]
    MultiplicityViolation {
        /// The expected multiplicity range.
        expected: String,
        /// The number of values that were actually present.
        actual: usize,
    },

    /// Division by zero.
    #[error("Division by zero")]
    DivisionByZero,

    /// An assertion failed (constraint violation or explicit assertion).
    #[error("Assertion failed: {0}")]
    AssertionFailed(String),

    /// A generic evaluation error with a message.
    #[error("{0}")]
    EvaluationError(String),
}

impl PureRuntimeError {
    /// Create a `TypeMismatch` error from an expected type name and actual `Value`.
    #[must_use]
    pub fn type_mismatch(expected: &'static str, actual: &Value) -> Self {
        Self::TypeMismatch {
            expected,
            actual: actual.type_name().to_owned(),
        }
    }
}
