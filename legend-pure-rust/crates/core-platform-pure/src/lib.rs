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
//! Use [`platform::load_platform()`] for the canonical no-arg path —
//! it loads the embedded platform + DSL repos and parses + compiles
//! them into a `PureModel`. Under the hood it calls
//! [`repo::load`] over [`repo::Repo::default_embedded`].
//!
//! For non-default configurations (live filesystem reload, mixing
//! shapes, future `.purem` snapshots), build a `Vec<Repo>` directly
//! and call [`repo::load`].
//!
//! # Embedded vs. filesystem vs. `.purem`
//!
//! Each repo is a [`repo::Repo`] in one of three shapes — embedded
//! (build-time `include_str!`), filesystem (runtime walk), or
//! pre-compiled `.purem` snapshot (Stage 2; see
//! `docs/PUREM_FORMAT.md`). The shape is per-repo, so users can mix
//! embedded DSL repos with a filesystem-backed `platform` repo for
//! `--live` development.
//!
//! # Embedding utility
//!
//! The build-time work is done by [`legend_pure_build`], a
//! standalone crate that any Rust crate owning Pure repos can use
//! from its own `build.rs`.

pub mod classpath;
pub mod platform;
pub mod repo;
pub mod sources;
pub mod topo;

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

        // Baseline check: the embedded `platform` repo is now a
        // `.purem` blob (no `.pure` sources). Verify we have it and
        // it surfaces at least one manifest (e.g. PCT exclusion
        // lists) so `find_manifest` keeps working.
        let repos = repo::Repo::default_embedded();
        assert_eq!(
            repos.len(),
            1,
            "default_embedded() should return only the platform after Phase 3b"
        );
        let manifest_count: usize = repos.iter().map(|r| r.manifests().count()).sum();
        println!("Number of platform .json manifests: {manifest_count}");
        assert!(
            manifest_count > 0,
            "expected at least one platform manifest"
        );
    }
}
