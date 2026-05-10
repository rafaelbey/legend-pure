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

//! Round-trip pin for the `Element::DSLInstance` graph-element path:
//! compile → walk graph → slice → write `.purem` → read → merge → walk
//! graph again. The same diagrams must reappear on the far side, proving
//! the chunk machinery carries DSL-defined elements through `.purem`
//! exactly the way it carries `Class` / `Function`.
//!
//! All reads route through `DiagramExtension::diagrams_from_model`,
//! which walks the graph for `Element::DSLInstance` rows keyed
//! `"Diagram"`. The legacy `extension.diagrams()` post-compile
//! accessor was removed when the RefCell was demoted to a private
//! during-compile cache.

use indoc::indoc;
use legend_pure_dsl_diagram::compiler::DiagramExtension;
use legend_pure_dsl_diagram::parser::DiagramSectionParser;
use legend_pure_parser_ast::SourceFile;
use legend_pure_parser_pure::extension::CompilerExtension;
use legend_pure_parser_pure::pipeline::{compile_with_extensions, init_bootstrap_model};
use legend_pure_parser_pure::purem::{merge_slice, read_repo, slice_by_repo, write_repo};

const FIXTURE: &str = indoc! {r"
    ###Pure
    Class model::test::A {}
    Class model::test::B {}

    ###Diagram
    Diagram model::test::TinyDiagram(width=100.0, height=80.0)
    {
        TypeView A(type=model::test::A, position=(0.0, 0.0), width=80.0, height=40.0)
        TypeView B(type=model::test::B, position=(100.0, 0.0), width=80.0, height=40.0)
        GeneralizationView A_to_B(source=A, target=B)
    }
"};

fn parse(source: &str) -> SourceFile {
    legend_pure_parser_parser::parse_with_sections(
        source,
        "purem_roundtrip.pure",
        legend_pure_parser_parser::island::default_island_parsers(),
        vec![Box::new(DiagramSectionParser)],
    )
    .unwrap_or_else(|p| {
        panic!(
            "parse failed: {:?}",
            p.errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        )
    })
}

#[test]
fn diagram_purem_roundtrip_preserves_diagrams() {
    // ----- Phase 1: compile the source ------------------------------
    let file = parse(FIXTURE);
    let extension = DiagramExtension::new();
    let exts: [&dyn CompilerExtension; 1] = [&extension];
    let model = compile_with_extensions(&[file], &[], &exts).unwrap_or_else(|p| {
        // Diagram views reference unresolved bootstrap classes here
        // (no platform loaded), but those errors are M3-layer and
        // don't break this round-trip — surface the partial model.
        p.model
    });

    // ----- Phase 2: pre-slice graph walk ----------------------------
    // The diagram must already be in the graph as `Element::DSLInstance`,
    // not just in the extension's RefCell. This is what the user's
    // architectural reframing buys us.
    let pre = DiagramExtension::diagrams_from_model(&model);
    assert_eq!(
        pre.len(),
        1,
        "expected 1 diagram in the graph after compile; got {}",
        pre.len()
    );
    assert_eq!(pre[0].0.as_str(), "model::test::TinyDiagram");
    assert_eq!(pre[0].1.views.len(), 3, "fixture has 3 views");

    // ----- Phase 3: slice + write .purem ----------------------------
    // Slice the just-compiled chunk range. Chunk 0 is bootstrap; the
    // synthetic file's chunk is the only non-bootstrap one.
    let chunk_range = 1u16..(model.chunks.len() as u16);
    let slice = slice_by_repo(&model, chunk_range);
    let blob = write_repo(&slice).expect(".purem write must succeed");

    // ----- Phase 4: read + merge into a fresh model -----------------
    let read_back = read_repo(&blob).expect(".purem read must succeed");
    let mut fresh = init_bootstrap_model();
    merge_slice(&mut fresh, read_back).expect("merge_slice must succeed");

    // ----- Phase 5: post-merge graph walk ---------------------------
    let post = DiagramExtension::diagrams_from_model(&fresh);
    assert_eq!(
        post.len(),
        1,
        "expected 1 diagram in the graph after .purem round-trip; got {}",
        post.len()
    );
    assert_eq!(post[0].0.as_str(), "model::test::TinyDiagram");
    assert_eq!(
        post[0].1, pre[0].1,
        "DiagramSnapshot must match exactly through .purem round-trip"
    );

}
