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

//! Compiled Function node.

use crate::annotations::{StereotypeRef, TaggedValueRef};
use crate::types::{Expression, Multiplicity, Parameter, TypeExpr};
use smol_str::SmolStr;

/// A compiled top-level function definition.
#[derive(Debug, Clone, PartialEq)]
pub struct Function {
    /// The function's simple name (e.g., `"plus"`, `"toOne"`).
    ///
    /// `ElementNode.name` holds the mangled signature
    /// (e.g., `"plus_Integer_MANY__Integer_1_"`), which is the unique
    /// element identity. This field preserves the simple name for
    /// unqualified resolution in expression bodies.
    pub function_name: SmolStr,
    /// Parameters.
    pub parameters: Vec<Parameter>,
    /// Return type.
    pub return_type: TypeExpr,
    /// Return multiplicity.
    pub return_multiplicity: Multiplicity,
    /// Body expressions.
    pub body: Vec<Expression>,
    /// Stereotypes.
    pub stereotypes: Vec<StereotypeRef>,
    /// Tagged values.
    pub tagged_values: Vec<TaggedValueRef>,
}
