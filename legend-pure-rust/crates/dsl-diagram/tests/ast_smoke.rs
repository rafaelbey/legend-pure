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

//! Locks the `DSLElement` impl on `DiagramDef` — wrapping in
//! `Element::DSLElement` and round-tripping through `Element::clone()`
//! / `PartialEq` must preserve identity.

use legend_pure_dsl_diagram::ast::DiagramDef;
use legend_pure_parser_ast::annotation::SpannedString;
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::element::Element;
use legend_pure_parser_ast::source_info::SourceInfo;
use smol_str::SmolStr;

fn dummy_si() -> SourceInfo {
    SourceInfo::new("smoke.pure", 1, 1, 1, 8)
}

fn dummy_diagram(name: &str) -> DiagramDef {
    DiagramDef {
        package: None,
        name: SpannedString {
            value: SmolStr::new(name),
            source_info: dummy_si(),
        },
        geometry: None,
        views: Vec::new(),
        stereotypes: Vec::new(),
        tagged_values: Vec::new(),
        source_info: dummy_si(),
    }
}

#[test]
fn diagram_def_implements_dsl_element_kind() {
    let d = dummy_diagram("MyDiagram");
    let kind = <DiagramDef as DSLElement>::kind(&d);
    assert_eq!(kind, "Diagram");
}

#[test]
fn diagram_def_round_trips_through_element_dsl_element_variant() {
    let original = dummy_diagram("MyDiagram");
    let wrapped = Element::DSLElement(Box::new(original.clone()));

    // Clone should produce an equal Element via clone_box.
    let cloned = wrapped.clone();
    assert_eq!(
        wrapped, cloned,
        "Element::clone failed to preserve DSLElement"
    );

    // Downcast should recover the concrete DiagramDef.
    if let Element::DSLElement(boxed) = &cloned {
        let recovered = boxed
            .as_any()
            .downcast_ref::<DiagramDef>()
            .expect("downcast to DiagramDef");
        assert_eq!(recovered, &original);
    } else {
        panic!("clone produced wrong variant");
    }
}

#[test]
fn diagram_def_equality_distinguishes_different_names() {
    let a = Element::DSLElement(Box::new(dummy_diagram("A")));
    let b = Element::DSLElement(Box::new(dummy_diagram("B")));
    assert_ne!(a, b);
}
