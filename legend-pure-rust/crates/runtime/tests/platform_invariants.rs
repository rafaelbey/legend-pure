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

//! Negative invariant tests for the platform NativeRegistry.
//!
//! The invariant: `NativeRegistry::standard()` exposes only the
//! platform-callable native surface — consumer-defined mutators
//! (`mutateAdd`) and Java-internal reflective dispatch (`applyFunction`)
//! must NOT be reachable from a Pure program compiled against the
//! standard registry. They are extension-only and must arrive through
//! `RuntimeExtension::register_natives` if the consumer wants them.
//!
//! See `docs/runtime/metaprogramming.md` §5 for the rationale: heap
//! mutation IS available internally (`Reactivate` and `Copy` use it),
//! but exposing it as a Pure native would break the read-only-meta-
//! programming contract that matches Java's compiled-mode semantics.
//!
//! These tests are the lock-in for that invariant — any future PR that
//! accidentally registers `mutateAdd` (or `applyFunction`) in the
//! platform `standard()` factory will fail loudly here.

use legend_pure_runtime::native::NativeRegistry;

/// `mutateAdd<T>(T[1], String[1], Any[*]):T[1]` must not be registered
/// in the platform standard set. Lives in legend-engine's
/// functions-unclassified extension package.
#[test]
fn mutate_add_not_in_standard_registry() {
    let registry = NativeRegistry::standard();
    let mangled = "mutateAdd_T_1__String_1__Any_MANY__T_1_";
    assert!(
        registry.get(mangled).is_none(),
        "mutateAdd must not be registered in NativeRegistry::standard() — \
         it is a consumer-defined native that lives in the \
         legend-engine functions-unclassified extension package. Registering \
         it here would break the read-only-meta-programming invariant \
         documented in docs/runtime/metaprogramming.md §5.",
    );
}

/// Defence-in-depth: the prefix-fallback dispatch path (used for
/// operator-name dispatch when the compiler emits a simple name before
/// FQN resolution) must also not surface `mutateAdd`.
#[test]
fn mutate_add_not_resolvable_by_prefix() {
    let registry = NativeRegistry::standard();
    assert!(
        registry.find_by_prefix("mutateAdd").is_none(),
        "mutateAdd must not be resolvable by simple-name prefix in the \
         platform registry — see metaprogramming.md §5.",
    );
}

/// `applyFunction` is a Java-internal reflective-array dispatch entry
/// point that Pure never surfaces to user code (subsumed by `eval`/
/// `evaluate`). The standard registry must therefore not expose it
/// under any plausible mangled key.
#[test]
fn apply_function_not_in_standard_registry() {
    let registry = NativeRegistry::standard();
    assert!(
        registry.find_by_prefix("applyFunction").is_none(),
        "applyFunction must not be registered in NativeRegistry::standard() — \
         it is a Java-internal concept and is subsumed by `eval`/`evaluate` \
         in the user-visible Pure surface.",
    );
}

/// Positive control: confirm a representative meta-programming native
/// IS present in the standard set, so the negative assertions above
/// aren't trivially satisfied by a misnamed registry. If this test
/// fails alongside the negatives, the registry itself is broken (not
/// the invariant).
#[test]
fn deactivate_is_in_standard_registry() {
    let registry = NativeRegistry::standard();
    assert!(
        registry
            .get("deactivate_Any_MANY__ValueSpecification_1_")
            .is_some(),
        "deactivate must be registered in the standard set — if this assertion \
         fails, the registry initialisation itself is broken and the negative \
         invariant tests in this file are trivially satisfied.",
    );
}
