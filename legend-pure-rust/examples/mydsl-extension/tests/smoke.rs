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

//! End-to-end smoke: load the example lib (which force-links all
//! six `#[distributed_slice]` statics) and assert each contribution
//! reaches the corresponding discovered slice. This is the proof
//! the recipe doc promises — a downstream consumer that just adds
//! an extension crate to its Cargo.toml + a single `use` line gets
//! everything wired without per-binary plumbing.

#![allow(unused_imports)]
use legend_pure_mydsl_extension::prelude::{
    MyDslCompilerExtension, MyDslIdeExtension, MyDslIslandParser, MyDslPopulator,
    MyDslRuntimeExtension, MyDslSectionParser,
};

use legend_pure_ide::discovered_ide_extensions;
use legend_pure_parser_parser::island::discovered_island_parsers;
use legend_pure_parser_parser::section_parser::discovered_section_parsers;
use legend_pure_parser_pure::extension::discovered_compiler_extensions;
use legend_pure_runtime::dsl::discovered_populators;
use legend_pure_runtime::native::NativeRegistry;

#[test]
fn runtime_extension_is_discovered() {
    let reg = NativeRegistry::discovered();
    assert!(
        reg.get("greet_String_1__String_1_").is_some(),
        "MyDslRuntimeExtension's greet native must appear in NativeRegistry::discovered()",
    );
}

#[test]
fn compiler_extension_is_discovered() {
    let names: Vec<&'static str> = discovered_compiler_extensions()
        .iter()
        .map(|e| e.name())
        .collect();
    assert!(
        names.contains(&"mydsl-compiler"),
        "MyDslCompilerExtension must appear in discovered_compiler_extensions(); got: {names:?}",
    );
}

#[test]
fn section_parser_is_discovered() {
    let kinds: Vec<&str> = discovered_section_parsers()
        .iter()
        .map(|p| p.kind())
        .collect();
    assert!(
        kinds.contains(&"MyDsl"),
        "MyDslSectionParser must appear in discovered_section_parsers(); got: {kinds:?}",
    );
}

#[test]
fn island_parser_is_discovered() {
    let tags: Vec<&str> = discovered_island_parsers()
        .iter()
        .map(|p| p.tag())
        .collect();
    assert!(
        tags.contains(&"mytag"),
        "MyDslIslandParser must appear in discovered_island_parsers(); got: {tags:?}",
    );
}

#[test]
fn dsl_populator_is_discovered() {
    let names: Vec<&'static str> = discovered_populators()
        .iter()
        .map(|p| p.dsl_name())
        .collect();
    assert!(
        names.contains(&"MyDsl"),
        "MyDslPopulator must appear in discovered_populators(); got: {names:?}",
    );
}

#[test]
fn ide_extension_is_discovered() {
    let names: Vec<&'static str> = discovered_ide_extensions()
        .iter()
        .map(|e| e.name())
        .collect();
    assert!(
        names.contains(&"mydsl-ide"),
        "MyDslIdeExtension must appear in discovered_ide_extensions(); got: {names:?}",
    );
}
