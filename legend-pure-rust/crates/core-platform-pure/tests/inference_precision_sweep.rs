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

//! Inference-precision sweep across the embedded platform model.
//!
//! Walks every concrete (non-native) function and reports cases where
//! the declared return type is a specific `Named` element but the
//! body's last expression infers as `Any`. These are silent precision
//! losses that `check_body_return_signature` (infer.rs) deliberately
//! skips via the `actual_eid == ANY_ID` early-return — surfacing them
//! here makes the gap visible without breaking valid code that the
//! escape hatch already protects.
//!
//! Asserts a hard ceiling so regressions in the dispatch / generic
//! substitution / lambda inference pipeline trip a CI failure. The
//! ceiling started at 9 (commit `e5e27c592` brought it to 2 by running
//! the full lambda-body second-pass at infer-time), then to 0 once
//! `is_subtype` recognised `Nil` as the bottom type and the second-pass
//! walked into Lambdas wrapped in a `Collection` (the
//! `match([λ1, λ2, …])` shape).

use legend_pure_core_platform::platform;
use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::model::Element;
use legend_pure_parser_pure::types::{ExprKind, FunctionCallData, TypeExpr};

const PRECISION_CEILING: usize = 0;

#[test]
fn platform_inference_precision_under_ceiling() {
    let model = match platform::load_platform() {
        Ok(m) => m,
        Err(p) => p.model,
    };

    let mut imprecise: Vec<String> = Vec::new();

    for chunk in model.chunks.iter().skip(1) {
        for (local_idx, element) in chunk.elements.iter() {
            let Element::Function(func) = element else {
                continue;
            };
            if func.is_native {
                continue;
            }
            let Some(last) = func.body.last() else {
                continue;
            };

            // `let x = expr;` as last statement carries the let's
            // synthetic `Nil[0]` return — Pure's let-as-last rule means
            // the effective return is the rhs's. Skip.
            if let ExprKind::FunctionCall(FunctionCallData { function_name, .. }) =
                last.kind.as_ref()
                && function_name == "letFunction"
            {
                continue;
            }

            let declared_eid = match &func.return_type {
                TypeExpr::Named { element, .. } => Some(*element),
                _ => continue,
            };
            if declared_eid == Some(bootstrap::ANY_ID) || declared_eid == Some(bootstrap::NIL_ID) {
                continue;
            }

            let Some(rt) = last.type_info.as_ref() else {
                continue;
            };
            let actual_eid = match &rt.type_expr {
                TypeExpr::Named { element, .. } => Some(*element),
                _ => continue,
            };

            if actual_eid == Some(bootstrap::ANY_ID) && declared_eid != Some(bootstrap::ANY_ID) {
                let node = chunk.nodes.get(local_idx);
                let declared_name = declared_eid.map_or_else(
                    || "<unknown>".to_string(),
                    |e| model.element_name(e).to_string(),
                );
                imprecise.push(format!(
                    "{}: declared {} but body infers Any",
                    node.name, declared_name
                ));
            }
        }
    }

    if !imprecise.is_empty() {
        eprintln!(
            "Platform inference precision: {} fn(s) imprecise (ceiling: {}):",
            imprecise.len(),
            PRECISION_CEILING
        );
        for line in &imprecise {
            eprintln!("  {line}");
        }
    }

    #[allow(clippy::absurd_extreme_comparisons)]
    // PRECISION_CEILING is the load-bearing knob; comparison shape stays uniform when ceiling changes
    let within = imprecise.len() <= PRECISION_CEILING;
    assert!(
        within,
        "Inference precision regressed: {} > ceiling {}.\n{}",
        imprecise.len(),
        PRECISION_CEILING,
        imprecise.join("\n")
    );
}
