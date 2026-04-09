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

//! Hub-spoke stress tests: 1K, 10K, 100K.

use legend_pure_parser_stress::generate::common::PhaseTimer;
use legend_pure_parser_stress::generate::hub_spoke::{self, HubSpokeConfig};

use legend_pure_parser_pure::model::Element;
use smol_str::SmolStr;

/// Runs the full pipeline for a hub-spoke model and validates the result.
fn run_hub_spoke(title: &str, config: HubSpokeConfig) {
    println!();

    // Phase 0: Generate source
    let t0 = PhaseTimer::start("Phase 0 (generate source)");
    let (source, stats) = hub_spoke::generate(&config);
    t0.stop();

    stats.print(title);

    // Phase 1: Parse → AST
    let t1 = PhaseTimer::start("Phase 1 (parse → AST)");
    let source_file =
        legend_pure_parser_parser::parse(&source, "stress.pure").expect("parse failed");
    t1.stop();

    let element_count = source_file.element_count();
    println!("  AST elements: {element_count}");

    // Phase 2: Compile → PureModel
    let t2 = PhaseTimer::start("Phase 2 (compile → model)");
    let model = legend_pure_parser_pure::compile!(&[source_file]).expect("compilation failed");
    t2.stop();

    // Phase 3: Compose roundtrip
    // Re-parse the composed text to verify the roundtrip is lossless at the
    // structural level (element count must match).
    let t3 = PhaseTimer::start("Phase 3 (compose roundtrip)");
    let source_file_for_compose =
        legend_pure_parser_parser::parse(&source, "stress_rt.pure").expect("re-parse for compose");
    let composed = legend_pure_parser_compose::compose_source_file(&source_file_for_compose);
    let reparsed = legend_pure_parser_parser::parse(&composed, "stress_roundtrip.pure")
        .expect("roundtrip parse failed");
    t3.stop();

    assert_eq!(
        source_file_for_compose.element_count(),
        reparsed.element_count(),
        "roundtrip element count mismatch: original={} vs roundtrip={}",
        source_file_for_compose.element_count(),
        reparsed.element_count(),
    );
    println!("  Roundtrip: ✓ ({} elements)", reparsed.element_count());

    // Phase 4: Model assertions
    let t4 = PhaseTimer::start("Phase 4 (model assertions)");
    validate_model(&model, &config, &stats);
    t4.stop();

    // Phase 5: Path resolution benchmark
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
    let us_per_lookup = if config.hubs > 0 {
        dur.as_micros() / config.hubs as u128
    } else {
        0
    };
    println!("  Resolved {resolved} paths ({us_per_lookup} us/lookup)");

    println!("  === PASSED ===");
}

/// Validates the compiled model matches expectations.
fn validate_model(
    model: &legend_pure_parser_pure::model::PureModel,
    config: &HubSpokeConfig,
    stats: &legend_pure_parser_stress::generate::common::ModelStats,
) {
    // Count elements by type (skip chunk 0 = bootstrap)
    let (mut classes, mut enums, mut assocs, mut functions, mut profiles, mut measures, mut units) =
        (0, 0, 0, 0, 0, 0, 0);

    for chunk in model.chunks.iter().skip(1) {
        for (_, element) in chunk.elements.iter() {
            match element {
                Element::Class(_) => classes += 1,
                Element::Enumeration(_) => enums += 1,
                Element::Association(_) => assocs += 1,
                Element::Function(_) => functions += 1,
                Element::Profile(_) => profiles += 1,
                Element::Measure(_) => measures += 1,
                Element::Unit(_) => units += 1,
                Element::PrimitiveType(_) => {} // bootstrap only
            }
        }
    }

    assert_eq!(classes, stats.classes, "class count mismatch");
    assert_eq!(enums, stats.enums, "enum count mismatch");
    assert_eq!(assocs, stats.associations, "association count mismatch");
    assert_eq!(functions, stats.functions, "function count mismatch");
    assert_eq!(profiles, stats.profiles, "profile count mismatch");
    assert_eq!(measures, stats.measures, "measure count mismatch");

    println!(
        "  Model: {classes} classes, {enums} enums, {assocs} assocs, \
         {functions} functions, {profiles} profiles, {measures} measures, {units} units"
    );

    // Spot-check: first hub exists and has properties
    let h0_id = model
        .resolve_by_path(&[SmolStr::new("test"), SmolStr::new("H0")])
        .expect("H0 should exist");
    match model.get_element(h0_id) {
        Element::Class(class) => {
            assert!(
                class.properties.len() >= 5,
                "H0 should have at least 5 properties, got {}",
                class.properties.len()
            );

            // Check inheritance on H0 (should extend Mid_H0 if depth >= 3)
            if config.inheritance_depth >= 2 {
                assert!(
                    !class.super_types.is_empty(),
                    "H0 should have a super type (inheritance_depth={})",
                    config.inheritance_depth
                );
            }
        }
        other => panic!("H0 should be a Class, got {other:?}"),
    }

    // Spot-check enums
    if config.include_enums {
        let status_id = model
            .resolve_by_path(&[SmolStr::new("test"), SmolStr::new("Status")])
            .expect("Status enum should exist");
        match model.get_element(status_id) {
            Element::Enumeration(e) => {
                assert_eq!(e.values.len(), 4, "Status should have 4 values");
            }
            other => panic!("Status should be Enumeration, got {other:?}"),
        }
    }

    // Spot-check profiles
    if config.include_profiles {
        let doc_id = model
            .resolve_by_path(&[SmolStr::new("test"), SmolStr::new("doc")])
            .expect("doc profile should exist");
        match model.get_element(doc_id) {
            Element::Profile(p) => {
                assert_eq!(
                    p.stereotypes.len(),
                    3,
                    "doc profile should have 3 stereotypes"
                );
                assert_eq!(p.tags.len(), 3, "doc profile should have 3 tags");
            }
            other => panic!("doc should be Profile, got {other:?}"),
        }
    }

    // Spot-check specialization index (if inheritance is used)
    if config.inheritance_depth >= 3 {
        let base_id = model
            .resolve_by_path(&[SmolStr::new("test"), SmolStr::new("Base_H0")])
            .expect("Base_H0 should exist");
        let specs = model.specializations(base_id);
        assert!(
            !specs.is_empty(),
            "Base_H0 should have specializations in the derived index"
        );
    }

    // Spot-check association property index
    let h0_assoc_props = model.association_properties(h0_id);
    assert!(
        !h0_assoc_props.is_empty(),
        "H0 should have association-injected properties"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn stress_1k_hub_spoke() {
    run_hub_spoke("Hub-Spoke 1K", HubSpokeConfig::standard_1k());
}

#[test]
fn stress_10k_hub_spoke() {
    run_hub_spoke("Hub-Spoke 10K", HubSpokeConfig::standard_10k());
}

#[test]
#[cfg(feature = "heavy")]
fn stress_100k_hub_spoke() {
    run_hub_spoke("Hub-Spoke 100K", HubSpokeConfig::standard_100k());
}
