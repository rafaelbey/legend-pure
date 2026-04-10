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

//! Chaotic 100K stress test — non-uniform class shapes, all types.

#[cfg(feature = "heavy")]
use legend_pure_parser_stress::generate::chaotic::{self, ChaoticConfig};
#[cfg(feature = "heavy")]
use legend_pure_parser_stress::generate::common::PhaseTimer;

#[cfg(feature = "heavy")]
use legend_pure_parser_pure::model::Element;
#[cfg(feature = "heavy")]
use smol_str::SmolStr;

#[test]
#[cfg(feature = "heavy")]
fn stress_100k_chaotic() {
    let config = ChaoticConfig::standard_100k();
    let title = "Chaotic 100K";

    println!();

    // Phase 0: Generate source
    let t0 = PhaseTimer::start("Phase 0 (generate source)");
    let (source, stats) = chaotic::generate(&config);
    t0.stop();

    stats.print(title);

    // Phase 1: Parse → AST
    let t1 = PhaseTimer::start("Phase 1 (parse → AST)");
    let source_file =
        legend_pure_parser_parser::parse(&source, "stress_chaotic.pure").expect("parse failed");
    t1.stop();

    let element_count = source_file.element_count();
    println!("  AST elements: {element_count}");

    // Phase 2: Compile → PureModel
    let t2 = PhaseTimer::start("Phase 2 (compile → model)");
    let model = legend_pure_parser_pure::compile!(&[source_file]).expect("compilation failed");
    t2.stop();

    // Phase 3: Compose roundtrip (sample — full 100K roundtrip may be slow)
    // For the chaotic test, we skip full roundtrip and just measure compose time.
    let t3 = PhaseTimer::start("Phase 3 (compose)");
    let source_file_for_compose =
        legend_pure_parser_parser::parse(&source, "stress_chaotic_rt.pure").expect("re-parse");
    let composed = legend_pure_parser_compose::compose_source_file(&source_file_for_compose);
    t3.stop();
    println!("  Composed output: {} KB", composed.len() / 1024);

    // Phase 4: Model assertions
    let t4 = PhaseTimer::start("Phase 4 (model assertions)");

    // Count elements (skip chunk 0 = bootstrap)
    let (mut classes, mut enums, mut assocs, mut functions, mut profiles) = (0, 0, 0, 0, 0);
    for chunk in model.chunks.iter().skip(1) {
        for (_, element) in chunk.elements.iter() {
            match element {
                Element::Class(_) => classes += 1,
                Element::Enumeration(_) => enums += 1,
                Element::Association(_) => assocs += 1,
                Element::Function(_) => functions += 1,
                Element::Profile(_) => profiles += 1,
                _ => {}
            }
        }
    }

    assert_eq!(classes, stats.classes, "class count mismatch");
    assert_eq!(enums, stats.enums, "enum count mismatch");
    assert_eq!(assocs, stats.associations, "association count mismatch");
    assert_eq!(functions, stats.functions, "function count mismatch");
    assert_eq!(profiles, stats.profiles, "profile count mismatch");

    println!(
        "  Model: {classes} classes, {enums} enums, {assocs} assocs, {functions} functions, {profiles} profiles"
    );

    // Inheritance check — some classes should have super types
    if config.include_inheritance {
        let mut classes_with_parents = 0;
        for chunk in model.chunks.iter().skip(1) {
            for (_, element) in chunk.elements.iter() {
                if let Element::Class(class) = element {
                    if !class.super_types.is_empty() {
                        classes_with_parents += 1;
                    }
                }
            }
        }
        println!("  Classes with inheritance: {classes_with_parents}");
        assert!(
            classes_with_parents > 0,
            "chaotic model with include_inheritance should have some classes with parents"
        );
        assert_eq!(
            classes_with_parents, stats.inheritance_chains,
            "inheritance chain count mismatch"
        );
    }

    // Spot-check: C0 exists
    let c0_id = model
        .resolve_by_path(&[SmolStr::new("test"), SmolStr::new("C0")])
        .expect("C0 should exist");
    assert!(
        matches!(model.get_element(c0_id), Element::Class(_)),
        "C0 should be a Class"
    );

    t4.stop();

    // Phase 5: Path resolution benchmark
    let t5 = PhaseTimer::start("Phase 5 (path resolution)");
    let sample_size = config.total_classes.min(10_000);
    let mut resolved = 0;
    for i in 0..sample_size {
        if model
            .resolve_by_path(&[SmolStr::new("test"), SmolStr::new(format!("C{i}"))])
            .is_some()
        {
            resolved += 1;
        }
    }
    let dur = t5.stop();
    assert_eq!(resolved, sample_size, "not all classes resolved");
    let us_per_lookup = dur.as_micros() / sample_size as u128;
    println!(
        "  Resolved {resolved}/{} paths ({us_per_lookup} us/lookup)",
        config.total_classes,
    );

    println!("  === PASSED ===");
}
