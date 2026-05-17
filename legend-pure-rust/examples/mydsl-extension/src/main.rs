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

//! `mydsl-legend` — a custom CLI wrapper showing how a downstream
//! consumer ships their own `legend`-flavoured binary.
//!
//! The body is one line. Everything else is **link forcing**: the
//! six `use … as _;` statements below pull symbols from this
//! crate's library so the linker keeps its object file, which is
//! what makes the `#[distributed_slice]` statics inside
//! `legend_pure_mydsl_extension` actually contribute to the
//! discovered slices. Without these `use`s, Rust's linker is free
//! to drop the crate entirely (no other reachable code references
//! it) and the discovery layer would silently return empty.
//!
//! Once linked, `legend_cli::main()` does the rest: parses CLI
//! args, dispatches to the requested subcommand (`test`, `run`,
//! `check`, …), each of which calls
//! `NativeRegistry::discovered()` / `Evaluator::builder().build()` /
//! `pipeline::compile()` internally and picks up this crate's
//! extensions automatically.
//!
//! Downstream consumers building their own binary copy this `main`
//! verbatim and replace the import lines with their own extension
//! crate's symbols.

#![allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslCompilerExtension as _;
#[allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslIdeExtension as _;
#[allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslIslandParser as _;
#[allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslPopulator as _;
#[allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslRuntimeExtension as _;
#[allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::MyDslSectionParser as _;

fn main() {
    // Delegates to the stock `legend` CLI dispatch. Subcommands
    // pick up the link-forced extensions via the discovery layer.
    // Replace with your own clap-based main if you need to add
    // bespoke subcommands beyond what `legend` ships.
    legend_cli::main_entry();
}
