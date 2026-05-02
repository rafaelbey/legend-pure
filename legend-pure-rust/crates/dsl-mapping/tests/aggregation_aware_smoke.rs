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

//! Stage-6 AggregationAware coverage:
//!
//! Parsing:
//!   - parse_with_sections accepts `parserName == "AggregationAware"`
//!     and constructs a `ClassMappingBody::AggregationAware(...)`
//!     tree carrying the `Views: [(modelOperation, aggregateMapping)]`
//!     list plus the trailing `~mainMapping` clause.
//!   - The nested `~mainMapping` and `~aggregateMapping` clauses
//!     recurse into `ClassMappingBody` (Pure body in these tests).
//!   - Multiple views in one body parse cleanly.
//!   - Round-trip composer reproduces the parsed tree (counts +
//!     can_aggregate flag survive).
//!
//! Validation (positives + negatives):
//!   - Positive: AggregationAware with one view + nested Pure
//!     mainMapping + nested Pure aggregateMapping validates clean.
//!   - Negative: nested Pure mainMapping has unknown property —
//!     recursive validation catches it via the existing Stage-3.5
//!     property-resolution rule.
//!   - Negative: ~mapFn returns a Class (not a DataType).
//!   - Negative: ~aggregateFn returns a Class (not a DataType).

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{
    AggregationAwareClassMappingBody, ClassMappingBody, MappingDef,
};
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::compose::compose_mapping_section;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_pure::error::CompilationError;
use legend_pure_parser_pure::extension::CompilerExtension;

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
    panic!("no MappingDef in parsed source");
}

fn agg_body(m: &MappingDef, idx: usize) -> &AggregationAwareClassMappingBody {
    let cm = m
        .class_mappings
        .get(idx)
        .expect("expected at least one class mapping");
    let ClassMappingBody::AggregationAware(body) = &cm.body else {
        panic!("class mapping {idx} is not an AggregationAware body");
    };
    body
}

fn compile(sources: Vec<SourceFile>) -> (Vec<CompilationError>, MappingExtension) {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    let errors = match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    };
    (errors, extension)
}

// ----- Parser coverage --------------------------------------------------

#[test]
fn parses_one_view_plus_main_mapping() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          *pkg::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate true,
                  ~groupByFunctions ( $this.salesDate ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src pkg::SalesAgg
                  salesDate : $src.salesDate,
                  revenue : $src.netRevenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src pkg::SalesRaw
              salesDate : $src.salesDate,
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_one.pure", source);
    let body = agg_body(first_mapping(&file), 0);
    assert_eq!(body.views.len(), 1);
    assert!(body.views[0].model_operation.can_aggregate);
    assert_eq!(body.views[0].model_operation.group_by_functions.len(), 1);
    assert_eq!(body.views[0].model_operation.aggregate_values.len(), 1);
    assert_eq!(body.main_mapping.parser_name.as_str(), "Pure");
    assert!(matches!(body.main_mapping.body, ClassMappingBody::Pure(_)));
    assert!(matches!(
        body.views[0].aggregate_mapping.body,
        ClassMappingBody::Pure(_)
    ));
}

