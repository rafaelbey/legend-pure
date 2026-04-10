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

//! # Legend Pure Parser — JNI Bridge
//!
//! Exposes the Rust parser to Java via JNI. This is the only crate that uses `unsafe`
//! (required for FFI). It initializes the tracing subscriber and routes parse calls
//! from Java to the Rust parser pipeline.

#![deny(missing_docs)]
// Note: unsafe is required for JNI FFI — not forbidden here
