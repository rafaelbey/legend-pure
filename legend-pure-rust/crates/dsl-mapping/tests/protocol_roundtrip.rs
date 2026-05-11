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

// ---------------------------------------------------------------------------
// c2 coverage — PureInstanceClassMapping body
// ---------------------------------------------------------------------------

#[test]
fn pure_body_with_single_property_serializes_with_lambda_transform() {
    // Single property `legalName : $src.name` exercises the property
    // pointer + transform-wrapped-as-lambda Java-parity contract.
    // Note Java's PureInstanceClassMapping JSON discriminator is
    // `pureInstance`; property-mapping discriminator is
    // `purePropertyMapping`.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSrc
            legalName : $src.name
          }
        )
    "};
    let file = parse("pure_single.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    insta::with_settings!({sort_maps => true}, {
        insta::assert_json_snapshot!("pure_single_property", json, {
            ".sourceInformation" => "<source-info>",
            ".classMappings[].sourceInformation" => "<source-info>",
            ".classMappings[].sourceClassSourceInformation" => "<source-info>",
            ".classMappings[].propertyMappings[].sourceInformation" => "<source-info>",
            ".classMappings[].propertyMappings[].property.sourceInformation" => "<source-info>",
            ".classMappings[].propertyMappings[].transform.sourceInformation" => "<source-info>",
            ".classMappings[].propertyMappings[].transform.body[].sourceInformation" => "<source-info>",
            ".classMappings[].propertyMappings[].transform.body[].parameters[].sourceInformation" => "<source-info>",
        });
    });
}

#[test]
fn pure_body_with_filter_and_multiple_properties() {
    // `~filter`, `~src`, two property mappings: all top-level body
    // fields exercised in one shape. Filter wraps as a no-parameter
    // `LambdaFunction` per Java's
    // PureInstanceClassMappingParseTreeWalker.visitLambda.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSrc
            ~filter $src.active == true
            legalName : $src.name,
            count : $src.employees
          }
        )
    "};
    let file = parse("pure_filter.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let body = &json["classMappings"][0];
    assert_eq!(body["_type"], "pureInstance");
    assert_eq!(body["srcClass"], "my::test::FirmSrc");
    assert!(
        body.get("filter").is_some(),
        "~filter must materialize as a LambdaFunction; got: {body}"
    );
    assert_eq!(
        body["propertyMappings"].as_array().map_or(0, Vec::len),
        2,
        "two property mappings expected; got: {body}"
    );
    assert_eq!(
        body["propertyMappings"][0]["_type"], "purePropertyMapping",
        "property-mapping discriminator must be `purePropertyMapping` (Java parity)"
    );
}

#[test]
fn pure_body_with_local_property_serializes_local_mapping_property_info() {
    // `+computed : String[1] : $src.name` exercises the local-
    // property declaration form. The localMappingProperty field
    // carries `type` + `multiplicity` per Java's
    // LocalMappingPropertyInfo.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Pure
          {
            ~src my::test::FirmSrc
            +computed : String[1] : $src.name
          }
        )
    "};
    let file = parse("pure_local.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let pm = &json["classMappings"][0]["propertyMappings"][0];
    assert_eq!(pm["_type"], "purePropertyMapping");
    let local = &pm["localMappingProperty"];
    assert_eq!(
        local["type"], "String",
        "localMappingProperty.type must reflect declared type; got: {pm}"
    );
    let mult = &local["multiplicity"];
    assert_eq!(
        mult["lowerBound"], 1,
        "PureOne lower bound; got: {pm}"
    );
    assert_eq!(
        mult["upperBound"], 1,
        "PureOne upper bound; got: {pm}"
    );
}

// ---------------------------------------------------------------------------
// c3 coverage — Operation + MergeOperation bodies
// ---------------------------------------------------------------------------

#[test]
fn operation_body_union_form_maps_fqn_to_mapping_operation_enum() {
    // Java's `OperationClassMapping.operation` field is the enum
    // `MappingOperation`, not the FQN. Mangled FQN
    // `union_OperationSetImplementation_1__SetImplementation_MANY_`
    // maps to `STORE_UNION` per `OperationClassMapping.java:27-31`.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Operation
          {
            meta::pure::router::operations::union_OperationSetImplementation_1__SetImplementation_MANY_(rel1, rel2)
          }
        )
    "};
    let file = parse("op_union.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let body = &json["classMappings"][0];
    assert_eq!(body["_type"], "operation");
    assert_eq!(
        body["operation"], "STORE_UNION",
        "Java parity: FQN must map to MappingOperation enum name"
    );
    let params = body["parameters"].as_array().expect("parameters list");
    assert_eq!(params.len(), 2);
    assert_eq!(params[0], "rel1");
    assert_eq!(params[1], "rel2");
}

#[test]
fn operation_body_unknown_fqn_omits_operation_field_per_java_parity() {
    // Java's `funcToOps.get(...)` returns `null` for FQNs outside the
    // four router ops, so the `operation` field serializes as absent.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Operation
          {
            my::custom::userFunction(rel1)
          }
        )
    "};
    let file = parse("op_user_fn.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let body = &json["classMappings"][0];
    assert_eq!(body["_type"], "operation");
    assert!(
        body.get("operation").is_none(),
        "Java parity: unknown FQN omits operation; got: {body}"
    );
}

