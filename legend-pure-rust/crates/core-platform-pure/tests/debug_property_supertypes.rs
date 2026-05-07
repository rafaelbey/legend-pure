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

//! Debug: print Property's super_types from the loaded M3 model so
//! we can see whether the M3 parser captured the Function
//! generalization with FunctionType substitution. Always passes.

#[test]
fn debug_property_supertypes() {
    let model = match legend_pure_core_platform::platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    let prop_eid = model
        .resolve_by_path(&[
            smol_str::SmolStr::new("meta"),
            smol_str::SmolStr::new("pure"),
            smol_str::SmolStr::new("metamodel"),
            smol_str::SmolStr::new("function"),
            smol_str::SmolStr::new("property"),
            smol_str::SmolStr::new("Property"),
        ])
        .expect("Property element should resolve");

    let element = model.get_element(prop_eid);
    println!("\n=== Property element ===");
    if let legend_pure_parser_pure::model::Element::Class(c) = element {
        println!(
            "type_parameters: {:?}",
            c.type_parameters
        );
        println!(
            "multiplicity_parameters: {:?}",
            c.multiplicity_parameters
        );
        println!("super_types: {} entries", c.super_types.len());
        for (i, st) in c.super_types.iter().enumerate() {
            println!("  [{i}] {st:?}");
        }
    } else {
        println!("Property is not a Class!");
    }
}
