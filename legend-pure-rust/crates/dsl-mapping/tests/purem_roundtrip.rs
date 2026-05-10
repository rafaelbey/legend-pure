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

//! Round-trip pin for the Mapping DSL's `Element::DSLInstance` graph
//! path: compile → walk graph → slice → write `.purem` → read → merge
//! → walk graph again. Mirrors `crates/dsl-diagram/tests/purem_roundtrip.rs`.
//!
//! The RefCell registry side (`extension.mappings()`) is not asserted
//! on the post-merge model — it's an in-process artefact of the
//! original compile and is **expected** to be empty after a `.purem`
//! round-trip. The graph-walk side (`MappingExtension::mappings_from_model`)
//! is the durable read.

use indoc::indoc;
use legend_pure_dsl_mapping::compiler::MappingExtension;
use legend_pure_dsl_mapping::parser::MappingSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::pipeline::{compile_with_extensions, init_bootstrap_model};
use legend_pure_parser_pure::purem::{merge_slice, read_repo, slice_by_repo, write_repo};

// Pure section first so the slice's chunk has at least one M3
// element. Without an M3 element, the round-trip behaves
// differently — a follow-up question, but not in scope for the
// pilot which mirrors the Diagram test's source shape.
const FIXTURE: &str = indoc! {r"
    ###Pure
    Class pkg::Firm { legalName : String[1]; }

    ###Mapping
    Mapping pkg::M
    (
      pkg::Firm : Pure
      {
        legalName : 'literal'
      }
    )
"};

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "purem_roundtrip.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(MappingSectionParser::new())],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

#[test]
fn mapping_purem_roundtrip_preserves_mappings() {
    // ----- Phase 1: compile the source ------------------------------
    let file = parse(FIXTURE);
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let model = compile_with_extensions(&[file], &[], &exts).unwrap_or_else(|p| {
        // M3-layer errors are expected here (no platform loaded), but
        // they don't break the graph round-trip — surface the partial
        // model.
        p.model
    });

    // ----- Phase 2: pre-slice graph walk ----------------------------
    let pre = MappingExtension::mappings_from_model(&model);
    assert_eq!(
        pre.len(),
        1,
        "expected 1 mapping in the graph after compile; got {}",
        pre.len()
    );
    assert_eq!(pre[0].0.as_str(), "pkg::M");
    assert_eq!(pre[0].1.class_mappings.len(), 1);
    assert_eq!(pre[0].1.class_mappings[0].class_fqn.as_str(), "pkg::Firm");
    assert_eq!(pre[0].1.class_mappings[0].body_kind.as_str(), "Pure");

    // ----- Phase 3: slice + write .purem ----------------------------
    let chunk_range = 1u16..(model.chunks.len() as u16);
    let slice = slice_by_repo(&model, chunk_range);
    let blob = write_repo(&slice).expect(".purem write must succeed");

    // ----- Phase 4: read + merge into a fresh model -----------------
    let read_back = read_repo(&blob).expect(".purem read must succeed");
    let mut fresh = init_bootstrap_model();
    merge_slice(&mut fresh, read_back).expect("merge_slice must succeed");

    // ----- Phase 5: post-merge graph walk ---------------------------
    let post = MappingExtension::mappings_from_model(&fresh);
    assert_eq!(
        post.len(),
        1,
        "expected 1 mapping in the graph after .purem round-trip; got {}",
        post.len()
    );
    assert_eq!(post[0].0.as_str(), "pkg::M");
    assert_eq!(
        post[0].1, pre[0].1,
        "MappingSnapshot must match exactly through .purem round-trip"
    );

    // ----- Phase 6: legacy RefCell on a fresh extension is empty ----
    let fresh_extension = MappingExtension::new();
    assert!(
        fresh_extension.mappings().is_empty(),
        "fresh extension has no in-process state; round-trip flows \
         through the graph instead"
    );
}

#[test]
fn mapping_purem_roundtrip_preserves_includes() {
    // Two mappings, one includes the other. The snapshot's
    // `includes` field must round-trip the FQN.
    let source = indoc! {r"
        ###Pure
        Class pkg::Firm { legalName : String[1]; }

        ###Mapping
        Mapping pkg::Base
        (
          pkg::Firm : Pure
          {
            legalName : 'base'
          }
        )

        Mapping pkg::Derived
        (
          include pkg::Base
        )
    "};
    let file = parse(source);
    let extension = MappingExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let model = compile_with_extensions(&[file], &[], &exts).unwrap_or_else(|p| p.model);

    let chunk_range = 1u16..(model.chunks.len() as u16);
    let slice = slice_by_repo(&model, chunk_range);
    let blob = write_repo(&slice).expect(".purem write");
    let read_back = read_repo(&blob).expect(".purem read");
    let mut fresh = init_bootstrap_model();
    merge_slice(&mut fresh, read_back).expect("merge_slice");

    let post = MappingExtension::mappings_from_model(&fresh);
    assert_eq!(post.len(), 2);

    let derived = post
        .iter()
        .find(|(fqn, _)| fqn.as_str() == "pkg::Derived")
        .expect("Derived mapping in graph");
    assert_eq!(derived.1.includes.len(), 1);
    assert_eq!(derived.1.includes[0].as_str(), "pkg::Base");
}
