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

//! Stage-1 [`CompilerExtension`] for the Mapping DSL.
//!
//! All four phases (`declare` / `define_signatures` / `define_bodies` /
//! `validate`) inherit the trait's no-op defaults. The struct exists so
//! that future stages have a stable registration site — once a
//! `###Mapping` section parser lands and produces Mapping AST nodes,
//! the corresponding processors and validators will be filled in here.

use legend_pure_parser_pure::extension::CompilerExtension;

/// Compiler extension for the `###Mapping` DSL.
///
/// Stage 1: empty — all phases default to no-op. See the crate-level
/// docs for the staged roadmap.
#[derive(Default)]
pub struct MappingExtension;

impl MappingExtension {
    /// Construct a fresh extension.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl CompilerExtension for MappingExtension {
    fn name(&self) -> &'static str {
        "dsl-mapping"
    }
}
