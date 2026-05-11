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

//! Java-parity round-trip tests for `crates/dsl-mapping/src/protocol/`.
//!
//! Each test:
//! 1. Parses a `.pure` source containing a `###Mapping` block.
//! 2. Converts the `MappingDef` AST → `ProtocolMapping`.
//! 3. Serializes to JSON.
//! 4. Asserts the JSON shape via `insta::assert_json_snapshot!`.
//! 5. Deserializes the JSON back into `ProtocolMapping`.
//! 6. Asserts the round-trip is structurally equal.
//!
//! `_type` discriminator names mirror Java's `@JsonSubTypes` exactly:
//! `pureInstance` / `operation` / `mergeOperation` / `aggregationAware` /
//! `relation` for class mappings; `mappingIncludeMapping` for includes.
//!
//! Snapshots live under `tests/snapshots/`. Run `cargo insta review`
//! after intentional shape changes.

use indoc::indoc;
use legend_pure_dsl_mapping::ast::MappingDef;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_dsl_mapping::protocol::ProtocolMapping;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;

fn parse(name: &str, source: &str) -> SourceFile {
    let result = legend_pure_parser_parser::parse_with_sections(
        source,
        name,
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    );
    match result {
        Ok(f) => f,
        Err(p) => panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        ),
    }
}

fn first_mapping(file: &SourceFile) -> &MappingDef {
    for section in &file.sections {
        for elem in &section.elements {
            if let AstElement::DSLElement(boxed) = elem
                && let Some(m) = boxed.as_any().downcast_ref::<MappingDef>()
            {
                return m;
            }
        }
    }
    panic!("no MappingDef found in parsed source");
}

/// Round-trip helper: AST → JSON → AST', assert structural equality.
fn round_trip(mapping: &MappingDef) -> (serde_json::Value, ProtocolMapping) {
    let proto: ProtocolMapping = mapping.into();
    let json = serde_json::to_value(&proto).expect("serialization must succeed");
    let back: ProtocolMapping =
        serde_json::from_value(json.clone()).expect("deserialization must succeed");
    assert_eq!(proto, back, "AST → JSON → AST must be structurally identical");
    (json, proto)
}

// ---------------------------------------------------------------------------
// c1 coverage
// ---------------------------------------------------------------------------

#[test]
fn empty_mapping_serializes_to_minimal_json() {
    // An empty `Mapping pkg::M ()` exercises the container shape with
    // no class mappings and no includes. Java-parity check: missing
    // `classMappings` / `includedMappings` keys (skipped via
    // `skip_serializing_if = "Vec::is_empty"`) deserialize as empty
    // lists per Jackson's defaults; we want the SAME shape on output.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::EmptyMapping
        (
        )
    "};
    let file = parse("empty_mapping.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    insta::with_settings!({sort_maps => true}, {
        insta::assert_json_snapshot!("empty_mapping", json, {
            ".sourceInformation" => "<source-info>",
        });
    });
}

#[test]
fn mapping_with_includes_serializes_include_list() {
    // Single substitution per include — Java's lossy single-
    // substitution JSON shape. Pin the `(source, target)` pair
    // appearing in `MappingIncludeMapping`.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::Inner
        (
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [my::test::DbA -> my::test::DbB]
        )
    "};
    let file = parse("with_include.pure", source);
    // Find the Outer mapping (the one with the include).
    let outer = file
        .sections
        .iter()
        .flat_map(|s| s.elements.iter())
        .filter_map(|e| match e {
            AstElement::DSLElement(b) => b.as_any().downcast_ref::<MappingDef>(),
            _ => None,
        })
        .find(|m| m.name.value.as_str() == "Outer")
        .expect("expected the Outer mapping");
    let (json, _) = round_trip(outer);
    insta::with_settings!({sort_maps => true}, {
        insta::assert_json_snapshot!("mapping_with_single_include", json, {
            ".sourceInformation" => "<source-info>",
            ".includedMappings[].sourceInformation" => "<source-info>",
        });
    });
}

#[test]
fn include_with_three_substitutions_loses_pair_per_java_parity() {
    // Multi-substitution AST: Java's protocol JSON only carries ONE
    // (source, target) pair. With three substitutions, Java's
    // `CorePureGrammarParser.parseMappingInclude` sets both to null.
    // Mirror that — both fields absent in JSON.
    //
    // This is intentional Java parity (not a bug): the AST keeps the
    // full vector for validators and `.purem` snapshots; only the
    // wire format loses the extras.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::Inner
        (
        )

        Mapping my::test::Outer
        (
          include my::test::Inner [
            my::test::DbA -> my::test::DbX,
            my::test::DbB -> my::test::DbY,
            my::test::DbC -> my::test::DbZ
          ]
        )
    "};
    let file = parse("multi_sub.pure", source);
    let outer = file
        .sections
        .iter()
        .flat_map(|s| s.elements.iter())
        .filter_map(|e| match e {
            AstElement::DSLElement(b) => b.as_any().downcast_ref::<MappingDef>(),
            _ => None,
        })
        .find(|m| m.name.value.as_str() == "Outer")
        .expect("expected the Outer mapping");
    let (json, _) = round_trip(outer);
    let include = &json["includedMappings"][0];
    assert_eq!(include["_type"], "mappingIncludeMapping");
    assert_eq!(include["includedMapping"], "my::test::Inner");
    assert!(
        include.get("sourceDatabasePath").is_none(),
        "Java-parity: multi-substitution loses sourceDatabasePath; got: {include}"
    );
    assert!(
        include.get("targetDatabasePath").is_none(),
        "Java-parity: multi-substitution loses targetDatabasePath; got: {include}"
    );
}
