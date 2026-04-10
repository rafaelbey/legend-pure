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

//! Pure Runtime — interpreter, runtime heap, and execution engine.
//!
//! This crate implements the execution layer for Legend Pure. It evaluates
//! compiled Pure expressions against a `PureModel` (from the `pure` crate),
//! producing [`Value`](value::Value)s.
//!
//! # Architecture
//!
//! The runtime is organized in four layers:
//!
//! 1. **Model Arena** — Immutable compiled model (`PureModel` from the `pure` crate).
//!    Contains class definitions, function definitions, type hierarchy. Shared
//!    across threads via `Arc`.
//!
//! 2. **Runtime Heap** — Mutable storage for runtime object instances.
//!    Each object is identified by an [`ObjectId`](heap::ObjectId) and stores
//!    properties as `im_rc::Vector<Value>`. Supports `mutateAdd` for in-place mutation.
//!
//! 3. **Value Stack** — Scoped variable bindings for expression evaluation.
//!    Implements a push/pop scope chain for `let` bindings, function parameters,
//!    and `$this` references.
//!
//! 4. **Extension Points** — Traits for compiled function dispatch
//!    (`CompiledFunction`), struct-based class access
//!    ([`TypedObject`](heap::TypedObject)), and external runtime environments
//!    (`RuntimeEnv`).
//!
//! # Example (future)
//!
//! ```ignore
//! use legend_pure_runtime::{Executor, Value};
//!
//! let model = load_and_compile("my/pure/sources/");
//! let mut executor = Executor::new(Arc::new(model));
//! let result = executor.evaluate_function("my::package::hello", &[]);
//! assert_eq!(result.unwrap(), Value::String("Hello, world!".into()));
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod context;
pub mod date;
pub mod error;
pub mod eval;
pub mod heap;
pub mod native;
pub mod value;