#[test]
fn merge_operation_body_serializes_validation_function_as_lambda() {
    // Merge form: `[ids], <validation lambda>`. Java's
    // `MergeOperationClassMapping extends OperationClassMapping`
    // adds `validationFunction: LambdaFunction`. The discriminator
    // is `mergeOperation`.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          *my::test::Firm[op] : Operation
          {
            meta::pure::router::operations::merge_OperationSetImplementation_1__SetImplementation_MANY_(
              [p1, p2],
              {p1: my::test::A[1], p2: my::test::B[1] | $p1.id == $p2.id}
            )
          }
        )
    "};
    let file = parse("op_merge.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let body = &json["classMappings"][0];
    assert_eq!(body["_type"], "mergeOperation");
    assert_eq!(body["operation"], "MERGE");
    let params = body["parameters"].as_array().expect("parameters list");
    assert_eq!(params.len(), 2);
    assert!(
        body["validationFunction"].is_object(),
        "merge form must populate validationFunction as LambdaFunction; got: {body}"
    );
    assert!(
        body["validationFunction"]["body"].is_array(),
        "validationFunction must have body array; got: {body}"
    );
}

// ---------------------------------------------------------------------------
// c4 coverage — EnumerationMapping body
// ---------------------------------------------------------------------------

#[test]
fn enumeration_mapping_routes_into_enumeration_mappings_sibling_list() {
    // Enumeration bodies don't appear under `classMappings`. Java's
    // `Mapping.enumerationMappings` is a sibling list. Confirm:
    // - empty `classMappings`
    // - non-empty `enumerationMappings`
    // - each enum value mapping carries source-value variants with
    //   the expected discriminators.
    let source = indoc! {r"
        ###Pure
        Enum my::test::Status
        {
          ACTIVE,
          INACTIVE
        }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Status : EnumerationMapping
          {
            ACTIVE : 'A',
            INACTIVE : ['I', 'X']
          }
        )
    "};
    let file = parse("enum_simple.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    assert!(
        json.get("classMappings").is_none(),
        "Enumeration body must NOT appear in classMappings; got: {json}"
    );
    let enums = json["enumerationMappings"].as_array().expect("enums list");
    assert_eq!(enums.len(), 1);
    let em = &enums[0];
    assert_eq!(em["enumeration"]["fullPath"], "my::test::Status");
    let vms = em["enumValueMappings"].as_array().expect("vms list");
    assert_eq!(vms.len(), 2);
    assert_eq!(vms[0]["enumValue"], "ACTIVE");
    let sv0 = &vms[0]["sourceValues"];
    assert_eq!(sv0[0]["_type"], "stringSourceValue");
    assert_eq!(sv0[0]["value"], "A");
    let sv1 = &vms[1]["sourceValues"];
    assert_eq!(sv1.as_array().map_or(0, Vec::len), 2);
}

