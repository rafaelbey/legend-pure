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

//! Round-trip pin for the Relational DSL's `Element::DSLInstance`
//! graph path. Mirrors the Diagram + Mapping pilots: compile → walk
//! graph → slice → write `.purem` → read → merge → walk graph.

use indoc::indoc;
use legend_pure_dsl_relational::compiler::RelationalExtension;
use legend_pure_dsl_relational::parser::RelationalSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::pipeline::{compile_with_extensions, init_bootstrap_model};
use legend_pure_parser_pure::purem::{merge_slice, read_repo, slice_by_repo, write_repo};

/// DSL-only source — exercises the `pass_declare` fix that pushes a
/// chunk for any non-empty source file.
const FIXTURE: &str = indoc! {r"
    ###Relational
    Database pkg::PaymentsDb
    (
      Schema main
      (
        Table customers ( id INTEGER PRIMARY KEY, name VARCHAR(100) )
        Table orders    ( id INTEGER PRIMARY KEY, customer_id INTEGER )
      )

      Join cust_orders ( main.customers.id = main.orders.customer_id )
    )
"};

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "purem_roundtrip.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(RelationalSectionParser)],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

#[test]
fn relational_purem_roundtrip_preserves_databases() {
    // ----- Phase 1: compile the source ------------------------------
    let file = parse(FIXTURE);
    let extension = RelationalExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let model = compile_with_extensions(&[file], &[], &exts).unwrap_or_else(|p| {
        // M3-layer errors are expected here (no platform loaded);
        // surface the partial model.
        p.model
    });

    // ----- Phase 2: pre-slice graph walk ----------------------------
    let pre = RelationalExtension::databases_from_model(&model);
    assert_eq!(
        pre.len(),
        1,
        "expected 1 database in the graph after compile; got {}",
        pre.len()
    );
    assert_eq!(pre[0].0.as_str(), "pkg::PaymentsDb");
    assert_eq!(pre[0].1.schemas.len(), 1, "1 named schema");
    assert_eq!(pre[0].1.schemas[0].name.as_str(), "main");
    assert_eq!(pre[0].1.schemas[0].tables.len(), 2);
    assert_eq!(pre[0].1.joins.len(), 1);
    assert_eq!(pre[0].1.joins[0].as_str(), "cust_orders");

    // ----- Phase 3: slice + write .purem ----------------------------
    let chunk_range = 1u16..(model.chunks.len() as u16);
    let slice = slice_by_repo(&model, chunk_range);
    let blob = write_repo(&slice).expect(".purem write must succeed");

    // ----- Phase 4: read + merge into a fresh model -----------------
    let read_back = read_repo(&blob).expect(".purem read must succeed");
    let mut fresh = init_bootstrap_model();
    merge_slice(&mut fresh, read_back).expect("merge_slice must succeed");

    // ----- Phase 5: post-merge graph walk ---------------------------
    let post = RelationalExtension::databases_from_model(&fresh);
    assert_eq!(
        post.len(),
        1,
        "expected 1 database in the graph after .purem round-trip; got {}",
        post.len()
    );
    assert_eq!(post[0].0.as_str(), "pkg::PaymentsDb");
    assert_eq!(
        post[0].1, pre[0].1,
        "DatabaseSnapshot must match exactly through .purem round-trip"
    );

}

#[test]
fn relational_purem_roundtrip_preserves_includes_and_filters() {
    // Database with an include + a Filter, so the snapshot exercises
    // both `includes` and `filters` fields.
    let source = indoc! {r"
        ###Relational
        Database pkg::CommonDb
        (
          Schema common ( Table audit ( id INTEGER PRIMARY KEY ) )
        )

        Database pkg::AppDb
        (
          include pkg::CommonDb

          Schema app ( Table events ( id INTEGER PRIMARY KEY ) )

          Filter active_only ( app.events.id > 0 )
        )
    "};
    let file = parse(source);
    let extension = RelationalExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let model = compile_with_extensions(&[file], &[], &exts).unwrap_or_else(|p| p.model);

    let chunk_range = 1u16..(model.chunks.len() as u16);
    let slice = slice_by_repo(&model, chunk_range);
    let blob = write_repo(&slice).expect(".purem write");
    let read_back = read_repo(&blob).expect(".purem read");
    let mut fresh = init_bootstrap_model();
    merge_slice(&mut fresh, read_back).expect("merge_slice");

    let post = RelationalExtension::databases_from_model(&fresh);
    assert_eq!(post.len(), 2);

    let app = post
        .iter()
        .find(|(fqn, _)| fqn.as_str() == "pkg::AppDb")
        .expect("AppDb in graph");
    assert_eq!(app.1.includes.len(), 1);
    assert_eq!(app.1.includes[0].as_str(), "pkg::CommonDb");
    assert_eq!(app.1.filters.len(), 1);
    assert_eq!(app.1.filters[0].as_str(), "active_only");
}
