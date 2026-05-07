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

//! **Platform compile cleanliness gate.**
//!
//! Asserts the embedded platform compiles with zero errors. The
//! historical `strict_mode_platform_cost.rs` test that measured
//! strict-vs-default divergence is gone — strict-mode behaviours are
//! now the default (the "deliberate divergence over Java" set is
//! always-on). This test pins the resulting compile-clean invariant.

#[test]
fn platform_compiles_clean() {
    match legend_pure_core_platform::platform::load_platform() {
        Ok(_) => {}
        Err(p) => {
            for e in &p.errors {
                eprintln!(
                    "  - {} @ {}:{}",
                    e.message, e.source_info.source, e.source_info.start_line
                );
            }
            panic!(
                "Platform must compile with zero errors; got {}",
                p.errors.len()
            );
        }
    }
}
