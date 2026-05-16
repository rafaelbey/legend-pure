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

//! Proves `MappingExtension`'s `#[distributed_slice]` registration
//! surfaces through [`legend_pure_parser_pure::extension::discovered_compiler_extensions`]
//! when the `legend-pure-dsl-mapping` crate is in the consumer's link
//! graph (forced via the `use` below — mirrors the recipe doc's
//! "link forcing" guidance).

#[allow(unused_imports)]
use legend_pure_dsl_mapping::compiler::MappingExtension as _;

use legend_pure_parser_pure::extension::discovered_compiler_extensions;

#[test]
fn mapping_extension_appears_in_discovered_slice() {
    let names: Vec<&'static str> = discovered_compiler_extensions()
        .iter()
        .map(|e| e.name())
        .collect();
    assert!(
        names.contains(&"dsl-mapping"),
        "discovered_compiler_extensions() must include `dsl-mapping` after the \
         dsl-mapping crate is linked into the binary; got: {names:?}",
    );
}
