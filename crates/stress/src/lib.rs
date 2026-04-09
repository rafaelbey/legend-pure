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

//! # Legend Pure Parser — Stress Tests
//!
//! Synthetic model generation and pipeline stress testing at scale.
//!
//! This crate generates Pure grammar source for models of varying size
//! (1K → 100K classes) and runs them through the full parse → compile →
//! compose pipeline, measuring per-phase timing and validating model
//! correctness.
//!
//! # Test Tiers
//!
//! | Tier | Classes | Feature Gate |
//! |------|---------|-------------|
//! | 1K hub-spoke | 1,000 | default |
//! | 10K hub-spoke | 10,000 | default |
//! | 10K dense | 10,000 | default |
//! | 100K hub-spoke | 100,000 | `heavy` |
//! | 100K chaotic | 100,000 | `heavy` |
//!
//! # Usage
//!
//! ```bash
//! # Standard suite
//! cargo test -p legend-pure-parser-stress -- --nocapture
//!
//! # Heavy suite (100K models)
//! cargo test -p legend-pure-parser-stress --features heavy -- --nocapture
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(
    clippy::too_many_arguments,
    clippy::struct_excessive_bools,
    clippy::format_push_string,
    clippy::uninlined_format_args,
    clippy::must_use_candidate,
    clippy::cast_possible_truncation,
    clippy::manual_is_multiple_of,
    clippy::unusual_byte_groupings,
    clippy::similar_names
)]

pub mod generate;
