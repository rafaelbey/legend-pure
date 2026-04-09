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

//! Dense connectivity stress test: 10K classes with ~10 links per hub.

use legend_pure_parser_stress::generate::common::PhaseTimer;
use legend_pure_parser_stress::generate::hub_spoke::{self, HubSpokeConfig};

use legend_pure_parser_pure::model::Element;
use smol_str::SmolStr;

#[test]
fn stress_10k_dense() {
    let config = HubSpokeConfig::dense_10k();
    let title = "Dense 10K";

    println!();

    // Phase 0: Generate source
    let t0 = PhaseTimer::start("Phase 0 (generate source)");
    let (source, stats) = hub_spoke::generate(&config);
    t0.stop();

    stats.print(title);

    // Phase 1: Parse → AST
    let t1 = PhaseTimer::start("Phase 1 (parse → AST)");
    let source_file =
        legend_pure_parser_parser::parse(&source, "stress_dense.pure").expect("parse failed");
    t1.stop();

    let element_count = source_file.element_count();
    println!("  AST elements: {element_count}");

    // Phase 2: Compile → PureModel
    let t2 = PhaseTimer::start("Phase 2 (compile → model)");
    let model = legend_pure_parser_pure::compile!(&[source_file]).expect("compilation failed");
    t2.stop();

    // Phase 3: Compose roundtrip
    let t3 = PhaseTimer::start("Phase 3 (compose roundtrip)");
    let source_file_for_compose =
        legend_pure_parser_parser::parse(&source, "stress_dense_rt.pure").expect("re-parse");
    let composed = legend_pure_parser_compose::compose_source_file(&source_file_for_compose);
    let reparsed = legend_pure_parser_parser::parse(&composed, "stress_dense_roundtrip.pure")
        .expect("roundtrip parse failed");
    t3.stop();

    assert_eq!(
        source_file_for_compose.element_count(),
        reparsed.element_count(),
        "roundtrip element count mismatch"
    );
    println!("  Roundtrip: ✓ ({} elements)", reparsed.element_count());

    // Phase 4: Model assertions
    let t4 = PhaseTimer::start("Phase 4 (model assertions)");

    // Count elements
    let (mut classes, mut assocs) = (0, 0);
    for chunk in model.chunks.iter().skip(1) {
        for (_, element) in chunk.elements.iter() {
            match element {
                Element::Class(_) => classes += 1,
                Element::Association(_) => assocs += 1,
                _ => {}
            }
        }
    }

    assert_eq!(classes, stats.classes, "class count mismatch");
    assert_eq!(assocs, stats.associations, "association count mismatch");
    println!("  Model: {classes} classes, {assocs} associations");

    // Dense topology should have significantly more associations than hub count
    assert!(
        assocs > config.hubs * 5,
        "dense model should have >5x associations per hub, got {assocs} for {} hubs",
        config.hubs
    );

    // Spot-check: H0 should have many association-injected properties
    let h0_id = model
        .resolve_by_path(&[SmolStr::new("test"), SmolStr::new("H0")])
        .expect("H0 should exist");
    let h0_assoc_props = model.association_properties(h0_id);
    assert!(
        h0_assoc_props.len() >= 10,
        "H0 in dense model should have >=10 association properties, got {}",
        h0_assoc_props.len()
    );

    t4.stop();

    // Phase 5: Path resolution
    let t5 = PhaseTimer::start("Phase 5 (path resolution)");
    let mut resolved = 0;
    for h in 0..config.hubs {
        if model
            .resolve_by_path(&[SmolStr::new("test"), SmolStr::new(format!("H{h}"))])
            .is_some()
        {
            resolved += 1;
        }
    }
    let dur = t5.stop();
    assert_eq!(resolved, config.hubs, "not all hubs resolved");
    let us_per_lookup = dur.as_micros() / config.hubs as u128;
    println!("  Resolved {resolved} paths ({us_per_lookup} us/lookup)");

    println!("  === PASSED ===");
}