#[test]
fn enumeration_mapping_with_enum_ref_source_value_serializes_discriminator() {
    // `pkg::Other.VALUE` source value uses the
    // `enumSourceValue` discriminator with `enumeration` + `value`
    // fields (`EnumValueMappingEnumSourceValue.java:18-20`).
    let source = indoc! {r"
        ###Pure
        Enum my::test::Status { ACTIVE }
        Enum my::test::Other { GO }

        ###Mapping
        Mapping my::test::M
        (
          my::test::Status : EnumerationMapping
          {
            ACTIVE : my::test::Other.GO
          }
        )
    "};
    let file = parse("enum_ref.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let sv = &json["enumerationMappings"][0]["enumValueMappings"][0]["sourceValues"][0];
    assert_eq!(sv["_type"], "enumSourceValue");
    assert_eq!(sv["enumeration"], "my::test::Other");
    assert_eq!(sv["value"], "GO");
}

// ---------------------------------------------------------------------------
// c5 coverage — AggregationAware body
// ---------------------------------------------------------------------------

#[test]
fn aggregation_aware_body_serializes_main_set_implementation_recursively() {
    // Single-view AggregationAware with nested Pure mappings. Pins:
    //  - `_type: aggregationAware` discriminator
    //  - `mainSetImplementation` is itself a `_type: pureInstance`
    //    ClassMapping carrying the outer class FQN
    //  - `aggregateSetImplementations[]` carries `(index, setImpl,
    //    aggregateSpecification)` per Java
    //    `AggregateSetImplementationContainer.java:19-24`.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          *my::test::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate true,
                  ~groupByFunctions ( $this.salesDate ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  salesDate : $src.salesDate,
                  revenue : $src.netRevenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src my::test::SalesRaw
              salesDate : $src.salesDate,
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_simple.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let cm = &json["classMappings"][0];
    assert_eq!(cm["_type"], "aggregationAware");

    let main = &cm["mainSetImplementation"];
    assert_eq!(main["_type"], "pureInstance");
    assert_eq!(
        main["class"], "my::test::Sales",
        "nested mainSetImplementation inherits outer class FQN"
    );
    assert_eq!(main["srcClass"], "my::test::SalesRaw");

    let aggs = cm["aggregateSetImplementations"].as_array().expect("aggregates list");
    assert_eq!(aggs.len(), 1);
    let agg0 = &aggs[0];
    assert_eq!(agg0["index"], 0);
    assert_eq!(agg0["setImplementation"]["_type"], "pureInstance");
    assert_eq!(agg0["setImplementation"]["srcClass"], "my::test::SalesAgg");
    let spec = &agg0["aggregateSpecification"];
    assert_eq!(spec["canAggregate"], true);
    let gbf = spec["groupByFunctions"].as_array().expect("groupBy list");
    assert_eq!(gbf.len(), 1);
    assert!(
        gbf[0]["groupByFn"]["body"].is_array(),
        "each groupByFunction must wrap as LambdaFunction; got: {gbf:#?}"
    );
    let aggvals = spec["aggregateValues"].as_array().expect("aggregateValues list");
    assert_eq!(aggvals.len(), 1);
    assert!(aggvals[0]["mapFn"]["body"].is_array());
    assert!(aggvals[0]["aggregateFn"]["body"].is_array());
}

#[test]
fn aggregation_aware_with_can_aggregate_false_and_multiple_group_by_keys() {
    // Pins `canAggregate: false` round-trip + multiple group-by
    // expressions each wrapped as their own LambdaFunction.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          *my::test::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate false,
                  ~groupByFunctions ( $this.salesDate, $this.region ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  salesDate : $src.salesDate,
                  region : $src.region,
                  revenue : $src.revenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src my::test::SalesRaw
              salesDate : $src.salesDate,
              region : $src.region,
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_multi.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let spec =
        &json["classMappings"][0]["aggregateSetImplementations"][0]["aggregateSpecification"];
    assert_eq!(spec["canAggregate"], false);
    let gbf = spec["groupByFunctions"].as_array().expect("groupBy list");
    assert_eq!(gbf.len(), 2, "two distinct group-by expressions");
}

// ---------------------------------------------------------------------------
// c6 coverage — RelationFunction body
// ---------------------------------------------------------------------------

#[test]
fn relation_function_body_serializes_with_plain_property_mapping() {
    // Single plain property mapping `legalName : name_col`. Pins:
    //  - `_type: relation` discriminator (Java: `RelationFunctionClassMapping`)
    //  - `relationFunction.fullPath` carries the function FQN
    //  - `propertyMappings[0]._type` is `relationFunctionPropertyMapping`
    //  - `column` field carries the column name
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            legalName : name_col
          }
        )
    "};
    let file = parse("rel_plain.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let cm = &json["classMappings"][0];
    assert_eq!(cm["_type"], "relation");
    assert_eq!(cm["relationFunction"]["fullPath"], "my::test::myFn");
    let pm = &cm["propertyMappings"][0];
    assert_eq!(pm["_type"], "relationFunctionPropertyMapping");
    assert_eq!(pm["property"]["class"], "my::test::Firm");
    assert_eq!(pm["property"]["property"], "legalName");
    assert_eq!(pm["column"], "name_col");
}

#[test]
fn relation_function_body_with_binding_transformer_serializes_binding_field() {
    // `Binding pkg::B :` transformer. Java's `BindingTransformer.binding`
    // is a plain `String` FQN, `sourceInformation` is `@JsonIgnore`
    // so absent from the wire. Pin the shape.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            data : Binding my::test::TextBinding : text_col
          }
        )
    "};
    let file = parse("rel_binding.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let pm = &json["classMappings"][0]["propertyMappings"][0];
    let bt = &pm["bindingTransformer"];
    assert_eq!(bt["binding"], "my::test::TextBinding");
    assert!(
        bt.get("sourceInformation").is_none(),
        "BindingTransformer.sourceInformation is @JsonIgnore (Java); must not appear in JSON"
    );
}

#[test]
fn relation_function_body_with_local_mapping_property_serializes_decl() {
    // `+computed : String[1] : composite_col` form. Pins
    // `localMappingProperty` carrying `type` + `multiplicity`.
    let source = indoc! {r"
        ###Mapping
        Mapping my::test::M
        (
          my::test::Firm : Relation
          {
            ~func my::test::myFn():meta::pure::metamodel::relation::Relation<Any>[1]
            +computed : String[1] : composite_col
          }
        )
    "};
    let file = parse("rel_local.pure", source);
    let m = first_mapping(&file);
    let (json, _) = round_trip(m);
    let pm = &json["classMappings"][0]["propertyMappings"][0];
    assert_eq!(pm["column"], "composite_col");
    let local = &pm["localMappingProperty"];
    assert_eq!(local["type"], "String");
    assert_eq!(local["multiplicity"]["lowerBound"], 1);
    assert_eq!(local["multiplicity"]["upperBound"], 1);
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
