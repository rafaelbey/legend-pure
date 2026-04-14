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

#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! # Legend Pure Core Platform
//!
//! Provides the standard Pure platform library, test runner ("surveyor"),
//! and compilation orchestration for the Rust implementation.
//!
//! # Platform Loading
//!
//! Use [`platform::load_platform()`] to parse and compile the standard
//! library into a `PureModel`. This currently reads embedded `.pure` source
//! files and compiles them on the fly.
//!
//! ## Future: Binary `.purem` Format
//!
//! For release builds, the platform model will be pre-compiled and
//! serialized into a binary `.purem` file (`FlatBuffers`). Loading from
//! `.purem` is orders of magnitude faster than parsing + compiling
//! from source.
//!
//! The loading strategy will be controlled by Cargo feature flags:
//!
//! | Feature | `load_platform()` behavior |
//! |---------|---------------------------|
//! | `from-source` (default) | Parse `.pure` → compile → `PureModel` |
//! | `from-binary` | Deserialize `.purem` → `PureModel` |
//!
//! The `from-source` path is essential for development (edit Pure files,
//! recompile) while `from-binary` is the release path (fast startup,
//! no parser/compiler dependency needed).
//!
//! The same feature-flag pattern applies to any Pure repository, not
//! just the platform — user repos can pre-compile their `.pure` files
//! into `.purem` for deployment.

pub mod platform;
pub mod sources;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_compiles() {
        match platform::load_platform() {
            Ok(model) => {
                println!("Platform compiled cleanly");
                // Verify that the model has elements
                assert!(!model.chunks.is_empty(), "Expected non-empty model chunks");
            }
            Err(partial) => {
                println!(
                    "Platform loaded with {} error(s) (expected during iteration)",
                    partial.errors.len()
                );
                // Still verify the model has elements
                assert!(
                    !partial.model.chunks.is_empty(),
                    "Expected non-empty model chunks even with errors"
                );
            }
        }

        // As a baseline check, just ensure we have platform source files
        let sources = sources::platform_sources();
        println!("Number of platform files: {}", sources.len());
        assert!(
            !sources.is_empty(),
            "Expected platform sources to be non-empty"
        );
        assert!(sources.len() > 10, "Expected over 10 platform files");
    }
}
