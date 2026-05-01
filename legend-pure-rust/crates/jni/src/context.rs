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

use std::cell::RefCell;
use std::collections::HashMap;

use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;
use legend_pure_runtime::heap::{HeapEntry, ObjectHandle};
use legend_pure_runtime::native::NativeRegistry;
use legend_pure_runtime::value::Value;
use slotmap::{SlotMap, new_key_type};

new_key_type! {
    /// Stable opaque handle exposed across the JNI boundary.
    ///
    /// `JniHandle::data().as_ffi()` is an `i64` Java holds; the table
    /// maps it back to the strong [`ObjectHandle`] that keeps the
    /// underlying `Rc<RefCell<HeapEntry>>` alive while Java retains
    /// the integer.
    pub struct JniHandle;
}

/// Pure-side bookkeeping for objects exposed to Java.
///
/// `Rc<RefCell<HeapEntry>>` has no integer form and no stable address
/// (the `Rc` may drop), so the JNI cannot hand its raw representation
/// to Java directly. Instead, every Pure→Java emission registers the
/// handle here and returns a [`JniHandle`] (encoded as `i64`); every
/// Java→Pure callback resolves the integer back to the retained
/// `ObjectHandle`.
///
/// Java code drops references explicitly via [`Self::release`]; without
/// that call the entry leaks for the [`JniContext`]'s lifetime — same
/// as the pre-Rc heap behaviour, where slotmap rows lived for the
/// evaluator's lifetime. Wiring Java's `AutoCloseable`/finalizer to
/// `release` is a follow-up.
pub struct JniHandleTable {
    live: SlotMap<JniHandle, ObjectHandle>,
    /// Reverse index keyed by the Rc's pointer address, so emitting the
    /// same metamodel handle twice (e.g. `Class.all().each(c | c.name)`)
    /// returns the same `JniHandle` instead of growing the table.
    by_ptr: HashMap<*const RefCell<HeapEntry>, JniHandle>,
}

impl JniHandleTable {
    pub fn new() -> Self {
        Self {
            live: SlotMap::with_key(),
            by_ptr: HashMap::new(),
        }
    }

    /// Register a handle and return its stable `JniHandle`. Idempotent —
    /// the same `Rc` pointer always maps to the same `JniHandle`.
    pub fn register(&mut self, handle: ObjectHandle) -> JniHandle {
        let key_ptr = std::rc::Rc::as_ptr(&handle);
        if let Some(&existing) = self.by_ptr.get(&key_ptr) {
            return existing;
        }
        let h = self.live.insert(handle);
        self.by_ptr.insert(key_ptr, h);
        h
    }

    /// Look up the [`ObjectHandle`] for a `JniHandle`. Returns `None` if
    /// the entry was released or never registered.
    pub fn lookup(&self, h: JniHandle) -> Option<&ObjectHandle> {
        self.live.get(h)
    }

    /// Drop the strong reference for `h`. After this call, Java should
    /// not reuse the integer; future `lookup` returns `None`.
    pub fn release(&mut self, h: JniHandle) {
        if let Some(handle) = self.live.remove(h) {
            self.by_ptr.remove(&std::rc::Rc::as_ptr(&handle));
        }
    }

    /// Encode a `JniHandle` as the `i64` Java holds.
    pub fn to_i64(h: JniHandle) -> i64 {
        slotmap::Key::data(&h).as_ffi() as i64
    }

    /// Decode an `i64` from Java back into a `JniHandle`.
    pub fn from_i64(v: i64) -> JniHandle {
        let kd = slotmap::KeyData::from_ffi(v as u64);
        JniHandle::from(kd)
    }
}

impl Default for JniHandleTable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct JniContext {
    model: *mut PureModel,
    registry: *mut NativeRegistry,
    evaluator: *mut Evaluator<'static>,
    /// Strong-Rc table keyed by the integer Java holds.
    pub handles: RefCell<JniHandleTable>,
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
            handles: RefCell::new(JniHandleTable::new()),
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

    /// Emit a Pure object to Java. Registers `handle` in the table and
    /// returns the encoded `i64` Java should pass back on subsequent
    /// callbacks.
    pub fn emit_handle(&self, handle: ObjectHandle) -> i64 {
        let jh = self.handles.borrow_mut().register(handle);
        JniHandleTable::to_i64(jh)
    }

    /// Resolve a Java-supplied `i64` back to the strong `ObjectHandle`.
    fn resolve_i64(&self, raw: i64) -> Option<ObjectHandle> {
        let jh = JniHandleTable::from_i64(raw);
        self.handles.borrow().lookup(jh).cloned()
    }

    pub fn get_property(
        &mut self,
        complex_ptr: i64,
        property_name: &str,
        _args: &[Value],
    ) -> Result<Value, String> {
        let handle = self
            .resolve_i64(complex_ptr)
            .ok_or_else(|| format!("Stale or unknown JNI handle: {complex_ptr}"))?;
        let evaluator = unsafe { &mut *self.evaluator };

        let values = evaluator
            .heap()
            .get_property_values(&handle, property_name)
            .map_err(|e| e.to_string())?;
        if values.is_empty() {
            return Err(format!("Property '{}' not found", property_name));
        }
        let collected: Vec<Value> = values.iter().cloned().collect();
        Ok(Value::from_vec(collected))
    }

    pub fn get_classifier(&mut self, complex_ptr: i64) -> Result<String, String> {
        let handle = self
            .resolve_i64(complex_ptr)
            .ok_or_else(|| format!("Stale or unknown JNI handle: {complex_ptr}"))?;
        let evaluator = unsafe { &mut *self.evaluator };

        let classifier = evaluator
            .heap()
            .classifier(&handle)
            .map_err(|e| e.to_string())?;
        Ok(classifier.to_string())
    }

    /// Look up the metamodel handle for a compiled element and emit it
    /// as a JNI integer.
    pub fn object_for_element(
        &self,
        element_id: legend_pure_parser_pure::ids::ElementId,
    ) -> Option<i64> {
        let evaluator = unsafe { &*self.evaluator };
        evaluator
            .heap()
            .object_for_element(element_id)
            .map(|h| self.emit_handle(h))
    }

    /// Reverse-lookup: resolve a Java `i64` back to its compiled
    /// `ElementId`, if any.
    pub fn element_for_object(
        &self,
        complex_ptr: i64,
    ) -> Option<legend_pure_parser_pure::ids::ElementId> {
        let handle = self.resolve_i64(complex_ptr)?;
        legend_pure_runtime::heap::RuntimeHeap::element_for_object(&handle)
    }

    /// Drop the JNI table's strong reference for `complex_ptr`. Idempotent
    /// — re-releasing a handle is a no-op.
    pub fn release(&self, complex_ptr: i64) {
        let jh = JniHandleTable::from_i64(complex_ptr);
        self.handles.borrow_mut().release(jh);
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
