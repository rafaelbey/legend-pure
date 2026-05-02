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

//! Thin CLI wrapper over `legend_pure_snapshot_builder::compile_to_purem`.
//!
//! ```text
//! legend-pure-snapshot-builder \
//!     --target platform_dsl_mapping \
//!     --output ./platform_dsl_mapping.purem \
//!     --descriptor /path/to/platform.json \
//!     --descriptor /path/to/platform_dsl_store.json \
//!     --descriptor /path/to/platform_dsl_mapping.json
//! ```
//!
//! Most callers should invoke the library directly from a `build.rs`
//! script. This binary exists for ad-hoc smoke testing.

use std::path::PathBuf;
use std::process::ExitCode;

use legend_pure_snapshot_builder::{
    CompileRequest, DEFAULT_PLATFORM_AUTO_IMPORTS, compile_to_purem,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut descriptors: Vec<PathBuf> = Vec::new();
    let mut target: Option<String> = None;
    let mut output: Option<PathBuf> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--descriptor" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--descriptor needs a value");
                    return ExitCode::from(2);
                };
                descriptors.push(PathBuf::from(v));
                i += 2;
            }
            "--target" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--target needs a value");
                    return ExitCode::from(2);
                };
                target = Some(v.clone());
                i += 2;
            }
            "--output" => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("--output needs a value");
                    return ExitCode::from(2);
                };
                output = Some(PathBuf::from(v));
                i += 2;
            }
            other => {
                eprintln!("unknown arg: {other}");
                return ExitCode::from(2);
            }
        }
    }

    let Some(target) = target else {
        eprintln!("--target is required");
        return ExitCode::from(2);
    };
    let Some(output) = output else {
        eprintln!("--output is required");
        return ExitCode::from(2);
    };

    let req = CompileRequest {
        descriptors: &descriptors,
        target: &target,
        output: &output,
        auto_imports: DEFAULT_PLATFORM_AUTO_IMPORTS,
    };
    if let Err(e) = compile_to_purem(req) {
        eprintln!("snapshot-builder failed: {e}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
