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

//! Runtime Heap — mutable storage for Pure object instances.
//!
//! Objects created by `^Class(...)` or `new` expressions live on the
//! [`RuntimeHeap`]. Each object is identified by an [`ObjectId`]
//! (a generational index from `slotmap`), ensuring safe identity-preserving
//! semantics for `mutateAdd`.
//!
//! # Object Representations
//!
//! The heap supports two representations for objects:
//!
//! - **Dynamic** (`RuntimeObject`): Properties stored in a
//!   `HashMap<SmolStr, im_rc::Vector<Value>>`. Used by the interpreter for
//!   all classes. Supports arbitrary property access by name.
//!
//! - **Typed** (`Box<dyn TypedObject>`): Generated Rust structs with direct
//!   field access. Created by compiled code for hot-path classes. Also
//!   supports dynamic property access via the [`TypedObject`] trait.
//!
//! Both representations produce the same `Value::Object(ObjectId)` — callers
//! never need to know which representation is used.

use std::collections::HashMap;
use std::fmt;

use im_rc::Vector as PVector;
use slotmap::{SlotMap, new_key_type};
use smol_str::SmolStr;

use crate::error::PureRuntimeError;
use crate::value::Value;

// ---------------------------------------------------------------------------
// ObjectId — generational handle
// ---------------------------------------------------------------------------

new_key_type! {
    /// A generational index into the [`RuntimeHeap`].
    ///
    /// `ObjectId` is `Copy`, `Eq`, `Hash` — safe to store in collections
    /// and use as map keys. The generational design means stale IDs are
    /// detected at runtime (slotmap returns `None`).
    pub struct ObjectId;
}

impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

// ---------------------------------------------------------------------------
// RuntimeObject — dynamic property storage
// ---------------------------------------------------------------------------

/// An object instance with dynamic property storage.
///
/// Properties are stored as `HashMap<SmolStr, PVector<Value>>` where:
/// - The key is the property name
/// - The value is a persistent vector (supports multiplicity `[*]`)
/// - Single-valued properties `[1]` have a vector of length 1
/// - Optional properties `[0..1]` have a vector of length 0 or 1
pub struct RuntimeObject {
    /// The class that this object is an instance of.
    pub classifier: SmolStr,
    /// Property values, keyed by property name.
    pub properties: HashMap<SmolStr, PVector<Value>>,
}

