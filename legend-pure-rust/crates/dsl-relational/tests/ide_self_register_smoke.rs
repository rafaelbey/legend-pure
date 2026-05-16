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

//! Proves `RelationalIdeExtension`'s `#[distributed_slice(IDE_EXTENSIONS)]`
//! registration surfaces through
//! [`legend_pure_parser_pure::refs::discovered_ide_extensions`] when
//! the `dsl-relational` crate is in the consumer's link graph. The
//! IDE extension was extracted from
//! `CompilerExtension::walk_references` in Phase 2-FULL — this test
//! pins the new discovery path.

#[allow(unused_imports)]
use legend_pure_dsl_relational::compiler::RelationalIdeExtension as _;

use legend_pure_parser_pure::refs::discovered_ide_extensions;

#[test]
fn relational_ide_extension_appears_in_discovered_ide_slice() {
    let names: Vec<&'static str> = discovered_ide_extensions()
        .iter()
        .map(|e| e.name())
        .collect();
    assert!(
        names.contains(&"RelationalExtension"),
        "discovered_ide_extensions() must include `RelationalExtension` after the \
         dsl-relational crate is linked into the binary; got: {names:?}",
    );
}
