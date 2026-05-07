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

//! **Strict-mode platform cost measurement.**
//!
//! Compiles the platform corpus under strict inference and reports
//! the number / categories of new errors compared to default mode.
//! Used to answer "what's the risk and cost of strict-mode-as-default?"
//! empirically.
//!
//! This test ALWAYS PASSES — it just prints the cost. Run with
//! `--nocapture` to see the breakdown.

use std::collections::HashMap;

#[test]
fn strict_mode_platform_cost_report() {
    println!("\n=== Default mode (Java parity) ===");
    let default_errors = match legend_pure_core_platform::platform::load_platform() {
        Ok(_) => {
            println!("Platform compiles cleanly (0 errors)");
            0
        }
        Err(p) => {
            println!("Platform: {} errors", p.errors.len());
            p.errors.len()
        }
    };

    println!("\n=== Strict mode ===");
    legend_pure_parser_pure::strict_mode::with_strict_mode(true, || {
        match legend_pure_core_platform::platform::load_platform() {
            Ok(_) => println!("Platform compiles cleanly under strict mode (0 errors)"),
            Err(p) => {
                let total = p.errors.len();
                println!("Platform: {total} errors");

                let mut by_kind: HashMap<String, usize> = HashMap::new();
                for e in &p.errors {
                    let k = format!("{:?}", e.kind);
                    let k = k.split(' ').next().unwrap_or("?");
                    let k = k.split('{').next().unwrap_or(k);
                    let k = k.trim_end_matches(':');
                    *by_kind.entry(k.to_string()).or_default() += 1;
                }
                println!("\nBy kind:");
                let mut v: Vec<_> = by_kind.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1));
                for (k, c) in &v {
                    println!("  {c:6}  {k}");
                }

                println!("\nFirst 8 messages:");
                for e in p.errors.iter().take(8) {
                    println!(
                        "  - {} @ {}:{}",
                        e.message, e.source_info.source, e.source_info.start_line
                    );
                }

                println!(
                    "\nDelta vs default: +{} errors under strict mode",
                    total - default_errors
                );
            }
        }
    });
}
