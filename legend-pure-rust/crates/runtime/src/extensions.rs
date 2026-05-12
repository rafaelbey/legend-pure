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

//! Per-Evaluator typed extension state.
//!
//! Companion to [`crate::native::RuntimeExtension`]. Where `RuntimeExtension`
//! contributes *natives* (immutable, shared across evaluators), this store
//! contributes *state* (per-evaluator, dropped with the evaluator).
//!
//! Each [`crate::eval::Evaluator`] owns one [`ExtensionStateStore`]. Natives
//! reach it via [`crate::native::EvalContextTrait::extensions`] and call
//! [`ExtensionStateStore::get_or_init`] keyed by their own state type:
//!
//! ```ignore
//! let state = ctx.extensions().get_or_init::<DuckDBState, _>(DuckDBState::new)?;
//! state.with_conn(|c| c.execute_batch(sql))?;
//! ```
//!
//! Two evaluators get fully independent state — supports concurrent
//! execution and per-test isolation without any process-wide singletons.

use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell};
use std::collections::HashMap;

use crate::error::PureException;

/// Per-Evaluator typed extension state.
///
/// State is keyed by `TypeId`. Each extension stashes one value per type;
/// repeated `get_or_init` calls with the same type return the existing value
/// without re-initialising.
///
/// The store uses interior mutability so natives can lazily install state
/// through an `&self` reference (matching the rest of the read-mostly
/// `EvalContextTrait` surface). The returned [`Ref<'_, T>`] borrows the store
/// for the duration of the access — drop it before re-borrowing.
pub struct ExtensionStateStore {
    map: RefCell<HashMap<TypeId, Box<dyn Any>>>,
}

impl ExtensionStateStore {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self {
            map: RefCell::new(HashMap::new()),
        }
    }

    /// Get the stored value of type `T`, or initialise it on first access.
    ///
    /// `init` runs exactly once per evaluator per `T`. If it returns
    /// `Err`, no entry is inserted and the error propagates to the caller —
    /// the next call will try again.
    ///
    /// # Errors
    /// Propagates any [`PureException`] returned by `init`.
    pub fn get_or_init<T, F>(&self, init: F) -> Result<Ref<'_, T>, PureException>
    where
        T: 'static,
        F: FnOnce() -> Result<T, PureException>,
    {
        let type_id = TypeId::of::<T>();
        if !self.map.borrow().contains_key(&type_id) {
            let value = init()?;
            self.map.borrow_mut().insert(type_id, Box::new(value));
        }
        Ok(Ref::map(self.map.borrow(), |m| {
            m.get(&type_id)
                .and_then(|b| b.downcast_ref::<T>())
                .unwrap_or_else(|| {
                    unreachable!("ExtensionStateStore: entry just inserted under TypeId<T>")
                })
        }))
    }

    /// Whether a value of type `T` has been initialised.
    #[must_use]
    pub fn contains<T: 'static>(&self) -> bool {
        self.map.borrow().contains_key(&TypeId::of::<T>())
    }

    /// The number of distinct extension states held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.borrow().len()
    }

    /// Whether the store holds no extension states.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.borrow().is_empty()
    }
}

impl Default for ExtensionStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ExtensionStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionStateStore")
            .field("count", &self.map.borrow().len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::PureRuntimeError;

    struct Counter {
        n: RefCell<u32>,
    }

    #[test]
    fn get_or_init_runs_once_per_type() {
        let store = ExtensionStateStore::new();
        let calls = RefCell::new(0u32);

        let init = || {
            *calls.borrow_mut() += 1;
            Ok::<_, PureException>(Counter { n: RefCell::new(0) })
        };

        let c1 = store.get_or_init::<Counter, _>(init).expect("first init");
        *c1.n.borrow_mut() = 7;
        drop(c1);

        let c2 = store
            .get_or_init::<Counter, _>(|| {
                *calls.borrow_mut() += 1;
                Ok::<_, PureException>(Counter { n: RefCell::new(0) })
            })
            .expect("second access");
        assert_eq!(*c2.n.borrow(), 7);

        assert_eq!(*calls.borrow(), 1);
    }

    #[test]
    fn distinct_types_get_distinct_state() {
        struct A;
        struct B;
        let store = ExtensionStateStore::new();
        store
            .get_or_init::<A, _>(|| Ok::<_, PureException>(A))
            .expect("A");
        store
            .get_or_init::<B, _>(|| Ok::<_, PureException>(B))
            .expect("B");
        assert_eq!(store.len(), 2);
        assert!(store.contains::<A>());
        assert!(store.contains::<B>());
    }

    #[test]
    fn init_error_leaves_no_entry() {
        let store = ExtensionStateStore::new();
        let err = store.get_or_init::<Counter, _>(|| {
            Err(PureRuntimeError::EvaluationError("nope".into()).into())
        });
        assert!(err.is_err());
        assert!(!store.contains::<Counter>());
    }
}