#[test]
fn parses_multiple_views() {
    // Two distinct views, multiple groupByFunctions in one of them.
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          *pkg::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate false,
                  ~groupByFunctions (
                    $this.salesDate,
                    $this.region
                  ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src pkg::SalesByQtr
                  salesDate : $src.qtrStart,
                  revenue : $src.netRevenue
                }
              ),
              (
                ~modelOperation : {
                  ~canAggregate true,
                  ~groupByFunctions ( $this.salesDate ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src pkg::SalesByDate
                  salesDate : $src.salesDate,
                  revenue : $src.netRevenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src pkg::SalesRaw
              salesDate : $src.salesDate,
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_many.pure", source);
    let body = agg_body(first_mapping(&file), 0);
    assert_eq!(body.views.len(), 2);
    assert!(!body.views[0].model_operation.can_aggregate);
    assert_eq!(body.views[0].model_operation.group_by_functions.len(), 2);
    assert!(body.views[1].model_operation.can_aggregate);
}

#[test]
fn round_trip_through_composer() {
    let source = indoc! {r"
        ###Mapping
        Mapping pkg::M
        (
          *pkg::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate true,
                  ~groupByFunctions ( $this.salesDate ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src pkg::SalesAgg
                  salesDate : $src.salesDate,
                  revenue : $src.netRevenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src pkg::SalesRaw
              salesDate : $src.salesDate,
              revenue : $src.revenue
            }
          }
        )
    "};
    let file1 = parse("rt1.pure", source);
    let m1 = first_mapping(&file1);
    let composed = compose_mapping_section(&[m1]);

    // Defence against silent-content corruption: structural counts
    // alone would survive a composer that emitted, say, `$src.foo`
    // for a `$this.salesDate` expression. Assert the actual tokens
    // from the input survive into the composed string before the
    // round-trip parse — if any expression body got dropped or
    // rewritten, this catches it before the count assertions even
    // run.
    for needle in [
        "$this.salesDate",
        "$this.revenue",
        "$mapped->sum",
        "$src.salesDate",
        "$src.netRevenue",
        "$src.revenue",
    ] {
        assert!(
            composed.contains(needle),
            "composed AggregationAware lost expression token '{needle}'; composed:\n{composed}"
        );
    }

    let file2 = parse("rt2.pure", &composed);
    let m2 = first_mapping(&file2);

    // is_root + id + class FQN survive.
    assert!(m1.class_mappings[0].is_root && m2.class_mappings[0].is_root);
    assert_eq!(m1.class_mappings[0].id, m2.class_mappings[0].id);

    let b1 = agg_body(m1, 0);
    let b2 = agg_body(m2, 0);
    assert_eq!(b1.views.len(), b2.views.len());
    assert_eq!(
        b1.views[0].model_operation.can_aggregate,
        b2.views[0].model_operation.can_aggregate
    );
    assert_eq!(
        b1.views[0].model_operation.group_by_functions.len(),
        b2.views[0].model_operation.group_by_functions.len()
    );
    assert_eq!(
        b1.views[0].model_operation.aggregate_values.len(),
        b2.views[0].model_operation.aggregate_values.len()
    );
    assert_eq!(b1.main_mapping.parser_name, b2.main_mapping.parser_name);
    // Both nested mappings still parse as Pure after the round-trip.
    assert!(matches!(b2.main_mapping.body, ClassMappingBody::Pure(_)));
    assert!(matches!(
        b2.views[0].aggregate_mapping.body,
        ClassMappingBody::Pure(_)
    ));
}

// ----- Validator coverage -----------------------------------------------

#[test]
fn aggregation_aware_with_pure_mappings_validates_clean() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales
        {
          salesDate : String[1];
          revenue : Integer[1];
        }

        Class my::test::SalesRaw
        {
          salesDate : String[1];
          revenue : Integer[1];
        }

        Class my::test::SalesAgg
        {
          salesDate : String[1];
          netRevenue : Integer[1];
        }

        ###Mapping
        Mapping my::test::SalesMapping
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
    let file = parse("agg_clean.pure", source);
    let (errors, ext) = compile(vec![file]);
    let agg_errors: Vec<_> = errors
        .iter()
        .filter(|e| {
            e.message.contains("AggregationAware")
                || e.message.contains("~mapFn")
                || e.message.contains("~aggregateFn")
        })
        .collect();
    assert!(
        agg_errors.is_empty(),
        "expected no AggregationAware-validator errors; got: {:#?}",
        agg_errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert!(ext.mappings().contains_key("my::test::SalesMapping"));
}

#[test]
fn nested_main_mapping_unknown_property_errors() {
    // Recursive validation must reach the nested Pure mainMapping
    // and surface the existing Stage-3.5 unknown-property rule.
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales { salesDate : String[1]; }
        Class my::test::SalesRaw { salesDate : String[1]; }
        Class my::test::SalesAgg { salesDate : String[1]; netRevenue : Integer[1]; }

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
                  ~aggregateValues ( ( ~mapFn: $this.salesDate, ~aggregateFn: $mapped->joinStrings(',') ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  salesDate : $src.salesDate
                }
              )
            ],
            ~mainMapping : Pure {
              ~src my::test::SalesRaw
              doesNotExist : $src.salesDate
            }
          }
        )
    "};
    let file = parse("agg_nested_unknown.pure", source);
    let (errors, _ext) = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("doesNotExist")),
        "expected unknown-property error in nested main mapping; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn map_fn_returning_class_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales
        {
          inner : my::test::Inner[1];
          revenue : Integer[1];
        }

        Class my::test::Inner { name : String[1]; }
        Class my::test::SalesRaw { revenue : Integer[1]; }
        Class my::test::SalesAgg { revenue : Integer[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate true,
                  ~groupByFunctions ( $this.revenue ),
                  ~aggregateValues ( ( ~mapFn: $this.inner, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  revenue : $src.revenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src my::test::SalesRaw
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_bad_mapfn.pure", source);
    let (errors, _ext) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("~mapFn") && e.message.contains("DataType")),
        "expected mapFn-not-DataType error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn aggregate_fn_returning_class_errors() {
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales
        {
          revenue : Integer[1];
        }

        Class my::test::Inner { name : String[1]; }

        function my::test::makeInner(values : Integer[*]) : my::test::Inner[1]
        {
          ^my::test::Inner(name = 'x')
        }

        Class my::test::SalesRaw { revenue : Integer[1]; }
        Class my::test::SalesAgg { revenue : Integer[1]; }

        ###Mapping
        Mapping my::test::M
        (
          *my::test::Sales[s] : AggregationAware
          {
            Views : [
              (
                ~modelOperation : {
                  ~canAggregate true,
                  ~groupByFunctions ( $this.revenue ),
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: my::test::makeInner_Integer_MANY__Inner_1_($mapped) ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  revenue : $src.revenue
                }
              )
            ],
            ~mainMapping : Pure {
              ~src my::test::SalesRaw
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_bad_aggfn.pure", source);
    let (errors, _ext) = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("~aggregateFn") && e.message.contains("DataType")),
        "expected aggregateFn-not-DataType error; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}
