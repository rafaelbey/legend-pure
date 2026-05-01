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

//! Probes the `m3_parser`'s coverage of Class metamodel properties.
//!
//! The platform `Class` metaclass (declared in `m3.pure`) defines many
//! reflective properties — `properties`, `qualifiedProperties`,
//! `constraints`, `propertiesFromAssociations`, etc. Some live directly
//! on `Class`; others (like `constraints`) live on a supertype
//! (`ElementWithConstraints`) and reach `Class` via the generalization
//! chain.
//!
//! This test pins the current behavior so a regression in the m3 parser
//! or supertype walk is caught immediately. If a property goes from
//! "found via supertype walk" to "not found at all", that's a fix needed.

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};

fn collect_properties_via_super(model: &PureModel, id: ElementId) -> Vec<String> {
    let mut seen = Vec::new();
    let mut stack = vec![id];
    while let Some(eid) = stack.pop() {
        let Element::Class(c) = model.get_element(eid) else {
            continue;
        };
        for p in &c.properties {
            if !seen.iter().any(|n: &String| n == p.name.as_str()) {
                seen.push(p.name.to_string());
            }
        }
        for st in &c.super_types {
            if let legend_pure_parser_pure::types::TypeExpr::Named { element, .. } = st {
                stack.push(*element);
            }
        }
    }
    seen
}

#[test]
fn class_metamodel_inheritance_chain_exposes_constraints() {
    let model = match legend_pure_core_platform::platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };
    let class_id = model
        .resolve_by_path(&[
            "meta".into(),
            "pure".into(),
            "metamodel".into(),
            "type".into(),
            "Class".into(),
        ])
        .expect("Class element in platform");

    let direct: Vec<&str> = match model.get_element(class_id) {
        Element::Class(c) => c.properties.iter().map(|p| p.name.as_str()).collect(),
        _ => panic!("Class is not Class"),
    };
    eprintln!("Class direct: {direct:?}");

    let with_super = collect_properties_via_super(&model, class_id);
    eprintln!("Class with supertype walk: {with_super:?}");

    // These are declared *directly* on Class in m3.pure
    for must_be_direct in [
        "properties",
        "qualifiedProperties",
        "propertiesFromAssociations",
    ] {
        assert!(
            direct.contains(&must_be_direct),
            "missing directly-declared metaprop '{must_be_direct}'",
        );
    }

    // `constraints` lives on `ElementWithConstraints` — Class reaches it
    // via supertype walk. If this fires, the supertype chain or
    // ElementWithConstraints registration has regressed.
    assert!(
        with_super.iter().any(|s| s == "constraints"),
        "missing 'constraints' even via supertype walk; chain: {with_super:?}",
    );
}