impl RuntimeObject {
    /// Create a new object with no properties set.
    pub fn new(classifier: impl Into<SmolStr>) -> Self {
        Self {
            classifier: classifier.into(),
            properties: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// TypedObject — trait for generated struct access (hybrid compilation)
// ---------------------------------------------------------------------------

/// Trait implemented by generated Rust structs for Pure classes.
///
/// This bridges the gap between statically-typed compiled code (direct field
/// access) and dynamically-typed interpreted code (property name lookup).
///
/// # Example (generated code)
///
/// ```ignore
/// pub struct Trade {
///     pub ticker: SmolStr,
///     pub price: f64,
///     pub quantity: i64,
/// }
///
/// impl TypedObject for Trade {
///     fn classifier_path(&self) -> &str { "my::trading::Trade" }
///
///     fn get_property(&self, name: &str) -> Option<Value> {
///         match name {
///             "ticker"   => Some(Value::String(self.ticker.clone())),
///             "price"    => Some(Value::Float(self.price)),
///             "quantity" => Some(Value::Integer(self.quantity)),
///             _ => None,
///         }
///     }
///     // ...
/// }
/// ```
pub trait TypedObject: Send + Sync + 'static {
    /// The Pure class path (e.g., `"my::trading::Trade"`).
    fn classifier_path(&self) -> &str;

    /// Dynamic property access by name.
    /// Returns `None` if the property name is not recognized.
    fn get_property(&self, name: &str) -> Option<Value>;

    /// Dynamic property mutation (for `mutateAdd` support).
    ///
    /// # Errors
    /// Returns an error if the property name is not recognized or the value type is wrong.
    fn set_property(&mut self, name: &str, value: Value) -> Result<(), PureRuntimeError>;

    /// Downcast to `Any` for type-safe access from compiled code.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Mutable downcast for mutation from compiled code.
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

// ---------------------------------------------------------------------------
// HeapEntry — unified storage for dynamic + typed objects
// ---------------------------------------------------------------------------

/// The storage representation for a single heap object.
///
/// The interpreter doesn't need to know which variant is active —
/// all access goes through `RuntimeHeap` methods which handle both.
pub enum HeapEntry {
    /// Dynamic object — interpreter-created, HashMap-based property storage.
    Dynamic(RuntimeObject),
    /// Typed object — generated struct with direct field access.
    Typed(Box<dyn TypedObject>),
}

// ---------------------------------------------------------------------------
// RuntimeHeap
// ---------------------------------------------------------------------------

/// Mutable runtime storage for Pure object instances.
///
/// Objects are allocated with [`alloc_dynamic`](Self::alloc_dynamic) (interpreter
/// path) or [`alloc_typed`](Self::alloc_typed) (compiled code path) and
/// accessed via [`ObjectId`] handles.
///
/// # Thread Safety
///
/// `RuntimeHeap` is **not** `Send` or `Sync` — it contains `im_rc` types
/// (which use `Rc`). Each thread/executor gets its own heap. The compiled
/// `PureModel` (which contains no `Rc` types) is shared across threads
/// via `Arc`.
pub struct RuntimeHeap {
    objects: SlotMap<ObjectId, HeapEntry>,
}

impl RuntimeHeap {
    /// Create an empty heap.
    #[must_use]
    pub fn new() -> Self {
        Self {
            objects: SlotMap::with_key(),
        }
    }

    /// Create an empty heap with pre-allocated capacity.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            objects: SlotMap::with_capacity_and_key(capacity),
        }
    }

    // -- Allocation --

    /// Allocate a dynamic object (interpreter path).
    pub fn alloc_dynamic(&mut self, classifier: impl Into<SmolStr>) -> ObjectId {
        self.objects
            .insert(HeapEntry::Dynamic(RuntimeObject::new(classifier)))
    }

    /// Allocate a typed object (compiled code path).
    pub fn alloc_typed(&mut self, obj: Box<dyn TypedObject>) -> ObjectId {
        self.objects.insert(HeapEntry::Typed(obj))
    }

    // -- Property Access (works for both Dynamic and Typed) --

    /// Get a single property value by name.
    ///
    /// For dynamic objects: `HashMap` lookup + first element.
    /// For typed objects: `TypedObject::get_property` (match on name).
    ///
    /// # Errors
    /// Returns `InvalidObjectId` if the ID is stale or invalid.
    /// Returns `PropertyNotFound` if the property does not exist.
    pub fn get_property(&self, id: ObjectId, name: &str) -> Result<Value, PureRuntimeError> {
        let entry = self
            .objects
            .get(id)
            .ok_or(PureRuntimeError::InvalidObjectId(id))?;

        match entry {
            HeapEntry::Dynamic(obj) => obj
                .properties
                .get(name)
                .and_then(|v| v.head())
                .cloned()
                .ok_or_else(|| PureRuntimeError::PropertyNotFound {
                    property: name.into(),
                    classifier: obj.classifier.clone(),
                }),
            HeapEntry::Typed(obj) => {
                obj.get_property(name)
                    .ok_or_else(|| PureRuntimeError::PropertyNotFound {
                        property: name.into(),
                        classifier: obj.classifier_path().into(),
                    })
            }
        }
    }

    /// Get all values for a multi-valued property.
    ///
    /// # Errors
    /// Returns `InvalidObjectId` if the ID is stale or invalid.
    pub fn get_property_values(
        &self,
        id: ObjectId,
        name: &str,
    ) -> Result<PVector<Value>, PureRuntimeError> {
        let entry = self
            .objects
            .get(id)
            .ok_or(PureRuntimeError::InvalidObjectId(id))?;

        match entry {
            HeapEntry::Dynamic(obj) => Ok(obj.properties.get(name).cloned().unwrap_or_default()),
            HeapEntry::Typed(obj) => {
                // TypedObject returns a single Value; wrap in a vector
                match obj.get_property(name) {
                    Some(v) => Ok(PVector::unit(v)),
                    None => Ok(PVector::new()),
                }
            }
        }
    }

    /// Get the classifier (class path) of an object.
    ///
    /// # Errors
    /// Returns `InvalidObjectId` if the ID is stale or invalid.
    pub fn classifier(&self, id: ObjectId) -> Result<&str, PureRuntimeError> {
        let entry = self
            .objects
            .get(id)
            .ok_or(PureRuntimeError::InvalidObjectId(id))?;

        match entry {
            HeapEntry::Dynamic(obj) => Ok(&obj.classifier),
            HeapEntry::Typed(obj) => Ok(obj.classifier_path()),
        }
    }

    // -- Mutation --

    /// Add values to a property (in-place mutation).
    ///
    /// This is the core operation for `mutateAdd` — it preserves the
    /// `ObjectId` while modifying the object's property values.
    ///
    /// # Errors
    /// Returns `InvalidObjectId` if the ID is stale or invalid.
    pub fn mutate_add(
        &mut self,
        id: ObjectId,
        property: &str,
        values: &[Value],
    ) -> Result<(), PureRuntimeError> {
        let entry = self
            .objects
            .get_mut(id)
            .ok_or(PureRuntimeError::InvalidObjectId(id))?;

        match entry {
            HeapEntry::Dynamic(obj) => {
                let prop_vec = obj
                    .properties
                    .entry(SmolStr::new(property))
                    .or_insert_with(PVector::new);
                for v in values {
                    prop_vec.push_back(v.clone());
                }
                Ok(())
            }
            HeapEntry::Typed(obj) => {
                for v in values {
                    obj.set_property(property, v.clone())?;
                }
                Ok(())
            }
        }
    }

    // -- Typed Access (compiled code fast path) --

    /// Downcast a heap entry to a concrete typed struct.
    ///
    /// This is the fast path for compiled code: direct struct field
    /// access instead of `heap.get_property(id, "price")`.
    ///
    /// # Errors
    /// Returns `InvalidObjectId` if the ID is stale or invalid.
    /// Returns `DowncastFailed` if the object is dynamic or a different concrete type.
    pub fn downcast_ref<T: TypedObject + 'static>(
        &self,
        id: ObjectId,
    ) -> Result<&T, PureRuntimeError> {
        let entry = self
            .objects
            .get(id)
            .ok_or(PureRuntimeError::InvalidObjectId(id))?;

        match entry {
            HeapEntry::Typed(obj) => {
                obj.as_any()
                    .downcast_ref::<T>()
                    .ok_or(PureRuntimeError::DowncastFailed {
                        expected: std::any::type_name::<T>(),
                        actual: obj.classifier_path().into(),
                    })
            }
            HeapEntry::Dynamic(obj) => Err(PureRuntimeError::DowncastFailed {
                expected: std::any::type_name::<T>(),
                actual: obj.classifier.clone().into(),
            }),
        }
    }

