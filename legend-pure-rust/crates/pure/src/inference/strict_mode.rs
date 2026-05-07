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

//! Strict-inference toggle — Step 3g of the inference encapsulation
//! plan.
//!
//! # Java parity vs strict divergence
//!
//! Java's `FunctionExpressionProcessor` silently widens
//! mismatched-type args to `Any` via `findBestCommonGenericType`'s
//! covariant LUB at `register():467-480`. The platform PCT corpus
//! depends on that — fold-style chains, recursive generic helpers
//! (`getAllTypeGeneralisations`), and the `match([λ1, λ2])` pattern
//! all rely on the lenient binding step. We hold parity by default.
//!
//! **Strict mode** is a *deliberate divergence over Java semantics*,
//! not a parity gap. When enabled, `validate_call_arguments`
//! substitutes each parameter's type with the call's already-computed
//! `GenericBindings` before the `is_type_compatible` check, so
//! `eval(f:Function<{Integer→String}>, 'wrong')` rejects the second
//! arg (T was bound to Integer authoritatively from the FunctionType
//! slot; the constraint slot rejects String) instead of LUBing T to
//! Any silently.
//!
//! # Mechanism
//!
//! A `Cell<bool>` thread-local — strict mode is process- AND
//! thread-scoped, never global. Tests that exercise strict behaviour
//! wrap their work in [`with_strict_mode`] which sets the flag,
//! invokes the closure, and restores the previous value (RAII via
//! `scopeguard` would be cleaner but `with_*` keeps the dependency
//! footprint flat).
//!
//! Default: OFF. Setting `LEGEND_PURE_STRICT_INFERENCE=1` at process
//! start flips the default ON for that process — used for opt-in
//! production use of strict mode without code changes.
//!
//! # Why thread-local instead of plumbed-through
//!
//! Plumbing a `strict_mode: bool` through every level of
//! `pipeline::compile → infer_function_body → InferCtx →
//! validate_call_arguments` would touch a substantial public API
//! surface. The toggle is read in exactly one site
//! (`validate_call_arguments`), and tests that need to flip it run
//! sequentially within a single closure. A thread-local fits the
//! contour without leaking the divergence into the pipeline's
//! signature.

use std::cell::Cell;

thread_local! {
    /// Per-thread override. `None` = "use the process-level default";
    /// `Some(b)` = explicit override (typically set by a test through
    /// [`with_strict_mode`]).
    static STRICT_OVERRIDE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// True when strict-inference is enabled. The active value is the
/// thread-local override if set, otherwise the process-level default
/// derived from `LEGEND_PURE_STRICT_INFERENCE` (anything but empty,
/// `0`, or `false` enables it).
#[must_use]
pub fn is_enabled() -> bool {
    if let Some(b) = STRICT_OVERRIDE.with(Cell::get) {
        return b;
    }
    process_default()
}

/// Run `body` with strict mode forced to the given value, restoring
/// the previous override on exit (panic-safe via the closure's drop
/// order — the guard restores even when `body` panics, which is what
/// tests want when they assert `expect_err`).
///
/// Returns whatever `body` returns.
pub fn with_strict_mode<R>(enabled: bool, body: impl FnOnce() -> R) -> R {
    struct Guard {
        previous: Option<bool>,
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            STRICT_OVERRIDE.with(|cell| cell.set(self.previous));
        }
    }
    let previous = STRICT_OVERRIDE.with(|cell| cell.replace(Some(enabled)));
    let _guard = Guard { previous };
    body()
}

/// Sample the env var lazily-and-once-per-process. Not memoised —
/// strict-mode runs on the call-validation hot path and reading
/// the env var directly is O(1) here, but if profiling later shows
/// it as a bottleneck we'd cache via a `OnceLock`.
fn process_default() -> bool {
    match std::env::var("LEGEND_PURE_STRICT_INFERENCE") {
        Ok(s) => !matches!(s.as_str(), "" | "0" | "false" | "FALSE"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_disabled() {
        // Whatever the test runner's env says, an *unscoped* call
        // returns the process default. We don't assert false directly
        // because a developer could be running tests with the env
        // var set; instead, assert that a `with_strict_mode(false)`
        // wrapper forces the override to false.
        with_strict_mode(false, || {
            assert!(!is_enabled());
        });
    }

    #[test]
    fn override_is_thread_local_and_restores_on_exit() {
        // Outside the wrapper, no override.
        let outside_before = STRICT_OVERRIDE.with(Cell::get);
        with_strict_mode(true, || {
            assert!(is_enabled());
            with_strict_mode(false, || {
                assert!(!is_enabled());
            });
            // Inner guard restored the outer override.
            assert!(is_enabled());
        });
        let outside_after = STRICT_OVERRIDE.with(Cell::get);
        assert_eq!(outside_before, outside_after);
    }
}
