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

//! AggregationAware negative-test parity. Java's
//! `TestAggregationAwareMapping` asserts pointed errors for the
//! following six broken-shape cases — `aggregation_aware_smoke.rs`
//! covered only the DataType-return rules. These fill the gap:
//!
//! 1. Unknown `$src.X` source property in main mapping.
//! 2. Unknown `$src.X` source property in an aggregate view.
//! 3. Unknown target property in an aggregate view (the analog of
//!    the existing `nested_main_mapping_unknown_property_errors`
//!    test, but on the aggregate side).
//! 4. Unknown `$this.X` property in `~groupByFunctions`.
//! 5. Unknown function call inside `~aggregateFn` (e.g.
//!    `summation()` — Java mis-spells `sum()` to test).
//! 6. Multiple `~aggregateValues` in one view parses cleanly
//!    (parser-side coverage Java has via
//!    `testAggregationAwareMappingGrammarMultiViewsMultiAggregateValues`).

use indoc::indoc;
use legend_pure_dsl_mapping::ast::{ClassMappingBody, MappingDef};
use legend_pure_dsl_mapping::compiler::MappingExtension;
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
        vec![Box::new(MappingSectionParser)],
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

fn compile(sources: Vec<SourceFile>) -> Vec<CompilationError> {
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let result = legend_pure_parser_pure::pipeline::compile_with_extensions(&sources, &[], &exts);
    match result {
        Ok(_) => Vec::new(),
        Err(p) => p.errors,
    }
}

#[test]
fn unknown_src_property_in_main_mapping_errors() {
    // Mirrors Java testAggregationAwareMappingErrorInMainSetImplementationTarget.
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales { salesDate : String[1]; revenue : Integer[1]; }
        Class my::test::SalesRaw { salesDate : String[1]; revenue : Integer[1]; }
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
              salesDate : $src.salesDate_NonExistent,
              revenue : $src.revenue
            }
          }
        )
    "};
    let file = parse("agg_main_src.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("salesDate_NonExistent")),
        "expected unknown $src.salesDate_NonExistent error in main mapping; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_src_property_in_aggregate_view_errors() {
    // Mirrors Java testAggregationAwareMappingErrorInAggregateViewTarget.
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales { salesDate : String[1]; revenue : Integer[1]; }
        Class my::test::SalesRaw { salesDate : String[1]; revenue : Integer[1]; }
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
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  salesDate : $src.salesDate_NonExistent,
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
    let file = parse("agg_view_src.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("salesDate_NonExistent")),
        "expected unknown $src.salesDate_NonExistent error in aggregate view; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_target_property_in_aggregate_view_errors() {
    // Mirrors Java testAggregationAwareMappingErrorInAggregateViewProperty.
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales { salesDate : String[1]; revenue : Integer[1]; }
        Class my::test::SalesRaw { salesDate : String[1]; revenue : Integer[1]; }
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
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
                },
                ~aggregateMapping : Pure {
                  ~src my::test::SalesAgg
                  salesDate_nonExistent : $src.salesDate,
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
    let file = parse("agg_view_tgt.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("salesDate_nonExistent")),
        "expected unknown target property error in aggregate view; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_property_in_group_by_functions_errors() {
    // Mirrors Java
    // testAggregationAwareMappingErrorInAggregateViewModelOperationGroupByFunction.
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales { salesDate : String[1]; revenue : Integer[1]; }
        Class my::test::SalesRaw { salesDate : String[1]; revenue : Integer[1]; }
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
                  ~groupByFunctions ( $this.salesDate_NonExistent ),
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
    let file = parse("agg_groupby_unknown.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("salesDate_NonExistent")),
        "expected unknown $this.salesDate_NonExistent error in groupByFunctions; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn unknown_function_in_aggregate_fn_errors() {
    // Mirrors Java
    // testAggregationAwareMappingErrorInAggregateViewModelOperationAggregateFunction
    // — `summation()` is misspelled, must surface as unresolved
    // function. Java's exact message: "The system can't find a
    // match for the function: summation(...)".
    let source = indoc! {r"
        ###Pure
        Class my::test::Sales { salesDate : String[1]; revenue : Integer[1]; }
        Class my::test::SalesRaw { salesDate : String[1]; revenue : Integer[1]; }
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
                  ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->summation() ) )
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
    let file = parse("agg_unknown_fn.pure", source);
    let errors = compile(vec![file]);
    assert!(
        errors.iter().any(|e| e.message.contains("summation")),
        "expected unresolved-function error mentioning 'summation'; got: {:#?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

#[test]
fn parses_view_with_multiple_aggregate_values() {
    // Java parser-side coverage from
    // testAggregationAwareMappingGrammarMultiViewsMultiAggregateValues
    // — one view with two aggregateValues entries.
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
                  ~aggregateValues (
                    ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ),
                    ( ~mapFn: $this.units,   ~aggregateFn: $mapped->sum() )
                  )
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
    let file = parse("agg_multi_av.pure", source);
    let m = first_mapping(&file);
    let cm = &m.class_mappings[0];
    let ClassMappingBody::AggregationAware(body) = &cm.body else {
        panic!("expected AggregationAware body");
    };
    assert_eq!(body.views.len(), 1);
    assert_eq!(body.views[0].model_operation.aggregate_values.len(), 2);
}
