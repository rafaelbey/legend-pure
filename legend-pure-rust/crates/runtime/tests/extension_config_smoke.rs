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

//! End-to-end proof that per-evaluator extension configuration
//! installed via [`legend_pure_runtime::builder::EvaluatorBuilder::extension_configs`]
//! is observable through the [`legend_pure_runtime::native::EvalContextTrait::config_for`]
//! API that natives use.
//!
//! Two-evaluator isolation matters: this is the property the
//! [`store-relational-runtime::set_extension_configs`] `OnceLock`
//! sacrificed (process-wide singleton). Phase 4 restores it via the
//! per-evaluator field.

use std::collections::HashMap;

use legend_pure_parser_pure::model::PureModel;
use legend_pure_runtime::eval::Evaluator;

#[test]
fn config_for_returns_installed_subtable() {
    let model = PureModel::new();
    let mut h2 = HashMap::new();
    h2.insert(
        "jar_path".to_string(),
        toml::Value::String("/tmp/h2.jar".to_string()),
    );
    h2.insert("pg_port".to_string(), toml::Value::Integer(5435));

    let mut relational = HashMap::new();
    relational.insert(
        "h2".to_string(),
        toml::Value::Table(h2.into_iter().collect()),
    );

    let mut configs = HashMap::new();
    configs.insert("relational".to_string(), relational);

    let eval = Evaluator::builder()
        .extension_configs(configs)
        .build(&model);

    let sub = eval
        .config_for("relational")
        .expect("relational sub-table installed");
    let h2 = sub.get("h2").expect("h2 entry under relational");
    let table = h2.as_table().expect("h2 is a TOML table");
    assert_eq!(
        table.get("pg_port"),
        Some(&toml::Value::Integer(5435)),
        "pg_port round-trips verbatim through builder + Evaluator + config_for",
    );
}

#[test]
fn two_evaluators_get_independent_configs() {
    // The OnceLock workaround in store-relational-runtime made this
    // test impossible: every evaluator in the process saw the same
    // first-write-wins config. Per-evaluator storage restores
    // isolation.
    let model = PureModel::new();

    let mut cfg_a = HashMap::new();
    cfg_a.insert("name".to_string(), toml::Value::String("alpha".into()));
    let mut configs_a = HashMap::new();
    configs_a.insert("test".to_string(), cfg_a);

    let mut cfg_b = HashMap::new();
    cfg_b.insert("name".to_string(), toml::Value::String("beta".into()));
    let mut configs_b = HashMap::new();
    configs_b.insert("test".to_string(), cfg_b);

    let eval_a = Evaluator::builder()
        .extension_configs(configs_a)
        .build(&model);
    let eval_b = Evaluator::builder()
        .extension_configs(configs_b)
        .build(&model);

    assert_eq!(
        eval_a
            .config_for("test")
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str()),
        Some("alpha"),
    );
    assert_eq!(
        eval_b
            .config_for("test")
            .and_then(|m| m.get("name"))
            .and_then(|v| v.as_str()),
        Some("beta"),
    );
}

#[test]
fn evaluator_with_no_config_returns_none() {
    let model = PureModel::new();
    let eval = Evaluator::builder().build(&model);
    assert!(eval.config_for("anything").is_none());
}
