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

//! Chaotic (non-uniform) model generator.
//!
//! Generates a model with heterogeneous class shapes:
//! - **Tiny** (1–3 props): 40%
//! - **Small** (4–8 props): 25%
//! - **Medium** (9–20 props): 20%
//! - **Large** (21–35 props): 10%
//! - **Huge** (36–50 props): 5%
//!
//! Features all 7 Pure types, random association graph, enum properties,
//! function definitions, profiles with annotations, and optional
//! inheritance hierarchies.

use crate::generate::common::{self, ModelStats};

/// Configuration for chaotic model generation.
pub struct ChaoticConfig {
    /// Total number of classes.
    pub total_classes: usize,
    /// Include profiles and annotations on a percentage of classes.
    pub include_profiles: bool,
    /// Include enum properties on a percentage of classes.
    pub include_enums: bool,
    /// Include simple functions.
    pub include_functions: bool,
    /// Include inheritance: ~5% of classes extend another class.
    pub include_inheritance: bool,
}

impl ChaoticConfig {
    /// Standard 100K config.
    #[must_use]
    pub fn standard_100k() -> Self {
        Self {
            total_classes: 100_000,
            include_profiles: true,
            include_enums: true,
            include_functions: true,
            include_inheritance: true,
        }
    }
}

/// Metadata about a generated class.
#[allow(dead_code)] // Fields retained for future query generation phase
struct ClassInfo {
    index: usize,
    prop_count: usize,
    props: Vec<(String, String)>,
    has_enum: bool,
    has_parent: bool,
    parent_idx: Option<usize>,
}

/// Generates Pure grammar source for a chaotic non-uniform model.
///
/// Returns `(source_text, stats)`.
#[allow(clippy::too_many_lines)]
pub fn generate(config: &ChaoticConfig) -> (String, ModelStats) {
    let n = config.total_classes;
    let mut sb = String::with_capacity(n * 300);
    let mut stats = ModelStats::default();

    // ---- Profiles ----
    if config.include_profiles {
        sb.push_str("Profile test::doc {\n");
        sb.push_str("  stereotypes: [deprecated, internal, experimental];\n");
        sb.push_str("  tags: [description, owner, version];\n");
        sb.push_str("}\n\n");

        sb.push_str("Profile test::meta {\n");
        sb.push_str("  stereotypes: [generated, synthetic, chaos];\n");
        sb.push_str("  tags: [source, tier, shape];\n");
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

    // ---- Pre-compute class metadata ----
    let mut class_infos: Vec<ClassInfo> = Vec::with_capacity(n);
    let (mut tiny, mut small, mut med, mut large, mut huge) = (0, 0, 0, 0, 0);

    for i in 0..n {
        let h = common::det_hash((i as u32).wrapping_mul(7).wrapping_add(13));

        // Property count distribution: 40% tiny, 25% small, 20% med, 10% large, 5% huge
        let bucket = h % 100;
        let prop_count = if bucket < 40 {
            tiny += 1;
            1 + (common::det_hash(i as u32 * 3) % 3) as usize
        } else if bucket < 65 {
            small += 1;
            4 + (common::det_hash(i as u32 * 5) % 5) as usize
        } else if bucket < 85 {
            med += 1;
            9 + (common::det_hash(i as u32 * 11) % 12) as usize
        } else if bucket < 95 {
            large += 1;
            21 + (common::det_hash(i as u32 * 17) % 15) as usize
        } else {
            huge += 1;
            36 + (common::det_hash(i as u32 * 23) % 15) as usize
        };

        let mut props = Vec::with_capacity(prop_count + 2);

        // Everyone gets id
        props.push(("id".to_string(), "Integer".to_string()));

        for p in 0..prop_count {
            let type_idx = common::det_hash(i as u32 * 100 + p as u32 * 13) as usize
                % common::PURE_TYPES.len();
            let pure_type = common::PURE_TYPES[type_idx];
            let stem_idx =
                common::det_hash(i as u32 * 50 + p as u32 * 7) as usize % common::PROP_STEMS.len();
            let stem = common::PROP_STEMS[stem_idx];
            props.push((format!("{stem}{p}"), pure_type.to_string()));
        }

        // ~5% get an enum property
        let has_enum =
            config.include_enums && (common::det_hash(i as u32 * 41) % 20 == 0) && prop_count >= 2;
        if has_enum {
            props.push(("prio".to_string(), "test::Priority".to_string()));
        }

        // ~5% inherit from a previous class (only if i > 10 and include_inheritance)
        let has_parent =
            config.include_inheritance && i > 10 && common::det_hash(i as u32 * 67) % 20 == 0;
        let parent_idx = if has_parent {
            // Pick a deterministic parent from the first quarter of classes
            let target = common::det_hash(i as u32 * 71) as usize % (i / 2).max(1);
            Some(target)
        } else {
            None
        };

        class_infos.push(ClassInfo {
            index: i,
            prop_count,
            props,
            has_enum,
            has_parent,
            parent_idx,
        });
    }

    // ---- Emit classes ----
    // First pass: emit classes that are parents (no parent themselves) to avoid forward refs.
    // Since our compiler does topo sort, order doesn't strictly matter for correctness,
    // but emitting parents first is cleaner.
    for ci in &class_infos {
        let annotate = config.include_profiles && ci.index % 7 == 0;
        if annotate {
            let stereo = match ci.index % 6 {
                0 => "test::doc.deprecated",
                1 => "test::doc.internal",
                2 => "test::doc.experimental",
                3 => "test::meta.generated",
                4 => "test::meta.synthetic",
                _ => "test::meta.chaos",
            };
            sb.push_str(&format!("Class <<{stereo}>> "));
        } else {
            sb.push_str("Class ");
        }

        sb.push_str(&format!("test::C{}", ci.index));

        if let Some(parent_idx) = ci.parent_idx {
            sb.push_str(&format!(" extends test::C{parent_idx}"));
            stats.inheritance_chains += 1;
        }

        sb.push_str("\n{\n");
        for (name, typ) in &ci.props {
            sb.push_str(&format!("  {name}: {typ}[1];\n"));
        }
        sb.push_str("}\n\n");
        stats.classes += 1;
    }

    // ---- Associations: 1 link per class ----
    for i in 0..n {
        let mut target = common::det_hash(i as u32 * 97 + 53) as usize % n;
        if target == i {
            target = (target + 1) % n;
        }
        sb.push_str(&format!(
            "Association test::L{i} {{\n  to{i}: test::C{target}[0..1];\n  from{i}: test::C{i}[0..1];\n}}\n\n"
        ));
        stats.associations += 1;
    }

    // ---- Functions ----
    if config.include_functions {
        let fn_count = n / 500;
        for f in 0..fn_count {
            sb.push_str(&format!(
                "function test::fn{f}(x: Integer[1]): Integer[1]\n{{\n  $x + 1\n}}\n\n"
            ));
        }
        stats.functions = fn_count;
    }

    println!(
        "  Shape distribution: tiny={tiny}, small={small}, med={med}, large={large}, huge={huge}"
    );

    stats.source_bytes = sb.len();
    (sb, stats)
}
