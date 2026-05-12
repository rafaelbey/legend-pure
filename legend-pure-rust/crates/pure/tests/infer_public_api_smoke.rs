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

//! Locks the public surface of `legend_pure_parser_pure::infer` so
//! external compiler extensions (currently `crates/dsl-mapping`,
//! future `crates/dsl-relational`) can drive type inference on lambda
//! bodies they own.
//!
//! If `infer` becomes `pub(crate)` again or `infer_function_body`
//! changes signature, this test won't compile — flagging the
//! breakage at PR time rather than at the consumer.

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::infer::infer_function_body;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::types::{ExprKind, Multiplicity, TypeExpr, ValueSpec};
use smol_str::SmolStr;

fn si() -> SourceInfo {
    SourceInfo::new("infer_pub.pure", 1, 1, 1, 10)
}

fn untyped(kind: ExprKind) -> ValueSpec {
    ValueSpec {
        kind: Box::new(kind),
        source_info: si(),
        type_info: None,
    }
}

fn bootstrap_model() -> PureModel {
    let mut model = PureModel::new();
    let chunk = bootstrap::create_bootstrap_chunk(model.root_package);
    model.chunks.push(chunk);
    model
}

#[test]
fn infer_function_body_is_callable_from_external_crate() {
    let model = bootstrap_model();
    // A trivial body: a single string literal. After inference, the
    // node's `type_info` should report `String[1]`.
    let mut body = vec![untyped(ExprKind::StringLiteral(SmolStr::new("hello")))];
    let mut errors = Vec::new();

    infer_function_body(&model, &[], &mut body, &mut errors);

    assert!(
        errors.is_empty(),
        "literal inference should produce no errors, got {errors:?}"
    );
    let ti = body[0]
        .type_info
        .as_ref()
        .expect("inference should populate type_info on the literal");
    assert_eq!(
        ti.type_expr,
        TypeExpr::Named {
            element: bootstrap::STRING_ID,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
        }
    );
    assert_eq!(ti.multiplicity, Multiplicity::PureOne);
}
