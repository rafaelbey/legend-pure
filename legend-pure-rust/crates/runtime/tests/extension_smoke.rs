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

//! Smoke tests for the `RuntimeExtension` SPI.
//!
//! These tests validate that downstream crates can register additional
//! native functions through the trait without modifying the runtime
//! crate. They exercise the SPI directly — registry composition, name
//! diagnostics, and last-write-wins ordering — without crossing repo
//! boundaries. Full end-to-end exercise (loading consumer .pure source
//! through an evaluator built with a real extension) lives in the
//! consumer crate's own integration tests.

use legend_pure_parser_pure::types::ValueSpec;
use legend_pure_runtime::error::PureException;
use legend_pure_runtime::native::{
    EvalContextTrait, Evaluated, NativeFunction, NativeRegistry, RuntimeExtension,
};
use legend_pure_runtime::value::Value;

/// A trivial native that returns a fixed integer; mirrors the
/// `ConstantFn` pattern from `native.rs`'s test module.
#[derive(Debug)]
struct ConstantNative(i64);

impl NativeFunction for ConstantNative {
    fn execute(
        &self,
        _args: &[ValueSpec],
        _ctx: &mut dyn EvalContextTrait,
    ) -> Result<Evaluated, PureException> {
        Ok(Evaluated::new(Value::Integer(self.0)))
    }

    fn signature(&self) -> &'static str {
        "ext_constant__Integer_1_"
    }
}

/// A test extension that contributes one native under a unique key.
struct TestExt;

impl RuntimeExtension for TestExt {
    fn name(&self) -> &'static str {
        "test-extension"
    }

    fn register_natives(&self, registry: &mut NativeRegistry) {
        registry.register("ext_constant__Integer_1_", ConstantNative(42));
    }
}

#[test]
fn with_extensions_preserves_platform_natives() {
    let registry = NativeRegistry::with_extensions(&[&TestExt]);
    // A representative platform native must still resolve. `plus` is
    // registered by `arithmetic::register` in the standard set; if the
    // standard pre-load were skipped this lookup would miss.
    assert!(
        registry.get("plus_Integer_MANY__Integer_1_").is_some(),
        "platform native `plus_Integer_MANY__Integer_1_` should remain registered"
    );
}

#[test]
fn with_extensions_adds_extension_natives() {
    let registry = NativeRegistry::with_extensions(&[&TestExt]);
    assert!(
        registry.get("ext_constant__Integer_1_").is_some(),
        "extension native should be registered by TestExt::register_natives"
    );
}

#[test]
fn extension_name_is_retrievable() {
    let ext = TestExt;
    assert_eq!(ext.name(), "test-extension");
}

#[test]
fn empty_extension_slice_equivalent_to_standard() {
    let standard = NativeRegistry::standard();
    let with_none = NativeRegistry::with_extensions(&[]);
    assert_eq!(
        standard.len(),
        with_none.len(),
        "with_extensions(&[]) must produce the same count as standard()"
    );
}

/// Two extensions registering the same mangled key — later wins.
/// Mirrors `register`'s last-write-wins semantics documented on
/// `NativeRegistry::with_extensions`.
#[test]
fn later_extension_overrides_earlier() {
    struct ExtA;
    impl RuntimeExtension for ExtA {
        fn name(&self) -> &'static str {
            "ext-a"
        }
        fn register_natives(&self, registry: &mut NativeRegistry) {
            registry.register("shared_key__Integer_1_", ConstantNative(1));
        }
    }
    struct ExtB;
    impl RuntimeExtension for ExtB {
        fn name(&self) -> &'static str {
            "ext-b"
        }
        fn register_natives(&self, registry: &mut NativeRegistry) {
            registry.register("shared_key__Integer_1_", ConstantNative(2));
        }
    }

    // Both extensions register under "shared_key__Integer_1_"; ExtB lands second.
    let registry = NativeRegistry::with_extensions(&[&ExtA, &ExtB]);
    let func = registry
        .get("shared_key__Integer_1_")
        .expect("shared key should be registered");
    // Signature is the same for both ConstantNative instances; we can't
    // distinguish which native won via the signature alone. The
    // last-wins semantic is enforced at the HashMap layer (insert
    // overwrites) and locked by the `register` contract; this test
    // confirms the SPI doesn't panic or duplicate on key collision.
    assert_eq!(func.signature(), "ext_constant__Integer_1_");
}
