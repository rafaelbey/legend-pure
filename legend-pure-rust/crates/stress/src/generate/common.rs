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

//! Shared helpers for stress test generators.

use std::time::Instant;

/// Deterministic hash — reproducible across runs.
///
/// This is the same hash function used in the Java `StressTestChaotic` for
/// consistency: a splitmix-style integer hash that distributes well.
#[must_use]
pub fn det_hash(seed: u32) -> u32 {
    let mut s = seed;
    s = (s >> 16 ^ s).wrapping_mul(0x45d9_f3b);
    s = (s >> 16 ^ s).wrapping_mul(0x45d9_f3b);
    s = (s >> 16) ^ s;
    s & 0x7FFF_FFFF
}

/// Pure primitive types and their display names.
pub const PURE_TYPES: &[&str] = &[
    "String", "Integer", "Boolean", "Date", "DateTime", "Float", "Decimal",
];

/// Short property name stems — used to generate unique property names.
pub const PROP_STEMS: &[&str] = &[
    "nm", "lb", "cd", "ds", "tg", "nt", "tt", "rf", "ct", "st", "rg", "kd", "sr", "mm", "pf", "am",
    "qt", "sc", "rk", "lv", "cn", "wt", "pr", "vr", "rt", "sz", "dp", "ht", "wd", "ag", "fl", "ac",
    "cr", "up", "ln", "ra", "px", "to", "bl", "mg", "of", "sp", "du", "al", "be", "ga", "de", "ep",
    "ze", "th",
];

/// A phase timing measurement.
pub struct PhaseTimer {
    label: &'static str,
    start: Instant,
}

impl PhaseTimer {
    /// Starts a new phase timer.
    #[must_use]
    pub fn start(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
        }
    }

    /// Stops the timer and prints the phase duration to stdout.
    pub fn stop(self) -> std::time::Duration {
        let elapsed = self.start.elapsed();
        let ms = elapsed.as_millis();
        println!("  {}: {} ms", self.label, ms);
        elapsed
    }
}

/// Summary statistics printed after a stress test completes.
#[derive(Debug, Default)]
pub struct ModelStats {
    /// Number of classes.
    pub classes: usize,
    /// Number of enumerations.
    pub enums: usize,
    /// Number of associations.
    pub associations: usize,
    /// Number of functions.
    pub functions: usize,
    /// Number of profiles.
    pub profiles: usize,
    /// Number of measures.
    pub measures: usize,
    /// Number of inheritance chains.
    pub inheritance_chains: usize,
    /// Source size in bytes.
    pub source_bytes: usize,
}

impl ModelStats {
    /// Prints a summary of the generated model.
    pub fn print(&self, title: &str) {
        println!("=== STRESS TEST: {} ===", title);
        println!(
            "  Model: {} classes, {} enums, {} assocs, {} functions, {} profiles, {} measures",
            self.classes,
            self.enums,
            self.associations,
            self.functions,
            self.profiles,
            self.measures,
        );
        if self.inheritance_chains > 0 {
            println!("  Inheritance chains: {}", self.inheritance_chains);
        }
        println!("  Pure source: {} KB", self.source_bytes / 1024);
    }
}
