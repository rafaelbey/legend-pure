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

//! Locks the `TDSIslandProtocol` end-to-end: a `#TDS\n cols\n rows\n#`
//! island parses through `dsl-tds`'s parser plug-in, then converts to
//! a v1 `ClassInstance { _type: "TDS", columns: [...], rows: [...] }`
//! JSON value via `dispatch_island_convert` +
//! `default_island_protocols()`.

use indoc::indoc;
use legend_pure_dsl_tds::parser::default_island_parsers;
use legend_pure_dsl_tds::protocol::default_island_protocols;
use legend_pure_parser_ast::element::Element;
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_protocol::IslandProtocol;
use legend_pure_parser_protocol::dispatch_island_convert;

#[test]
fn tds_converts_to_protocol_class_instance() {
    let source = format!(
        "###Pure\nfunction my::func(): Any[*]\n{{\n{}\n}}\n",
        indoc! {"
            #TDS
              value:Float, name
              1, A
              2, B
            #
        "}
    );
    let sf = legend_pure_parser_parser::parse_with_islands(
        &source,
        "protocol_smoke.pure",
        default_island_parsers(),
    )
    .expect("parse failed");

    let ast_elem = sf.all_elements().next().expect("first element");
    let Element::Function(f) = ast_elem else {
        panic!("expected function");
    };
    let Expression::Island(island) = f.body.first().expect("body") else {
        panic!("expected island expression");
    };

    let protocols = default_island_protocols();
    let refs: Vec<&dyn IslandProtocol> =
        protocols.iter().map(std::convert::AsRef::as_ref).collect();
    let value = dispatch_island_convert(island, &refs)
        .expect("convert ok")
        .expect("converter matched");

    let json = serde_json::to_value(&value).expect("serialise to json");
    assert_eq!(
        json["type"], "TDS",
        "ClassInstance.type should be `TDS`. Got: {json}"
    );
    assert_eq!(json["value"]["_type"], "TDS", "value._type should be TDS");

    let columns = json["value"]["columns"].as_array().expect("columns array");
    assert_eq!(columns.len(), 2);
    assert_eq!(columns[0]["name"], "value");
    assert_eq!(columns[0]["type"], "Float");
    assert_eq!(columns[1]["name"], "name");
    assert!(columns[1]["type"].is_null());

    let rows = json["value"]["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    let r0: Vec<&str> = rows[0]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(r0, vec!["1", "A"]);
}
