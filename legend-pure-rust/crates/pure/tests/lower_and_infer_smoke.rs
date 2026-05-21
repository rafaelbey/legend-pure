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

//! Locks the public surface of
//! `legend_pure_parser_pure::extension::lower_and_infer_expression`.
//! Compiler-extension consumers (currently the Mapping DSL) lower and
//! type-check user-supplied AST expressions through this wrapper.
//!
//! If the wrapper changes signature, this test won't compile —
//! flagging the breakage at PR time rather than at the consumer.

use legend_pure_parser_ast::expression as ast_expr;
use legend_pure_parser_ast::source_info::SourceInfo;
use legend_pure_parser_pure::bootstrap;
use legend_pure_parser_pure::extension::lower_and_infer_expression;
use legend_pure_parser_pure::model::PureModel;
use legend_pure_parser_pure::types::{Multiplicity, TypeExpr};
use smol_str::SmolStr;

fn si() -> SourceInfo {
    SourceInfo::new("smoke.pure", 1, 1, 1, 5)
}

fn bootstrap_model() -> PureModel {
    let mut model = PureModel::new();
    let (chunk, _) = bootstrap::create_bootstrap_chunk(model.root_package);
    model.chunks.push(chunk);
    model
}

#[test]
fn boolean_literal_infers_to_boolean_one() {
    let model = bootstrap_model();
    let expr =
        ast_expr::Expression::Literal(ast_expr::Literal::Boolean(ast_expr::BooleanLiteral {
            value: true,
            source_info: si(),
        }));
    let mut errors = Vec::new();
    let ty = lower_and_infer_expression(&model, &[], &expr, &[], &mut errors)
        .expect("boolean literal must lower + infer");
    assert!(
        errors.is_empty(),
        "literal inference should produce no errors, got {errors:?}"
    );
    assert_eq!(
        ty.type_expr,
        TypeExpr::Named {
            element: bootstrap::BOOLEAN_ID,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
        }
    );
    assert_eq!(ty.multiplicity, Multiplicity::PureOne);
}

#[test]
fn variable_binding_resolves_to_supplied_type() {
    let model = bootstrap_model();
    // The expression is `$src` — a bare variable reference. The
    // wrapper should bind `src` to the type we pass and report it
    // back from inference.
    let expr = ast_expr::Expression::Variable(ast_expr::Variable {
        name: SmolStr::new("src"),
        source_info: si(),
    });
    let mut errors = Vec::new();

    let bindings = [(
        SmolStr::new("src"),
        TypeExpr::Named {
            element: bootstrap::STRING_ID,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
        },
        Multiplicity::PureOne,
    )];
    let ty = lower_and_infer_expression(&model, &[], &expr, &bindings, &mut errors)
        .expect("variable reference must lower + infer with binding");
    assert!(errors.is_empty(), "got unexpected errors: {errors:?}");
    assert_eq!(
        ty.type_expr,
        TypeExpr::Named {
            element: bootstrap::STRING_ID,
            type_arguments: Vec::new(),
            multiplicity_arguments: Vec::new(),
            value_arguments: Vec::new(),
            source_info: None,
        }
    );
    assert_eq!(ty.multiplicity, Multiplicity::PureOne);
}