    /// Number of objects currently on the heap.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the heap is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl Default for RuntimeHeap {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_and_get_property() {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::Trade");

        heap.mutate_add(id, "price", &[Value::Float(42.0)]).unwrap();
        heap.mutate_add(id, "ticker", &[Value::String("AAPL".into())])
            .unwrap();

        assert_eq!(heap.get_property(id, "price").unwrap(), Value::Float(42.0));
        assert_eq!(
            heap.get_property(id, "ticker").unwrap(),
            Value::String("AAPL".into())
        );
    }

    #[test]
    fn mutate_add_accumulates() {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::Account");

        heap.mutate_add(id, "trades", &[Value::Integer(1)]).unwrap();
        heap.mutate_add(id, "trades", &[Value::Integer(2)]).unwrap();
        heap.mutate_add(id, "trades", &[Value::Integer(3)]).unwrap();

        let values = heap.get_property_values(id, "trades").unwrap();
        assert_eq!(values.len(), 3);
    }

    #[test]
    fn property_not_found() {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::Trade");

        let result = heap.get_property(id, "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn invalid_object_id() {
        let heap = RuntimeHeap::new();
        let fake_id = ObjectId::default();

        let result = heap.get_property(fake_id, "price");
        assert!(result.is_err());
    }

    #[test]
    fn classifier_returns_class_path() {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::trading::Trade");

        assert_eq!(heap.classifier(id).unwrap(), "my::trading::Trade");
    }

    #[test]
    fn object_identity_preserved() {
        let mut heap = RuntimeHeap::new();
        let id = heap.alloc_dynamic("my::Trade");

        // mutateAdd preserves the ObjectId
        heap.mutate_add(id, "price", &[Value::Float(1.0)]).unwrap();
        heap.mutate_add(id, "price", &[Value::Float(2.0)]).unwrap();

        // Same id still works
        assert_eq!(heap.classifier(id).unwrap(), "my::Trade");
    }
}
