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

//! # Legend CLI binary
//!
//! Thin wrapper around `legend_cli::main_entry`. The library does
//! all the work — parses argv, dispatches subcommands, prints
//! diagnostics, exits with the appropriate status code.
//!
//! The only thing this binary file owns is the **force-link `use`
//! statements** for the in-tree runtime-extension crates so the
//! shipped `legend` binary picks them up via the discovered slice.
//! External CLI binaries built on top of `legend-cli` (e.g. the
//! worked `examples/mydsl-extension/`) replicate this pattern with
//! their own extension crates' symbols.

#![forbid(unsafe_code)]

// Force-link the runtime-extension crates so their
// `#[distributed_slice]` registrations end up in the final binary.
// Without these, the linker is free to drop the entire crate object
// file (no reachable code → no contribution to RUNTIME_EXTENSIONS /
// DSL_POPULATORS / etc.). The `discovery_smoke` integration test in
// `crates/runtime/tests/` demonstrates the same pattern.
//
// Downstream consumers building their own CLI binaries follow this
// same recipe — see `docs/extensions/downstream-recipe.md` and the
// `examples/mydsl-extension/` template.
#[allow(unused_imports)]
use legend_pure_dsl_mapping_runtime::MappingDSLPopulator as _;
#[allow(unused_imports)]
use legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator as _;
#[allow(unused_imports)]
use legend_pure_store_relational_runtime::RelationalStoreExtension as _;

fn main() {
    legend_cli::main_entry();
}
