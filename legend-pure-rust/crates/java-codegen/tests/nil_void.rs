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

//! Pinning the Pure-Nil-to-Java-Void mapping. The platform doesn't
//! actually expose any user-facing function whose return is typed
//! `Nil[1]`, but the codegen rule must still hold so the moment one
//! does (or a class property gets typed as `Nil[*]`), the surface stays
//! coherent: `Nil[1]` → `Void`, `Nil[*]` → `Iterable<Void>`.

mod common;

use legend_pure_java_codegen::{FqnInput, Options, generate};

const NIL_CLASS_SOURCE: &str = r#"
Class user_test::HasNilProp
{
    label: String[1];
    nada: meta::pure::metamodel::type::Nil[*];
    maybeNada: meta::pure::metamodel::type::Nil[0..1];
}
"#;

#[test]
fn nil_property_renders_as_void() {
    let model = common::compile_with_platform(Some(NIL_CLASS_SOURCE));
    let opts = Options::new("com.example.gen");
    let files = generate(
        &model,
        &[],
        &[FqnInput::new("user_test::HasNilProp")],
        &[],
        &opts,
    )
    .expect("codegen succeeds");

    let iface = files
        .iter()
        .find(|f| f.relative_path.ends_with("HasNilProp.java"))
        .expect("HasNilProp.java emitted");
    let src = &iface.contents;

    assert!(
        src.contains("Iterable<Void> nada()"),
        "Nil[*] must render as Iterable<Void>: {src}"
    );
    assert!(
        src.contains("java.util.Optional<Void> maybeNada()"),
        "Nil[0..1] must render as Optional<Void>: {src}"
    );
    assert!(
        !src.contains("com.example.gen.meta.pure.metamodel.type.Nil"),
        "Nil should never appear as a generated FQN — got: {src}"
    );
}
