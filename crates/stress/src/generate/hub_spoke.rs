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

//! Hub-spoke topology generator.
//!
//! Generates a model with:
//! - `hubs` hub classes (`H0..H{n-1}`) with configurable properties
//! - `hubs * sats_per_hub` satellite classes (`S0..S{m-1}`)
//! - Hub ring associations: H0→H1→...→H{n-1}→H0
//! - Cross-link associations at configurable skip offsets
//! - Satellite→Hub associations
//! - Optional 3-level inheritance chains on every 10th hub
//! - Optional profiles, stereotypes, tagged values
//! - Optional enumerations with enum-typed properties
//! - Optional simple functions

use crate::generate::common::ModelStats;

/// Configuration for hub-spoke model generation.
pub struct HubSpokeConfig {
    /// Number of hub classes.
    pub hubs: usize,
    /// Satellite classes per hub.
    pub sats_per_hub: usize,
    /// Cross-link skip offsets (e.g., `[5]` links every 10th hub to hub+5).
    pub skip_offsets: Vec<usize>,
    /// Inheritance depth for every 10th hub (0 = flat, 3 = Base→Mid→H chain).
    pub inheritance_depth: usize,
    /// Include profile definitions and annotations on classes.
    pub include_profiles: bool,
    /// Include enum definitions and enum-typed properties.
    pub include_enums: bool,
    /// Include simple function definitions.
    pub include_functions: bool,
    /// Include measure + unit definitions.
    pub include_measures: bool,
}

impl HubSpokeConfig {
    /// Standard 1K config.
    #[must_use]
    pub fn standard_1k() -> Self {
        Self {
            hubs: 100,
            sats_per_hub: 9,
            skip_offsets: vec![5],
            inheritance_depth: 3,
            include_profiles: true,
            include_enums: true,
            include_functions: true,
            include_measures: true,
        }
    }

    /// Standard 10K config.
    #[must_use]
    pub fn standard_10k() -> Self {
        Self {
            hubs: 1_000,
            sats_per_hub: 9,
            skip_offsets: vec![5],
            inheritance_depth: 3,
            include_profiles: true,
            include_enums: true,
            include_functions: true,
            include_measures: true,
        }
    }

    /// Standard 100K config.
    #[must_use]
    pub fn standard_100k() -> Self {
        Self {
            hubs: 10_000,
            sats_per_hub: 9,
            skip_offsets: vec![5],
            inheritance_depth: 3,
            include_profiles: true,
            include_enums: true,
            include_functions: true,
            include_measures: true,
        }
    }

    /// Dense 10K config — more cross-links per hub.
    #[must_use]
    pub fn dense_10k() -> Self {
        Self {
            hubs: 1_000,
            sats_per_hub: 9,
            skip_offsets: vec![2, 3, 5, 7, 11, 13, 17, 19, 23],
            inheritance_depth: 3,
            include_profiles: true,
            include_enums: true,
            include_functions: true,
            include_measures: false,
        }
    }

    /// Total number of classes (hubs + satellites + inheritance bases).
    #[must_use]
    pub fn total_classes(&self) -> usize {
        let base = self.hubs + self.hubs * self.sats_per_hub;
        if self.inheritance_depth > 0 {
            // Every 10th hub gets (inheritance_depth - 1) base classes
            // (the hub itself is the leaf, so we add depth-1 ancestors)
            let chains = self.hubs / 10;
            base + chains * (self.inheritance_depth - 1)
        } else {
            base
        }
    }
}

