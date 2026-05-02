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

//! Stage-1 end-to-end test: load the embedded platform (now including
//! `platform_dsl_mapping`) and verify that the mapping metamodel
//! classes resolve. Asserts the descriptor entry in
//! `core-platform-pure/Cargo.toml` plus the `embedded_platform_dsl_mapping`
//! constructor in `repo.rs` correctly embed the metamodel.
//!
//! There is no `###Mapping` section parser yet, so no user-side
//! mapping syntax is exercised — that lands in Stage 2.

use legend_pure_core_platform::platform::load_platform;
use legend_pure_parser_pure::model::Element as ModelElement;
use smol_str::SmolStr;

#[test]
fn mapping_metamodel_classes_resolve_in_loaded_platform() {
    let model = match load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    // The Mapping metamodel lives at meta::pure::mapping::*. Each
    // entry below is a class declared in
    // platform_dsl_mapping/grammar/mapping.pure that downstream code
    // (Stage-2 processors, future extension authors) will reach for by
    // qualified name.
    for fqn in [
        ["meta", "pure", "mapping", "Mapping"].as_slice(),
        ["meta", "pure", "mapping", "SetImplementation"].as_slice(),
        ["meta", "pure", "mapping", "InstanceSetImplementation"].as_slice(),
        ["meta", "pure", "mapping", "PropertyMapping"].as_slice(),
        ["meta", "pure", "mapping", "EnumerationMapping"].as_slice(),
        ["meta", "pure", "mapping", "AssociationImplementation"].as_slice(),
    ] {
        let segments: Vec<SmolStr> = fqn.iter().map(|s| SmolStr::new(*s)).collect();
        let id = model
            .resolve_by_path(&segments)
            .unwrap_or_else(|| panic!("mapping metamodel class missing: {fqn:?}"));
        assert!(
            matches!(model.get_element(id), ModelElement::Class(_)),
            "expected {fqn:?} to be a Class element"
        );
    }
}
