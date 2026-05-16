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

//! Fluent builder for constructing [`Evaluator`] instances.
//!
//! The builder is the production wiring entry point. It defaults
//! every knob to the distributed-slice discovery path so a downstream
//! binary with extension crates in its Cargo graph inherits the
//! extensions with zero per-call wiring:
//!
//! ```ignore
//! use legend_pure_runtime::eval::Evaluator;
//!
//! let eval = Evaluator::builder().build(&model);
//! ```
//!
//! Tests and bespoke embedders override individual defaults:
//!
//! ```ignore
//! let eval = Evaluator::builder()
//!     .registry(&my_explicit_registry)
//!     .populators(&[&MyTestPopulator])
//!     .build(&model);
//! ```

use std::collections::HashMap;

use legend_pure_parser_pure::model::PureModel;

use crate::dsl::DSLPopulator;
use crate::eval::Evaluator;
use crate::native::NativeRegistry;

/// Fluent builder for [`Evaluator`] — see the [module docs](crate::builder).
///
/// Unset knobs fall back to distributed-slice discovery at
/// [`build`](Self::build) time. The builder is single-use (consumed
/// by [`build`](Self::build)); construct a fresh one per
/// `Evaluator`.
#[must_use = "EvaluatorBuilder does nothing until `.build(&model)` is called"]
pub struct EvaluatorBuilder<'a> {
    registry: Option<&'a NativeRegistry>,
    populators: Option<Vec<&'a dyn DSLPopulator>>,
    extension_configs: Option<HashMap<String, HashMap<String, toml::Value>>>,
}

impl<'a> EvaluatorBuilder<'a> {
    /// Create a builder with every knob unset (every default in play).
    pub fn new() -> Self {
        Self {
            registry: None,
            populators: None,
            extension_configs: None,
        }
    }

    /// Use the supplied [`NativeRegistry`] for native dispatch.
    ///
    /// Overrides the default of
    /// [`NativeRegistry::discovered`](NativeRegistry::discovered). Use
    /// in tests that compose a deliberate native set, or in production
    /// when the embedder pre-builds the registry once and reuses it
    /// across many `Evaluator` instances.
    pub fn registry(mut self, registry: &'a NativeRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Use the supplied DSL populator slice for `Element::DSLInstance`
    /// heap hydration.
    ///
    /// Overrides the default of
    /// [`crate::dsl::discovered_populators`].
    pub fn populators(mut self, populators: &[&'a dyn DSLPopulator]) -> Self {
        self.populators = Some(populators.to_vec());
        self
    }

    /// Install per-evaluator extension configuration sourced from the
    /// `[extension.<name>]` tables in `legend-pure-classpath.toml`.
    ///
    /// The CLI typically passes
    /// `legend_cli::classpath::ResolvedClasspath::extension_configs`
    /// verbatim. Extensions look up their sub-table via
    /// [`crate::native::EvalContextTrait::config_for`] during native
    /// dispatch or populator execution.
    ///
    /// Default is the empty table — extensions fall back to
    /// defaults / env-var overrides when no configuration is present.
    pub fn extension_configs(
        mut self,
        cfgs: HashMap<String, HashMap<String, toml::Value>>,
    ) -> Self {
        self.extension_configs = Some(cfgs);
        self
    }

    /// Construct the [`Evaluator`].
    ///
    /// Resolves defaults:
    /// - `registry` ← [`NativeRegistry::discovered`] (leaked to
    ///   `'static` so the evaluator's `&NativeRegistry` borrow is
    ///   satisfied; one leak per `build` call when the default is
    ///   used).
    /// - `populators` ← [`crate::dsl::discovered_populators`].
    ///
    /// # Panics
    ///
    /// Propagates any discovery-time panic from `NativeRegistry::discovered`
    /// (duplicate mangled FQNs, extension-vs-platform collision) or
    /// `discovered_populators` (duplicate `dsl_name`).
    pub fn build(self, model: &'a PureModel) -> Evaluator<'a> {
        let registry: &'a NativeRegistry = self.registry.unwrap_or_else(|| {
            // Leak so the evaluator's borrow is satisfied. This is a
            // one-shot allocation per default-registry build —
            // production code that constructs many Evaluators should
            // call `.registry(&pre_built)` to avoid the repeated leak.
            Box::leak(Box::new(NativeRegistry::discovered()))
        });

        // Resolve populators. The discovered slice yields `&'static`
        // refs which auto-coerce into the `&'a` slice the consumer
        // expects (via covariant lifetime narrowing).
        let populators: Vec<&'a dyn DSLPopulator> = self.populators.unwrap_or_else(|| {
            crate::dsl::discovered_populators()
                .into_iter()
                .map(|p| p as &dyn DSLPopulator)
                .collect()
        });

        let mut eval = Evaluator::new(model, registry);
        if let Some(cfgs) = self.extension_configs {
            eval.set_extension_configs(cfgs);
        }
        if !populators.is_empty() {
            crate::dsl::run_populators(model, eval.heap_mut(), &populators);
        }
        eval
    }
}

impl Default for EvaluatorBuilder<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use legend_pure_parser_pure::model::PureModel;

    #[test]
    fn build_with_defaults_constructs_evaluator() {
        // Empty model is enough for builder smoke — we only exercise
        // wiring, not evaluation.
        let model = PureModel::new();
        let _eval = Evaluator::builder().build(&model);
    }

    #[test]
    fn build_with_explicit_registry_uses_it() {
        let model = PureModel::new();
        let registry = NativeRegistry::standard();
        let eval = Evaluator::builder().registry(&registry).build(&model);
        assert_eq!(eval.natives().len(), NativeRegistry::standard().len());
    }

    #[test]
    fn build_with_explicit_empty_populators_short_circuits() {
        let model = PureModel::new();
        let pops: [&dyn DSLPopulator; 0] = [];
        let _eval = Evaluator::builder().populators(&pops).build(&model);
    }

    #[test]
    fn extension_configs_round_trip_via_builder() {
        let model = PureModel::new();
        let mut inner = HashMap::new();
        inner.insert("port".to_string(), toml::Value::Integer(9999));
        let mut cfgs = HashMap::new();
        cfgs.insert("relational.h2".to_string(), inner);

        let eval = Evaluator::builder().extension_configs(cfgs).build(&model);

        let sub = eval
            .config_for("relational.h2")
            .expect("relational.h2 sub-table installed");
        assert_eq!(
            sub.get("port"),
            Some(&toml::Value::Integer(9999)),
            "port value round-trips verbatim through the builder",
        );

        assert!(
            eval.config_for("unknown.ext").is_none(),
            "unknown extension name yields None",
        );
    }

    #[test]
    fn extension_configs_default_is_empty_when_unset() {
        let model = PureModel::new();
        let eval = Evaluator::builder().build(&model);
        assert!(eval.config_for("anything").is_none());
    }
}