/// Generates Pure grammar source for a hub-spoke model.
///
/// Returns `(source_text, stats)`.
#[allow(clippy::too_many_lines)]
pub fn generate(config: &HubSpokeConfig) -> (String, ModelStats) {
    let hubs = config.hubs;
    let sats = hubs * config.sats_per_hub;
    let total_estimate = config.total_classes();

    let mut sb = String::with_capacity(total_estimate * 400);
    let mut stats = ModelStats::default();

    // ---- Profiles ----
    if config.include_profiles {
        sb.push_str("Profile test::doc {\n");
        sb.push_str("  stereotypes: [deprecated, internal, experimental];\n");
        sb.push_str("  tags: [description, owner, version];\n");
        sb.push_str("}\n\n");

        sb.push_str("Profile test::meta {\n");
        sb.push_str("  stereotypes: [generated, synthetic];\n");
        sb.push_str("  tags: [source, tier];\n");
        sb.push_str("}\n\n");

        stats.profiles = 2;
    }

    // ---- Enumerations ----
    if config.include_enums {
        sb.push_str("Enum test::Status {\n");
        sb.push_str("  ACTIVE,\n");
        sb.push_str("  INACTIVE,\n");
        sb.push_str("  PENDING,\n");
        sb.push_str("  ARCHIVED\n");
        sb.push_str("}\n\n");

        sb.push_str("Enum test::Priority {\n");
        sb.push_str("  HIGH,\n");
        sb.push_str("  MEDIUM,\n");
        sb.push_str("  LOW,\n");
        sb.push_str("  CRITICAL,\n");
        sb.push_str("  NONE\n");
        sb.push_str("}\n\n");

        stats.enums = 2;
    }

    // ---- Measures ----
    if config.include_measures {
        sb.push_str("Measure test::Distance {\n");
        sb.push_str("  *Meter: x -> $x;\n");
        sb.push_str("  Kilometer: x -> $x * 1000;\n");
        sb.push_str("  Mile: x -> $x * 1609.344;\n");
        sb.push_str("}\n\n");
        stats.measures = 1;
    }

    // ---- Inheritance base/mid classes for every 10th hub ----
    let chain_count = if config.inheritance_depth > 1 {
        let chains = hubs / 10;
        for c in 0..chains {
            let h = c * 10;
            // Depth 3 => Base_H{h} -> Mid_H{h} -> H{h}
            // Depth 2 => Base_H{h} -> H{h}
            // Generate ancestors from root down
            if config.inheritance_depth >= 3 {
                emit_class_decl(
                    &mut sb,
                    &format!("test::Base_H{h}"),
                    None,
                    &[("baseField", "String"), ("baseScore", "Integer")],
                    config.include_profiles,
                    config.include_enums,
                    false,
                    h,
                );
                stats.classes += 1;
            }
            if config.inheritance_depth >= 2 {
                let parent = if config.inheritance_depth >= 3 {
                    Some(format!("test::Base_H{h}"))
                } else {
                    None
                };
                emit_class_decl(
                    &mut sb,
                    &format!("test::Mid_H{h}"),
                    parent.as_deref(),
                    &[("midField", "String"), ("midValue", "Integer")],
                    false,
                    false,
                    false,
                    h,
                );
                stats.classes += 1;
            }
        }
        chains
    } else {
        0
    };
    stats.inheritance_chains = chain_count;

    // ---- Hub classes ----
    for h in 0..hubs {
        let parent = if config.inheritance_depth >= 2 && h % 10 == 0 {
            if config.inheritance_depth >= 3 {
                Some(format!("test::Mid_H{h}"))
            } else {
                Some(format!("test::Base_H{h}"))
            }
        } else {
            None
        };

        let class_name = format!("test::H{h}");
        let props = vec![
            ("id", "Integer"),
            ("name", "String"),
            ("code", "String"),
            ("score", "Integer"),
            ("fullLabel", "String"),
        ];

        // Every 5th hub gets enum-typed props
        let has_enum = config.include_enums && h % 5 == 0;

        // Build the class
        emit_class_decl(
            &mut sb,
            &class_name,
            parent.as_deref(),
            &props,
            config.include_profiles && h % 3 == 0,
            has_enum,
            true, // include extra hub props
            h,
        );
        stats.classes += 1;
    }

    // ---- Satellite classes ----
    for s in 0..sats {
        let class_name = format!("test::S{s}");
        emit_class_decl(
            &mut sb,
            &class_name,
            None,
            &[("id", "Integer"), ("label", "String"), ("value", "Integer")],
            false,
            false,
            false,
            s,
        );
        stats.classes += 1;
    }

    // ---- Associations ----
    let mut assoc_count = 0;

    // Hub ring: H0→H1, H1→H2, ..., H{n-1}→H0
    for h in 0..hubs {
        let next = (h + 1) % hubs;
        sb.push_str(&format!(
            "Association test::HubRing{h} {{\n  nextHub{h}: test::H{next}[0..1];\n  prevHub{h}: test::H{h}[0..1];\n}}\n\n"
        ));
        assoc_count += 1;
    }

    // Cross-links at skip offsets (every 10th hub)
    for h in (0..hubs).step_by(10) {
        for &offset in &config.skip_offsets {
            let target = (h + offset) % hubs;
            sb.push_str(&format!(
                "Association test::HubCross{h}_{offset} {{\n  crossTo{h}_{offset}: test::H{target}[0..1];\n  crossFrom{h}_{offset}: test::H{h}[0..1];\n}}\n\n"
            ));
            assoc_count += 1;
        }
    }

    // Satellite→Hub
    for s in 0..sats {
        let hub = s / config.sats_per_hub;
        sb.push_str(&format!(
            "Association test::SatHub{s} {{\n  hub{s}: test::H{hub}[0..1];\n  sat{s}: test::S{s}[0..1];\n}}\n\n"
        ));
        assoc_count += 1;
    }
    stats.associations = assoc_count;

    // ---- Functions ----
    if config.include_functions {
        // A few simple functions to stress function compilation
        let fn_count = hubs / 20;
        for f in 0..fn_count {
            sb.push_str(&format!(
                "function test::greet{f}(name: String[1]): String[1]\n{{\n  'hello ' + $name\n}}\n\n"
            ));
        }
        stats.functions = fn_count;
    }

    stats.source_bytes = sb.len();
    (sb, stats)
}

/// Emits a single class declaration.
#[allow(clippy::fn_params_excessive_bools)]
fn emit_class_decl(
    sb: &mut String,
    fqn: &str,
    parent: Option<&str>,
    props: &[(&str, &str)],
    annotate: bool,
    include_status_prop: bool,
    _is_hub: bool,
    index: usize,
) {
    // Stereotype annotation
    if annotate {
        let stereo = match index % 3 {
            0 => "test::doc.internal",
            1 => "test::doc.experimental",
            _ => "test::meta.generated",
        };
        sb.push_str(&format!("Class <<{stereo}>> "));
    } else {
        sb.push_str("Class ");
    }

    sb.push_str(fqn);

    if let Some(p) = parent {
        sb.push_str(" extends ");
        sb.push_str(p);
    }

    sb.push_str("\n{\n");

    for (name, typ) in props {
        sb.push_str(&format!("  {name}: {typ}[1];\n"));
    }

    // Enum-typed property
    if include_status_prop {
        sb.push_str("  status: test::Status[1];\n");
    }

    sb.push_str("}\n\n");
}
