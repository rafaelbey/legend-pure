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

//! End-to-end proof that the `#[distributed_slice]` self-registrations
//! in the runtime-extension dev-deps (`store-relational-runtime`,
//! `dsl-mapping-runtime`, `dsl-relational-runtime`) actually surface
//! through [`NativeRegistry::discovered`] /
//! [`legend_pure_runtime::dsl::discovered_populators`].
//!
//! Lib-internal `#[cfg(test)]` tests compile against the runtime crate
//! alone, so they cannot see the dev-deps' static registrations — they
//! assert the empty-slice case. This integration test exercises the
//! linked-binary path that real CLI binaries will use.

// Force the dev-dep crates to be linked so their
// `#[distributed_slice]` statics actually appear in the runtime
// extensions slice. Without these `use` lines the linker is free to
// drop the entire crate object file (no reachable code), which silently
// erases the distributed-slice registrations.
//
// This mirrors the "force the link" guidance downstream consumers
// must follow in their own `main.rs`. The recipe doc spells this out
// explicitly.
#[allow(unused_imports)]
use legend_pure_dsl_mapping_runtime::MappingDSLPopulator as _;
#[allow(unused_imports)]
use legend_pure_dsl_relational_runtime::RelationalDatabaseDSLPopulator as _;
#[allow(unused_imports)]
use legend_pure_store_relational_runtime::RelationalStoreExtension as _;

use legend_pure_runtime::dsl::discovered_populators;
use legend_pure_runtime::native::NativeRegistry;

#[test]
fn discovered_picks_up_relational_store_extension() {
    // store-relational-runtime self-registers `RelationalStoreExtension`
    // which contributes 12 native bodies (10 distinct natives — see
    // its own registers_all_ten_native_keys unit test — with 2 overloads each).
    let standard = NativeRegistry::standard();
    let discovered = NativeRegistry::discovered();
    assert!(
        discovered.len() > standard.len(),
        "discovered({}) must include extension natives beyond standard({})",
        discovered.len(),
        standard.len(),
    );
    // Sentinel from the relational extension: executeInDb's mangled FQN.
    assert!(
        discovered
            .get("executeInDb_String_1__DatabaseConnection_1__Integer_1__Integer_1__ResultSet_1_")
            .is_some(),
        "discovered registry must surface RelationalStoreExtension's executeInDb",
    );
}

#[test]
fn discovered_populators_picks_up_all_three_dsl_populators() {
    // dsl-mapping-runtime contributes MappingDSLPopulator.
    // dsl-relational-runtime contributes RelationalDatabaseDSLPopulator
    // and RelationalClassMappingDSLPopulator.
    let pops = discovered_populators();
    let names: Vec<&'static str> = pops.iter().map(|p| p.dsl_name()).collect();
    assert!(
        names.contains(&"Mapping"),
        "discovered_populators must include MappingDSLPopulator, got {names:?}",
    );
    assert!(
        names.contains(&"RelationalDatabase"),
        "discovered_populators must include RelationalDatabaseDSLPopulator, got {names:?}",
    );
    assert!(
        names.contains(&"RelationalClassMapping"),
        "discovered_populators must include RelationalClassMappingDSLPopulator, got {names:?}",
    );
}
