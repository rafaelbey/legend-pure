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

//! Element nodes (functions; classes and enumerations follow) into model
//! elements.

use std::sync::Arc;

use legend_pure_metamodel::ids::ElementId;
use legend_pure_metamodel::model::Element;
use legend_pure_metamodel::nodes::function::Function;
use legend_pure_metamodel::types::{Multiplicity, Parameter, TypeExpr};
use smol_str::SmolStr;

use super::{LowerError, Lowering, Node};
use crate::value::Value;

impl Lowering<'_> {
    /// The model element a value stands for: an element as is; a node
    /// with a user path through the model, then the pending elements,
    /// then lowered by its classifier.
    ///
    /// # Errors
    /// The node is not a packageable element, or its kind is not lowered
    /// yet (`Unsupported("lowering <classifier>")`).
    pub fn element_for(&mut self, value: &Value) -> Result<ElementId, LowerError> {
        let node = match value {
            Value::Element(id) => return Ok(*id),
            Value::Object(h) => Node::new(h.clone()),
            other => {
                return Err(LowerError::Read(format!(
                    "an element should be a node, got {}",
                    other.type_name()
                )));
            }
        };
        let Some(path) = node.user_path() else {
            return Err(Self::unsupported(&node, "as an element (no user path)"));
        };
        let segments: Vec<SmolStr> = path.split("::").map(SmolStr::new).collect();
        if let Some(id) = self.model.resolve_by_fqn(&segments) {
            return Ok(id);
        }
        if let Some(id) = self
            .ext
            .lookup(&path)
            .or_else(|| self.ext.lookup_node(node.key()))
        {
            return Ok(id);
        }
        if self.is(&node, self.ids.native_function)
            || self.is(&node, self.ids.concrete_function_definition)
        {
            return self.lower_function(&node);
        }
        if self.is(&node, self.ids.primitive_type) {
            return Err(Self::unsupported(&node, "outside the bootstrap"));
        }
        Err(Self::unsupported(&node, "as an element"))
    }

    /// A `NativeFunction` or `ConcreteFunctionDefinition` node into a
    /// [`Function`] element whose node name is the Java mangled name,
    /// verbatim: dispatch reads it as the registry key. A Pure body is
    /// lowered with it, transitively.
    ///
    /// # Errors
    /// The node has no path or name, or its body cannot be lowered.
    pub fn lower_function(&mut self, node: &Node) -> Result<ElementId, LowerError> {
        if let Some(id) = self.ext.lookup_node(node.key()) {
            return Ok(id);
        }
        let path = node
            .user_path()
            .ok_or_else(|| Self::read_error(node, "missing user path"))?;
        let mangled = node
            .name()
            .ok_or_else(|| Self::read_error(node, "missing name"))?;
        let is_native = self.is(node, self.ids.native_function);
        let id = self
            .ext
            .reserve(node.key(), &path, mangled.clone(), node.span());
        let function_name = node
            .string("functionName")
            .unwrap_or_else(|| SmolStr::new(mangled.split('_').next().unwrap_or(mangled.as_str())));
        let (parameters, return_type, return_multiplicity) = self.function_signature(node);
        // The id is reserved, so a body that calls this function (directly
        // or through a sibling) resolves to it while it is being lowered.
        let mut body: Vec<legend_pure_metamodel::types::ValueSpec> = Vec::new();
        if !is_native {
            for e in node.nodes("expressionSequence") {
                body.push(self.lower_expr(&e)?);
            }
        }
        self.ext.fill(
            id,
            Element::Function(Function {
                function_name,
                is_native,
                parameters: parameters.into(),
                return_type,
                return_multiplicity,
                body: body.into(),
                stereotypes: Vec::new(),
                tagged_values: Vec::new(),
            }),
        );
        Ok(id)
    }

    /// Parameters, return type and return multiplicity from the function
    /// type a `FunctionDefinition` / `LambdaFunction` carries at
    /// `classifierGenericType.typeArguments[0].rawType`. A signature the
    /// node does not carry (some natives) is empty with a `T[*]` return:
    /// dispatch needs only the node name.
    pub(crate) fn function_signature(
        &mut self,
        node: &Node,
    ) -> (Vec<Parameter>, TypeExpr, Multiplicity) {
        let function_type = self.function_type_node(node);
        let mut parameters = Vec::new();
        if let Some(ft) = &function_type {
            for p in ft.nodes("parameters") {
                let name = p.string("name").unwrap_or_default();
                let type_expr = self.parameter_type(&p);
                let multiplicity = self.parameter_multiplicity(&p);
                parameters.push(Parameter {
                    name,
                    type_expr,
                    multiplicity,
                    source_info: p.span(),
                });
            }
        }
        let return_type = function_type
            .as_ref()
            .and_then(|ft| ft.one("returnType"))
            .and_then(|rt| self.lower_generic_type(&rt).ok())
            .unwrap_or_else(|| TypeExpr::Generic(SmolStr::new("T")));
        let return_multiplicity = function_type
            .as_ref()
            .and_then(|ft| ft.one("returnMultiplicity"))
            .and_then(|m| self.lower_multiplicity(&m).ok())
            .unwrap_or(Multiplicity::ZeroOrMany);
        (parameters, return_type, return_multiplicity)
    }

    pub(crate) fn function_type_node(&self, node: &Node) -> Option<Node> {
        let ft = node
            .node("classifierGenericType")?
            .nodes("typeArguments")
            .into_iter()
            .next()?
            .node("rawType")?;
        self.is(&ft, self.ids.function_type).then_some(ft)
    }
}

#[allow(dead_code)]
fn _assert_arc_from_vec(v: Vec<Parameter>) -> Arc<[Parameter]> {
    v.into()
}
