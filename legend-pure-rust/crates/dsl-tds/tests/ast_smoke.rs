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

//! AST-only invariants for [`TDSExpr`]: tag wiring, `Box<dyn
//! IslandContent>` round-trip, and structural equality through the
//! `eq_content` trait method.

use legend_pure_dsl_tds::ast::{TAG, TDSCell, TDSColumn, TDSColumnType, TDSExpr};
use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_ast::source_info::SourceInfo;
use smol_str::SmolStr;

fn dummy_si() -> SourceInfo {
    SourceInfo::new("ast_smoke.pure", 1, 1, 1, 1)
}

fn sample() -> TDSExpr {
    TDSExpr {
        columns: vec![
            TDSColumn {
                name: SmolStr::new("value"),
                type_ref: None,
                source_info: dummy_si(),
            },
            TDSColumn {
                name: SmolStr::new("name"),
                type_ref: Some(TDSColumnType {
                    name: SmolStr::new("String"),
                    multiplicity: Some(SmolStr::new("1")),
                    source_info: dummy_si(),
                }),
                source_info: dummy_si(),
            },
        ],
        rows: vec![
            vec![
                TDSCell {
                    raw: SmolStr::new("1"),
                    source_info: dummy_si(),
                },
                TDSCell {
                    raw: SmolStr::new("A"),
                    source_info: dummy_si(),
                },
            ],
            vec![
                TDSCell {
                    raw: SmolStr::new("2"),
                    source_info: dummy_si(),
                },
                TDSCell {
                    raw: SmolStr::new("B"),
                    source_info: dummy_si(),
                },
            ],
        ],
        source_info: dummy_si(),
    }
}

#[test]
fn tag_is_tds() {
    assert_eq!(TAG, "TDS");
    let s = sample();
    assert_eq!(IslandContent::tag(&s), "TDS");
}

#[test]
fn boxed_island_content_round_trips_through_clone_box() {
    let s = sample();
    let boxed: Box<dyn IslandContent> = Box::new(s.clone());
    let cloned = boxed.clone_box();
    assert!(cloned.eq_content(&*boxed));
}

#[test]
fn eq_content_distinguishes_different_columns() {
    let a = sample();
    let mut b = sample();
    b.columns[0].name = SmolStr::new("VALUE");
    assert!(!a.eq_content(&b));
}
