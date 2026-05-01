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

//! `IslandComposer` for the relation-store accessor (`#>{a::Store.t}#`).
//!
//! Tag: `">"`. Joins `path` with `.` and wraps the result in
//! `#>{ … }#` — single-line, matching the Java engine composer in
//! `DEPRECATED_PureGrammarComposerCore::case ">"`:
//!
//! ```java
//! return "#>{" + path.makeString(".") + "}#";
//! ```
//!
//! Plug-in callers register via [`default_island_composers`] when
//! calling [`legend_pure_parser_compose::island::compose_island_with`].

use legend_pure_parser_ast::island::IslandContent;
use legend_pure_parser_compose::island::IslandComposer;
use legend_pure_parser_compose::writer::IndentWriter;

use crate::ast::{RelationStoreAccessorRef, TAG};

/// Composer for relation-store accessors. Single-line emit:
/// `#>{ joined.path }#`.
pub struct RelationStoreAccessorComposer;

impl IslandComposer for RelationStoreAccessorComposer {
    #[allow(clippy::unnecessary_literal_bound)]
    fn tag(&self) -> &str {
        TAG
    }

    fn compose(&self, w: &mut IndentWriter, content: &dyn IslandContent) {
        if let Some(accessor) = content.as_any().downcast_ref::<RelationStoreAccessorRef>() {
            w.write("#>{");
            for (i, seg) in accessor.path.iter().enumerate() {
                if i > 0 {
                    w.write(".");
                }
                w.write(seg);
            }
            w.write("}#");
        }
    }
}

/// Convenience helper —
/// `vec![Box::new(RelationStoreAccessorComposer)]` for callers that
/// want the store DSL composer registered.
#[must_use]
pub fn default_island_composers() -> Vec<Box<dyn IslandComposer>> {
    vec![Box::new(RelationStoreAccessorComposer)]
}
