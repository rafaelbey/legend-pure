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

    // GenericType
    let gt_eid = model
        .resolve_by_path(&[
            smol_str::SmolStr::new("meta"),
            smol_str::SmolStr::new("pure"),
            smol_str::SmolStr::new("metamodel"),
            smol_str::SmolStr::new("type"),
            smol_str::SmolStr::new("generics"),
            smol_str::SmolStr::new("GenericType"),
        ])
        .expect("GenericType element should resolve");
    let any_eid = model
        .resolve_by_path(&[
            smol_str::SmolStr::new("meta"),
            smol_str::SmolStr::new("pure"),
            smol_str::SmolStr::new("metamodel"),
            smol_str::SmolStr::new("type"),
            smol_str::SmolStr::new("Any"),
        ])
        .expect("Any element should resolve");
    println!("\n=== Any properties ===");
    if let legend_pure_parser_pure::model::Element::Class(c) = model.get_element(any_eid) {
        for p in &c.properties {
            let type_name = match &p.type_expr {
                legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => {
                    model.get_node(*element).name.to_string()
                }
                other => format!("{other:?}"),
            };
            println!(
                "  {}: type_expr={} mult={:?}",
                p.name, type_name, p.multiplicity
            );
        }
    }

    println!("\n=== GenericType properties ===");
    if let legend_pure_parser_pure::model::Element::Class(c) = model.get_element(gt_eid) {
        for p in &c.properties {
            let type_name = match &p.type_expr {
                legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => {
                    model.get_node(*element).name.to_string()
                }
                other => format!("{other:?}"),
            };
            println!(
                "  {}: type_expr={} mult={:?}",
                p.name, type_name, p.multiplicity
            );
        }
    }

    let element = model.get_element(prop_eid);
    println!("\n=== Property element ===");
    if let legend_pure_parser_pure::model::Element::Class(c) = element {
        println!("type_parameters: {:?}", c.type_parameters);
        println!("multiplicity_parameters: {:?}", c.multiplicity_parameters);
        println!("super_types: {} entries", c.super_types.len());
        for (i, st) in c.super_types.iter().enumerate() {
            println!("  [{i}] {st:?}");
        }
    } else {
        println!("Property is not a Class!");
    }

    // Walk Property's supertype chain.
    println!("\n=== Walking up... ===");
    let mut current = Some(prop_eid);
    let mut depth = 0;
    while let Some(eid) = current
        && depth < 8
    {
        let node = model.get_node(eid);
        let element = model.get_element(eid);
        let supers = match element {
            legend_pure_parser_pure::model::Element::Class(c) => &c.super_types[..],
            _ => &[],
        };
        println!(
            "  [d={depth}] {} → {} super(s)",
            node.name,
            supers.len()
        );
        for (i, st) in supers.iter().enumerate() {
            println!("    [{i}] {st:?}");
        }
        current = supers.iter().find_map(|st| match st {
            legend_pure_parser_pure::types::TypeExpr::Named { element, .. } => Some(*element),
            _ => None,
        });
        depth += 1;
    }
}
