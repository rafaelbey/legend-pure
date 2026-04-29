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

use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;

pub struct JniContext {
    model: *mut PureModel,
    registry: *mut NativeRegistry,
    evaluator: *mut Evaluator<'static>,
}

impl JniContext {
    pub fn new(model: PureModel) -> Self {
        let model_ptr = Box::into_raw(Box::new(model));
        let registry_ptr = Box::into_raw(Box::new(NativeRegistry::standard()));
        let evaluator = Evaluator::new(unsafe { &*model_ptr }, unsafe { &*registry_ptr });
        let evaluator_ptr = Box::into_raw(Box::new(evaluator)) as *mut Evaluator<'static>;

        Self {
            model: model_ptr,
            registry: registry_ptr,
            evaluator: evaluator_ptr,
        }
    }

    pub fn evaluate(&mut self, function_path: &str, args: &[Value]) -> Result<Value, String> {
        let evaluator = unsafe { &mut *self.evaluator };
        let segments: Vec<smol_str::SmolStr> = function_path
            .split("::")
            .map(smol_str::SmolStr::new)
            .collect();
        let element_id = evaluator
            .model()
            .resolve_function_by_path(&segments)
            .or_else(|| evaluator.model().resolve_by_path(&segments))
            .ok_or_else(|| format!("Function not found: {}", function_path))?;

        evaluator
            .apply_callable(&Value::Element(element_id), args)
            .map_err(|e| e.to_string())
    }

    pub fn get_property(
        &mut self,
        complex_ptr: i64,
        property_name: &str,
        _args: &[Value],
    ) -> Result<Value, String> {
        let key = slotmap::KeyData::from_ffi(complex_ptr as u64);
        let obj_id = legend_pure_runtime::heap::ObjectId::from(key);
        let evaluator = unsafe { &mut *self.evaluator };

        let values = evaluator
            .heap()
            .get_property_values(obj_id, property_name)
            .map_err(|e| e.to_string())?;
        if values.is_empty() {
            return Err(format!("Property '{}' not found", property_name));
        }
        let collected: Vec<Value> = values.iter().cloned().collect();
        Ok(Value::from_vec(collected))
    }

    pub fn get_classifier(&mut self, complex_ptr: i64) -> Result<String, String> {
        let key = slotmap::KeyData::from_ffi(complex_ptr as u64);
        let obj_id = legend_pure_runtime::heap::ObjectId::from(key);
        let evaluator = unsafe { &mut *self.evaluator };

        let classifier_path = evaluator
            .heap()
            .classifier(obj_id)
            .map_err(|e| e.to_string())?;
        Ok(classifier_path.to_string())
    }

    pub fn object_for_element(
        &self,
        element_id: legend_pure_parser_pure::ids::ElementId,
    ) -> Option<legend_pure_runtime::heap::ObjectId> {
        let evaluator = unsafe { &*self.evaluator };
        evaluator.heap().object_for_element(element_id)
    }

    pub fn element_for_object(
        &self,
        obj_id: legend_pure_runtime::heap::ObjectId,
    ) -> Option<legend_pure_parser_pure::ids::ElementId> {
        let evaluator = unsafe { &*self.evaluator };
        evaluator.heap().element_for_object(obj_id)
    }
}

impl Drop for JniContext {
    fn drop(&mut self) {
        unsafe {
            drop(Box::from_raw(self.evaluator));
            drop(Box::from_raw(self.registry));
            drop(Box::from_raw(self.model));
        }
    }
}
