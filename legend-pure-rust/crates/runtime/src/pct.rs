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

//! Rust-port PCT manifest — `(adapter, exclusions)` for `runPCTTests`.
//!
//! Mirrors the Java `pct_*_native.json` manifest shape (an `adapter`
//! function path plus an `exclusions` map of test FQN to an expected
//! failure-message substring). Bundled at compile time from
//! `crates/runtime/resources/pct_grammar_rust_native.json` so both
//! tests and the `legend test` CLI consume the same exclusion set.
//!
//! The file is the single source of truth for which platform PCT
//! tests are intentionally skipped on the Rust port (representational
//! limits — i16 year range in jiff, i64-overflowing Integer literals
//! that Java's compiled engine excludes for the same reason).

use std::cell::RefCell;
use std::rc::Rc;

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::PureModel;
use smol_str::SmolStr;

use crate::value::{MapState, Value, ValueKey};

/// Embedded JSON text of the Rust-port PCT manifest. The CLI and the
/// runtime test harness both read from this constant so they cannot
/// drift out of sync.
pub const RUST_NATIVE_MANIFEST_JSON: &str =
    include_str!("../resources/pct_grammar_rust_native.json");

/// Parsed view of the Rust-port manifest, lazily decoded once per
/// process. Returns `None` if the bundled JSON is malformed (the build
/// validates this; failure here is a build-time invariant violation).
fn manifest() -> Option<&'static serde_json::Value> {
    use std::sync::OnceLock;
    static CELL: OnceLock<Option<serde_json::Value>> = OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str(RUST_NATIVE_MANIFEST_JSON).ok())
        .as_ref()
}

/// Resolve the manifest's adapter Function path against `model`.
///
/// Returns `None` when the manifest is malformed or the platform model
/// does not include the named adapter — for the standard in-memory
/// adapter the latter means a non-platform model.
#[must_use]
pub fn rust_native_adapter_id(model: &PureModel) -> Option<ElementId> {
    let path = manifest()?.get("adapter")?.as_str()?;
    let segments: Vec<SmolStr> = path.split("::").map(SmolStr::new).collect();
    model.resolve_by_path(&segments)
}

/// Build the `exclusions` argument expected by
/// `meta::pure::test::surveyor::runPCTTests` — a
/// `Map<Function<Any>, String>` keyed by test FQN.
///
/// Returns an empty map if the bundled manifest is missing or
/// malformed. Non-string entries are silently skipped.
#[must_use]
pub fn rust_native_exclusions() -> Value {
    let mut state = MapState::default();
    if let Some(exclusions_obj) = manifest()
        .and_then(|m| m.get("exclusions"))
        .and_then(serde_json::Value::as_object)
    {
        for (fqn, msg) in exclusions_obj {
            if let Some(msg) = msg.as_str() {
                state.entries.insert(
                    ValueKey::String(SmolStr::new(fqn)),
                    Value::String(SmolStr::new(msg)),
                );
            }
        }
    }
    Value::Map(Rc::new(RefCell::new(state)))
}

/// Convenience helper — the `(adapter, exclusions)` pair callers pass
/// to `runPCTTests`.
///
/// Returns `None` if the manifest's adapter path does not resolve in
/// `model`. In normal operation `model` is a platform model that
/// always contains `testAdapterForInMemoryExecution`.
#[must_use]
pub fn rust_native_pct_args(model: &PureModel) -> Option<(Value, Value)> {
    let adapter = rust_native_adapter_id(model)?;
    Some((Value::Element(adapter), rust_native_exclusions()))
}
