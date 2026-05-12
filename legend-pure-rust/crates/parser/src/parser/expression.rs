// Copyright 2026 The Legend Authors
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

use super::Parser;
use super::R;
use super::{ParserContext, split_package_name, unquote_string};
use crate::error::ParseError;
use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, Parameter};
use legend_pure_parser_ast::expression::{
    ArithmeticExpr, ArithmeticOp, ArrowFunction, BooleanLiteral, CollectionExpr, ComparisonExpr,
    ComparisonOp, CopyExpr, DateTimeLiteral, DecimalLiteral, Expression, FloatLiteral,
    FunctionApplication, IntegerLiteral, KeyValuePair, Lambda, LetExpr, Literal, LogicalExpr,
    LogicalOp, MemberAccess, MultiplicityReferenceExpr, NavigationPath, NewInstanceExpr, NotExpr,
    PackageableElementRef, PropertyPathElement, QualifiedMemberAccess, SimpleMemberAccess,
    SliceExpr, StrictDateLiteral, StrictTimeLiteral, StringLiteral, TypeReferenceExpr,
    UnaryMinusExpr, UnitInstanceExpr, Variable,
};
use legend_pure_parser_ast::island::IslandExpression;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::Package;
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

impl Parser {
    // ── Expressions (precedence-climbing recursive descent) ─────────────

    pub(crate) fn parse_expression_list(&mut self) -> R<Vec<Expression>> {
        let mut exprs = Vec::new();
        while !self.cursor.check(TokenKind::RBrace) && !self.cursor.check(TokenKind::Eof) {
            exprs.push(self.parse_expression()?);
            self.cursor.eat(TokenKind::Semicolon);
        }
        Ok(exprs)
    }

    /// Parse an expression (entry point — handles precedence via recursive descent).
    pub fn parse_expression(&mut self) -> R<Expression> {
        self.parse_or_expression()
    }

    pub(crate) fn parse_or_expression(&mut self) -> R<Expression> {
        let mut left = self.parse_and_expression()?;
        while self.cursor.check(TokenKind::PipePipe) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let right = self.parse_and_expression()?;
            left = Expression::Logical(LogicalExpr {
                left: Box::new(left),
                op: LogicalOp::Or,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    pub(crate) fn parse_and_expression(&mut self) -> R<Expression> {
        let mut left = self.parse_comparison()?;
        while self.cursor.check(TokenKind::AmpAmp) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let right = self.parse_comparison()?;
            left = Expression::Logical(LogicalExpr {
                left: Box::new(left),
                op: LogicalOp::And,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    pub(crate) fn parse_comparison(&mut self) -> R<Expression> {
        let mut left = self.parse_additive()?;
        if let Some(op) = self.comparison_op() {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let right = self.parse_additive()?;
            left = Expression::Comparison(ComparisonExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    pub(crate) fn comparison_op(&self) -> Option<ComparisonOp> {
        match self.cursor.peek_kind() {
            TokenKind::EqualEqual => Some(ComparisonOp::Equal),
            TokenKind::BangEqual => Some(ComparisonOp::NotEqual),
            TokenKind::Less => Some(ComparisonOp::LessThan),
            TokenKind::LessEqual => Some(ComparisonOp::LessThanOrEqual),
            TokenKind::Greater => Some(ComparisonOp::GreaterThan),
            TokenKind::GreaterEqual => Some(ComparisonOp::GreaterThanOrEqual),
            _ => None,
        }
    }

    pub(crate) fn parse_additive(&mut self) -> R<Expression> {
        let mut left = self.parse_multiplicative()?;
        while matches!(self.cursor.peek_kind(), TokenKind::Plus | TokenKind::Minus) {
            let si = self.cursor.current_source_info();
            let op = if self.cursor.peek_kind() == TokenKind::Plus {
                ArithmeticOp::Plus
            } else {
                ArithmeticOp::Minus
            };
            self.cursor.advance();
            let right = self.parse_multiplicative()?;
            left = Expression::Arithmetic(ArithmeticExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    pub(crate) fn parse_multiplicative(&mut self) -> R<Expression> {
        let mut left = self.parse_unary()?;
        while matches!(self.cursor.peek_kind(), TokenKind::Star | TokenKind::Slash) {
            let si = self.cursor.current_source_info();
            let op = if self.cursor.peek_kind() == TokenKind::Star {
                ArithmeticOp::Times
            } else {
                ArithmeticOp::Divide
            };
            self.cursor.advance();
            let right = self.parse_unary()?;
            left = Expression::Arithmetic(ArithmeticExpr {
                left: Box::new(left),
                op,
                right: Box::new(right),
                source_info: si,
            });
        }
        Ok(left)
    }

    pub(crate) fn parse_unary(&mut self) -> R<Expression> {
        if self.cursor.check(TokenKind::Bang) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let expr = self.parse_unary()?;
            return Ok(Expression::Not(NotExpr {
                operand: Box::new(expr),
                source_info: si,
            }));
        }
        if self.cursor.check(TokenKind::Minus) && !self.cursor.check(TokenKind::Arrow) {
            let si = self.cursor.current_source_info();
            self.cursor.advance();
            let expr = self.parse_unary()?;
            return Ok(Expression::UnaryMinus(UnaryMinusExpr {
                operand: Box::new(expr),
                source_info: si,
            }));
        }
        // Unary plus: `+expr` is semantically a no-op (identity)
        if self.cursor.check(TokenKind::Plus) {
            self.cursor.advance();
            return self.parse_unary();
        }
        self.parse_postfix()
    }

    pub(crate) fn parse_postfix(&mut self) -> R<Expression> {
        let mut expr = self.parse_primary()?;
        loop {
            if self.cursor.check(TokenKind::Arrow) {
                // Capture the target's span BEFORE we wrap it — used as
                // the start of the combined `target->fn(args)` span so
                // IDE goto / hover can locate the call from clicks
                // anywhere inside the chain. The `->` and function-name
                // spans alone (which the original code used) are far
                // too narrow.
                let target_si = expr.source_info().clone();
                let arrow_si = self.cursor.current_source_info();
                self.cursor.advance();
                let (func_name, func_si) = self.cursor.expect_identifier_or_keyword()?;
                // Build fully qualified path for the function
                let mut path = Package::root(func_name, func_si.clone());
                while self.cursor.eat(TokenKind::PathSep) {
                    let (seg, seg_si) = self.cursor.expect_identifier_or_keyword()?;
                    path = path.child(seg, seg_si);
                }
                let (pkg, name) = split_package_name(&path);
                let func = PackageableElementPtr {
                    package: pkg,
                    name,
                    // Function-pointer's source_info is the function-name
                    // span, not the `->` token. Goto-def underlines this.
                    source_info: func_si,
                };
                self.cursor.expect(TokenKind::LParen)?;
                let mut args = Vec::new();
                while !self.cursor.check(TokenKind::RParen) {
                    args.push(self.parse_expression()?);
                    self.cursor.eat(TokenKind::Comma);
                }
                // Capture the closing `)`'s span before consuming so the
                // combined source_info covers the whole call.
                let close_si = self.cursor.current_source_info();
                self.cursor.expect(TokenKind::RParen)?;
                let combined_si = SourceInfo::new(
                    target_si.source.clone(),
                    target_si.start_line,
                    target_si.start_column,
                    close_si.end_line,
                    close_si.end_column,
                );
                let _ = arrow_si; // kept for trace-debug if ever needed
                expr = Expression::ArrowFunction(ArrowFunction {
                    target: Box::new(expr),
                    function: func,
                    arguments: args,
                    source_info: combined_si,
                });
            } else if self.cursor.check(TokenKind::Dot) {
                let si = self.cursor.current_source_info();
                self.cursor.advance();
                let (member, member_si) = self.cursor.expect_identifier_or_keyword()?;
                if self.cursor.check(TokenKind::LParen) {
                    // Qualified member access: expr.member(args)
                    self.cursor.advance();
                    let mut args = Vec::new();
                    while !self.cursor.check(TokenKind::RParen) {
                        args.push(self.parse_expression()?);
                        self.cursor.eat(TokenKind::Comma);
                    }
                    self.cursor.expect(TokenKind::RParen)?;
                    expr =
                        Expression::MemberAccess(MemberAccess::Qualified(QualifiedMemberAccess {
                            target: Box::new(expr),
                            member,
                            member_source_info: member_si,
                            arguments: args,
                            source_info: si,
                        }));
                } else {
                    expr = Expression::MemberAccess(MemberAccess::Simple(SimpleMemberAccess {
                        target: Box::new(expr),
                        member,
                        member_source_info: member_si,
                        source_info: si,
                    }));
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    #[allow(clippy::too_many_lines, clippy::similar_names)]
    pub(crate) fn parse_primary(&mut self) -> R<Expression> {
        let si = self.cursor.current_source_info();
        match self.cursor.peek_kind() {
            // Literals
            TokenKind::IntegerLiteral => {
                let tok = self.cursor.advance().clone();
                let value: i64 = tok.text.parse().unwrap_or(0);
                let num_expr = Expression::Literal(Literal::Integer(IntegerLiteral {
                    value,
                    source_info: si.clone(),
                }));
                self.maybe_unit_instance(num_expr, si)
            }
            TokenKind::FloatLiteral => {
                let tok = self.cursor.advance().clone();
                let value: f64 = tok.text.parse().unwrap_or(0.0);
                let num_expr = Expression::Literal(Literal::Float(FloatLiteral {
                    value,
                    source_info: si.clone(),
                }));
                self.maybe_unit_instance(num_expr, si)
            }
            TokenKind::DecimalLiteral => {
                let tok = self.cursor.advance().clone();
                let text = tok.text.trim_end_matches(['D', 'd']);
                let num_expr = Expression::Literal(Literal::Decimal(DecimalLiteral {
                    value: text.to_string(),
                    source_info: si.clone(),
                }));
                self.maybe_unit_instance(num_expr, si)
            }
            TokenKind::StringLiteral => {
                let tok = self.cursor.advance().clone();
                Ok(Expression::Literal(Literal::String(StringLiteral {
                    value: unquote_string(&tok.text).into(),
                    source_info: si,
                })))
            }
            TokenKind::True => {
                self.cursor.advance();
                Ok(Expression::Literal(Literal::Boolean(BooleanLiteral {
                    value: true,
                    source_info: si,
                })))
            }
            TokenKind::False => {
                self.cursor.advance();
                Ok(Expression::Literal(Literal::Boolean(BooleanLiteral {
                    value: false,
                    source_info: si,
                })))
            }
            TokenKind::DateLiteral => {
                let tok = self.cursor.advance().clone();
                let raw = tok.text.trim_start_matches('%');
                // Classify: DateTime (has 'T'), StrictDate (year-only or
                // `YYYY`-prefixed, possibly `YYYY-MM` or `YYYY-MM-DD`),
                // StrictTime (everything else — `HH:MM:SS`-shaped).
                if raw.contains('T') {
                    Ok(Expression::Literal(Literal::DateTime(DateTimeLiteral {
                        value: raw.to_string(),
                        source_info: si,
                    })))
                } else {
                    // Strip optional leading minus for negative-year dates (%-YYYY[-MM[-DD]]).
                    let unsigned = raw.trim_start_matches('-');
                    // A StrictDate starts with a 4-digit year. Times always have
                    // ':' and 2-digit HH at the head, so a 4-digit prefix is
                    // sufficient to disambiguate. Accept `%YYYY`, `%YYYY-MM`,
                    // and `%YYYY-MM-DD` uniformly — the lowerer decides the
                    // precision based on segment count.
                    let year_prefix = unsigned.len() >= 4
                        && unsigned[..4].chars().all(|c| c.is_ascii_digit())
                        && !unsigned.contains(':');
                    if year_prefix {
                        Ok(Expression::Literal(Literal::StrictDate(
                            StrictDateLiteral {
                                value: raw.to_string(),
                                source_info: si,
                            },
                        )))
                    } else {
                        Ok(Expression::Literal(Literal::StrictTime(
                            StrictTimeLiteral {
                                value: raw.to_string(),
                                source_info: si,
                            },
                        )))
                    }
                }
            }
            // Leading-dot float literal: .01 → 0.01
            TokenKind::Dot if self.cursor.peek_kind_at(1) == TokenKind::IntegerLiteral => {
                self.cursor.advance(); // consume the dot
                let tok = self.cursor.advance().clone();
                let text = format!("0.{}", tok.text);
                let value: f64 = text.parse().unwrap_or(0.0);
                Ok(Expression::Literal(Literal::Float(FloatLiteral {
                    value,
                    source_info: si,
                })))
            }
            // Variable: $name
            TokenKind::Dollar => {
                self.cursor.advance();
                let (name, _) = self.cursor.expect_identifier_or_keyword()?;
                Ok(Expression::Variable(Variable {
                    name,
                    source_info: si,
                }))
            }
            // let binding
            TokenKind::Let => {
                self.cursor.advance();
                let (name, _) = self.cursor.expect_identifier_or_keyword()?;
                self.cursor.expect(TokenKind::Equals)?;
                let value = self.parse_expression()?;
                Ok(Expression::Let(LetExpr {
                    name,
                    value: Box::new(value),
                    source_info: si,
                }))
            }
            // No-param bare lambda: | body
            TokenKind::Pipe => {
                self.cursor.advance();
                let mut body = vec![self.parse_expression()?];
                while self.cursor.eat(TokenKind::Semicolon) {
                    // Stop if we hit a closing delimiter — `Comma` covers
                    // the case `if(cond, | then; , | else)` where a
                    // multi-statement no-param lambda body ends with `;,`
                    // before the next `if`/`match` arm.
                    if self.cursor.check(TokenKind::RParen)
                        || self.cursor.check(TokenKind::RBrace)
                        || self.cursor.check(TokenKind::RBracket)
                        || self.cursor.check(TokenKind::Comma)
                        || self.cursor.check(TokenKind::Eof)
                    {
                        break;
                    }
                    body.push(self.parse_expression()?);
                }
                Ok(Expression::Lambda(Lambda {
                    parameters: vec![],
                    body,
                    source_info: si,
                }))
            }
            // New instance: ^Type(props), ^Type<Args>(props), or copy: ^$var(overrides)
            TokenKind::Caret => {
                self.cursor.advance();

                // Copy from variable: ^$var(overrides)
                if self.cursor.check(TokenKind::Dollar) {
                    self.cursor.advance();
                    let (var_name, _) = self.cursor.expect_identifier_or_keyword()?;
                    self.cursor.expect(TokenKind::LParen)?;
                    let mut assignments = Vec::new();
                    while !self.cursor.check(TokenKind::RParen) {
                        let kv_si = self.cursor.current_source_info();
                        let (mut prop, _) = self.cursor.expect_identifier_or_keyword()?;
                        // Accept dotted property paths: address.name
                        while self.cursor.eat(TokenKind::Dot) {
                            let (next, _) = self.cursor.expect_identifier_or_keyword()?;
                            prop = SmolStr::new(format!("{prop}.{next}"));
                        }
                        // `+=` (append) vs `=` (replace) — record on the
                        // KeyValuePair so the runtime can pick `mutate_add`
                        // (append to existing values) or `mutate_set`
                        // (replace). Java threads this through as the
                        // `KeyValue.add` slot; mirror that semantics.
                        let augmented = self.cursor.eat(TokenKind::Plus);
                        self.cursor.expect(TokenKind::Equals)?;
                        let val = self.parse_expression()?;
                        assignments.push(KeyValuePair {
                            key: prop,
                            value: val,
                            augmented,
                            source_info: kv_si,
                        });
                        self.cursor.eat(TokenKind::Comma);
                    }
                    self.cursor.expect(TokenKind::RParen)?;
                    return Ok(Expression::Copy(CopyExpr {
                        source: var_name,
                        assignments,
                        source_info: si,
                    }));
                }

                // New instance: ^Type(props) or ^Type<Args>(props)
                let path = self.parse_package_path()?;
                let (pkg, name) = split_package_name(&path);

                // Optional type arguments: ^List<U>(...) or ^Class<|1>(...)
                let type_arguments = if self.cursor.eat(TokenKind::Less) {
                    let mut args = Vec::new();
                    loop {
                        if self.cursor.check(TokenKind::Pipe) {
                            // Multiplicity argument: |1, |* — skip over it
                            self.cursor.advance(); // consume |
                            if self.cursor.check(TokenKind::IntegerLiteral)
                                || self.cursor.check(TokenKind::Star)
                            {
                                self.cursor.advance(); // consume the value
                            }
                        } else {
                            args.push(self.parse_type_reference()?);
                        }
                        if !self.cursor.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    self.cursor.expect_closing_angle_bracket()?;
                    args
                } else {
                    vec![]
                };
                // type_arguments: thread through to NewInstanceExpr so the
                // compiler can preserve them in the lowered `new(…)` call —
                // `^List<String>(values=…)` instances need their element-side
                // bindings reflected at runtime via `genericType().typeArguments`.

                // Optional type variable values: (10, 'ok')
                let type_variable_values = if self.cursor.check(TokenKind::LParen) {
                    // Disambiguate from property assignments `(prop = val)`.
                    // A property assignment starts with an identifier/keyword (or integer property)
                    // followed by `=` or `+=` or `.` (for nested properties).
                    let mut lookahead = 1;
                    if self.cursor.peek_kind_at(lookahead) == TokenKind::IntegerLiteral {
                        lookahead += 1; // Integer keys
                    } else {
                        // Skip dotted property paths: address.name
                        while self.cursor.peek_kind_at(lookahead).is_identifier_like() {
                            lookahead += 1;
                            if self.cursor.peek_kind_at(lookahead) == TokenKind::Dot {
                                lookahead += 1;
                            } else {
                                break;
                            }
                        }
                    }
                    let next_kind = self.cursor.peek_kind_at(lookahead);
                    // `(prop = val)` or `(prop += val)` → property assignments
                    // `()` → empty property assignments (only if nothing preceded the `)`)
                    // `(10)` or `('str')` → type variable values
                    let is_prop_assignments = next_kind == TokenKind::Equals
                        || next_kind == TokenKind::Plus
                        || (next_kind == TokenKind::RParen && lookahead == 1); // truly empty `()`

                    if is_prop_assignments {
                        vec![]
                    } else {
                        self.cursor.expect(TokenKind::LParen)?;
                        let mut vals = Vec::new();
                        if !self.cursor.check(TokenKind::RParen) {
                            loop {
                                vals.push(self.parse_type_variable_value()?);
                                if !self.cursor.eat(TokenKind::Comma) {
                                    break;
                                }
                            }
                        }
                        self.cursor.expect(TokenKind::RParen)?;
                        vals
                    }
                } else {
                    vec![]
                };

                // Merge the start span (`^` token, captured before this
                // match arm) with the path's last-segment span so
                // `class_ref.source_info` covers the whole `^abc::Class1`
                // prefix (sans `()`). Without this, the lowered
                // `PackageableElementRef` carries a near-zero-width span
                // and Cmd-click on `Class1` inside `^abc::Class1()` falls
                // outside the index's recorded region.
                let class_span = si.merge(
                    legend_pure_parser_ast::source_info::Spanned::source_info(&path),
                );
                let class_ref = PackageableElementPtr {
                    package: pkg,
                    name,
                    source_info: class_span,
                };
                // Optional instance name: `^Type name(props)`. The Java
                // grammar accepts a single identifier between the type
                // (and any `<args>`/`(typeVars)` blocks) and the
                // property-assignments `(`. The runtime treats the name
                // as a distinguishing key.
                let instance_name = if !self.cursor.check(TokenKind::LParen)
                    && self.cursor.peek_kind().is_identifier_like()
                    && self.cursor.peek_kind_at(1) == TokenKind::LParen
                {
                    let (n, _) = self.cursor.expect_identifier_or_keyword()?;
                    Some(n)
                } else {
                    None
                };
                self.cursor.expect(TokenKind::LParen)?;
                let mut assignments = Vec::new();
                while !self.cursor.check(TokenKind::RParen) {
                    let kv_si = self.cursor.current_source_info();
                    // Accept identifier, dotted path, or integer key
                    let mut prop = if self.cursor.check(TokenKind::IntegerLiteral) {
                        let tok = self.cursor.advance().clone();
                        tok.text.clone()
                    } else {
                        let (name, _) = self.cursor.expect_identifier_or_keyword()?;
                        name
                    };
                    // Accept dotted property paths: address.name
                    while self.cursor.eat(TokenKind::Dot) {
                        let (next, _) = self.cursor.expect_identifier_or_keyword()?;
                        prop = SmolStr::new(format!("{prop}.{next}"));
                    }
                    // `+=` is a no-op on `^Class(...)` (no carried values
                    // to append to), but the parser tolerates it for symmetry
                    // with the copy-expression form. Recording the flag keeps
                    // the AST honest; the runtime collapses to `mutate_set`
                    // either way on a fresh allocation.
                    let augmented = self.cursor.eat(TokenKind::Plus);
                    self.cursor.expect(TokenKind::Equals)?;
                    let val = self.parse_expression()?;
                    assignments.push(KeyValuePair {
                        key: prop,
                        value: val,
                        augmented,
                        source_info: kv_si,
                    });
                    self.cursor.eat(TokenKind::Comma);
                }
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Expression::NewInstance(NewInstanceExpr {
                    class: class_ref,
                    type_arguments,
                    type_variable_values,
                    instance_name,
                    assignments,
                    source_info: si,
                }))
            }
            // Cast: `@Type` OR bare multiplicity literal: `@[m]`.
            // Java grammar `M3CoreParser.g4:311 AT (type | multiplicity)`
            // — peek the token after `@` to disambiguate.
            TokenKind::At => {
                self.cursor.advance();
                if self.cursor.check(TokenKind::LBracket) {
                    self.cursor.advance();
                    let multiplicity = self.parse_multiplicity_argument()?;
                    self.cursor.expect(TokenKind::RBracket)?;
                    return Ok(Expression::MultiplicityReferenceExpr(
                        MultiplicityReferenceExpr {
                            multiplicity,
                            source_info: si,
                        },
                    ));
                }
                let type_ref = self.parse_type_spec()?;
                Ok(Expression::TypeReferenceExpr(TypeReferenceExpr {
                    type_ref,
                    source_info: si,
                }))
            }
            // Column builder: ~name or ~[name, name2]
            TokenKind::Tilde => {
                self.cursor.advance();
                self.parse_column_builder(si)
            }
            // Collection: [expr, ...] or Range: [start:stop:step]
            TokenKind::LBracket => {
                self.cursor.advance();
                // Check for [:stop] (no start)
                if self.cursor.eat(TokenKind::Colon) {
                    let stop = self.parse_expression()?;
                    let step = if self.cursor.eat(TokenKind::Colon) {
                        Some(Box::new(self.parse_expression()?))
                    } else {
                        None
                    };
                    self.cursor.expect(TokenKind::RBracket)?;
                    return Ok(Expression::Slice(SliceExpr {
                        start: None,
                        stop: Box::new(stop),
                        step,
                        source_info: si,
                    }));
                }
                if self.cursor.check(TokenKind::RBracket) {
                    self.cursor.advance();
                    return Ok(Expression::Collection(CollectionExpr {
                        elements: vec![],
                        multiplicity: None,
                        source_info: si,
                    }));
                }
                let first = self.parse_expression()?;
                // Range: [start:stop] or [start:stop:step]
                if self.cursor.eat(TokenKind::Colon) {
                    let stop = self.parse_expression()?;
                    let step = if self.cursor.eat(TokenKind::Colon) {
                        Some(Box::new(self.parse_expression()?))
                    } else {
                        None
                    };
                    self.cursor.expect(TokenKind::RBracket)?;
                    Ok(Expression::Slice(SliceExpr {
                        start: Some(Box::new(first)),
                        stop: Box::new(stop),
                        step,
                        source_info: si,
                    }))
                } else {
                    // Standard collection
                    let mut elements = vec![first];
                    while self.cursor.eat(TokenKind::Comma) {
                        if self.cursor.check(TokenKind::RBracket) {
                            break;
                        }
                        elements.push(self.parse_expression()?);
                    }
                    self.cursor.expect(TokenKind::RBracket)?;
                    Ok(Expression::Collection(CollectionExpr {
                        elements,
                        multiplicity: None,
                        source_info: si,
                    }))
                }
            }
            // Parenthesized expression: (expr)
            TokenKind::LParen => {
                self.cursor.advance();
                let expr = self.parse_expression()?;
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Expression::Group(Box::new(expr)))
            }
            // Lambda or block: {x | body}, {| body}, {x: String[1], y | body}, or {expr; expr}
            TokenKind::LBrace => {
                self.cursor.advance();
                if self.is_lambda_start() {
                    let mut params = Vec::new();
                    // Only parse params if the next token isn't pipe ({|body})
                    if !self.cursor.check(TokenKind::Pipe) {
                        loop {
                            params.push(self.parse_lambda_param()?);
                            if !self.cursor.eat(TokenKind::Comma) {
                                break;
                            }
                        }
                    }
                    self.cursor.expect(TokenKind::Pipe)?;
                    let body = self.parse_expression_list()?;
                    self.cursor.expect(TokenKind::RBrace)?;
                    Ok(Expression::Lambda(Lambda {
                        parameters: params,
                        body,
                        source_info: si,
                    }))
                } else {
                    let exprs = self.parse_expression_list()?;
                    self.cursor.expect(TokenKind::RBrace)?;
                    if exprs.len() == 1 {
                        if let Some(single) = exprs.into_iter().next() {
                            Ok(single)
                        } else {
                            unreachable!("len() == 1 guarantees at least one element");
                        }
                    } else {
                        Ok(Expression::Collection(CollectionExpr {
                            elements: exprs,
                            multiplicity: None,
                            source_info: si,
                        }))
                    }
                }
            }
            // Bare lambda: x|body or x: Type[1]|body
            // Must be checked before identifier since both start with an identifier.
            TokenKind::Identifier if self.is_bare_lambda() => self.parse_bare_lambda(),
            // Identifier or element keyword used as a name.
            // Every identifier-like token (per `TokenKind::is_identifier_like`)
            // is valid in expression position — e.g., `Class('arg')`,
            // `my::Enum::VAL`, or `assertInstanceOf($x, Association)`.
            // The `PathSep` arm covers root-qualified paths (`::pkg::Name`).
            kind if kind.is_identifier_like() || kind == TokenKind::PathSep => {
                let path = self.parse_package_path()?;

                // Handle Measure~Unit references: RomanLength~Pes
                let tilde_suffix = if self.cursor.eat(TokenKind::Tilde) {
                    let (unit_name, _) = self.cursor.expect_identifier_or_keyword()?;
                    Some(unit_name)
                } else {
                    None
                };

                // Span the whole FQN (`abc::Sub::Name`) for the
                // resulting `PackageableElementPtr.source_info` so the
                // goto-def index records a clickable region covering
                // every segment. Without this, the `si` we'd inherit
                // is just the position before the path — a single-
                // token span. Mirrors the merge done for type refs.
                let fqn_span = si.merge(legend_pure_parser_ast::source_info::Spanned::source_info(
                    &path,
                ));
                if self.cursor.check(TokenKind::LParen) {
                    // Function call: name(args)
                    self.cursor.advance();
                    let mut args = Vec::new();
                    while !self.cursor.check(TokenKind::RParen) {
                        args.push(self.parse_expression()?);
                        self.cursor.eat(TokenKind::Comma);
                    }
                    self.cursor.expect(TokenKind::RParen)?;
                    let (pkg, mut name) = split_package_name(&path);
                    if let Some(unit) = &tilde_suffix {
                        name = SmolStr::new(format!("{name}~{unit}"));
                    }
                    let func = PackageableElementPtr {
                        package: pkg,
                        name,
                        source_info: fqn_span.clone(),
                    };
                    Ok(Expression::FunctionApplication(FunctionApplication {
                        function: func,
                        arguments: args,
                        source_info: si,
                    }))
                } else {
                    // Bare element reference (no parens): String, my::Enum, MyClass
                    let (pkg, mut name) = split_package_name(&path);
                    if let Some(unit) = &tilde_suffix {
                        name = SmolStr::new(format!("{name}~{unit}"));
                    }
                    let element = PackageableElementPtr {
                        package: pkg,
                        name,
                        source_info: fqn_span.clone(),
                    };
                    Ok(Expression::PackageableElementRef(PackageableElementRef {
                        element,
                        source_info: fqn_span,
                    }))
                }
            }
            // Island grammar: #{ content }# (empty tag — graph fetch).
            //
            // Empty-tag dispatch keeps the legacy contract: the
            // `IslandParser::parse` consumes the body up to (but not
            // past) `RBraceHash`, and the host parser consumes the
            // `RBraceHash` itself.
            TokenKind::HashLBrace => {
                self.cursor.advance();
                let tag = "";

                // Temporarily take the island parsers to avoid borrow conflict
                // (we need &self.island_parsers for lookup and &mut self for
                // ParserContext simultaneously).
                let parsers = std::mem::take(&mut self.island_parsers);
                let result = (|| {
                    let island_parser =
                        parsers.iter().find(|p| p.tag() == tag).ok_or_else(|| {
                            ParseError::expected(
                                &format!("island grammar for tag '{tag}'"),
                                self.cursor.peek_kind(),
                                si.clone(),
                            )
                        })?;
                    let mut ctx = ParserContext { parser: self };
                    island_parser.parse(&mut ctx)
                })();
                self.island_parsers = parsers;

                let content = result?;
                self.cursor.expect(TokenKind::RBraceHash)?;
                Ok(Expression::Island(IslandExpression {
                    content,
                    source_info: si,
                }))
            }

            // Navigation path expression: `#/Type/prop1/prop2(args)/prop3!alias#`.
            //
            // Distinct from the tagged-island path below — paths use `/`
            // as their step separator, not as a tag, and own a dedicated
            // opener token (`HashSlash`) emitted by the lexer. The body
            // is parsed inline (not via `IslandParser`) because Path is
            // a built-in language form, not an extension DSL.
            TokenKind::HashSlash => {
                self.cursor.advance();
                self.parse_navigation_path(si)
            }

            // Tagged island grammar: `#tag…#` or `#tag{…}#`.
            //
            // The host parser reads the tag (an identifier like `TDS`,
            // or a single special character like `>`), looks up a
            // matching `IslandParser` registered for that tag, and
            // delegates body parsing to it. **Tagged plug-ins are
            // responsible for consuming their own closing delimiter**
            // — `RBraceHash` for curly-body shapes (`#>{…}#`) or a
            // closing `Hash` for raw-body shapes (`#TDS\n…\n#`).
            // This contract is asymmetric with the empty-tag path
            // above so the existing graph-fetch parser keeps its
            // current behaviour, and so each new tagged island can
            // own its body shape without the host needing to know it.
            TokenKind::Hash => {
                self.cursor.advance();
                let tag_tok = self.cursor.advance().clone();
                let tag = match tag_tok.kind {
                    TokenKind::Identifier => tag_tok.text.clone(),
                    TokenKind::Greater => SmolStr::new(">"),
                    _ => {
                        return Err(ParseError::expected(
                            "island tag (identifier or `>`)",
                            tag_tok.kind,
                            tag_tok.source_info,
                        ));
                    }
                };

                let parsers = std::mem::take(&mut self.island_parsers);
                let result = (|| {
                    let island_parser = parsers
                        .iter()
                        .find(|p| p.tag() == tag.as_str())
                        .ok_or_else(|| {
                            ParseError::expected(
                                &format!("island grammar for tag '{tag}'"),
                                self.cursor.peek_kind(),
                                si.clone(),
                            )
                        })?;
                    let mut ctx = ParserContext { parser: self };
                    island_parser.parse(&mut ctx)
                })();
                self.island_parsers = parsers;

                let content = result?;
                // No host-side closing-delimiter consumption: the
                // tagged island parser already consumed its own.
                Ok(Expression::Island(IslandExpression {
                    content,
                    source_info: si,
                }))
            }
            _ => Err(ParseError::expected(
                "expression",
                self.cursor.peek_kind(),
                si,
            )),
        }
    }

    /// Parses the body of a navigation-path expression after the leading
    /// `#/` has been consumed.
    ///
    /// Grammar:
    /// ```text
    /// nav-path  ::= start-type ('/' prop ('(' params ')')?)+ ('!' alias)? '#'
    /// start-type ::= type-reference   // supports type-args (`Firm<Any>`)
    /// prop       ::= identifier
    /// alias      ::= identifier
    /// ```
    ///
    /// At least one `/property` step is required — `#/Type#` is rejected
    /// at parse time, matching the Java grammar's "A path must contain at
    /// least one navigation" rule.
    ///
    /// Parameters are parsed as full expressions and may include scalar
    /// literals, collection literals (`['a', 'b']`), and enum stubs
    /// (`SynType.CUSIP` parses as `MemberAccess::Simple`).
    fn parse_navigation_path(&mut self, opener_si: SourceInfo) -> R<Expression> {
        let start_type = self.parse_type_reference()?;

        let mut path: Vec<PropertyPathElement> = Vec::new();
        while self.cursor.check(TokenKind::Slash) {
            let step_si = self.cursor.current_source_info();
            self.cursor.advance(); // consume '/'
            let (property, _prop_si) = self.cursor.expect_identifier_or_keyword()?;
            let parameters = if self.cursor.eat(TokenKind::LParen) {
                let mut args = Vec::new();
                if !self.cursor.check(TokenKind::RParen) {
                    loop {
                        args.push(self.parse_expression()?);
                        if !self.cursor.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                }
                self.cursor.expect(TokenKind::RParen)?;
                args
            } else {
                Vec::new()
            };
            path.push(PropertyPathElement {
                property,
                parameters,
                source_info: step_si,
            });
        }

        if path.is_empty() {
            return Err(ParseError::expected(
                "navigation step (`/property`) — a path must contain at least one navigation",
                self.cursor.peek_kind(),
                self.cursor.current_source_info(),
            ));
        }

        // Optional alias: `!identifier`. Java grammar uses `!` (lexed as
        // `Bang`) here, distinct from `!=` (`BangEqual`).
        let name = if self.cursor.eat(TokenKind::Bang) {
            let (alias, _) = self.cursor.expect_identifier_or_keyword()?;
            Some(alias)
        } else {
            None
        };

        self.cursor.expect(TokenKind::Hash)?;

        Ok(Expression::NavigationPath(NavigationPath {
            start_type,
            path,
            name,
            source_info: opener_si,
        }))
    }

    /// Detects whether the current token starts a lambda inside braces.
    ///
    /// Matches patterns after `{` has been consumed:
    /// - `{x |`            — untyped single param
    /// - `{x, y |`         — untyped multi param
    /// - `{x: Type[1] |`   — typed param
    /// - `{|`              — no-param braced lambda
    pub(crate) fn is_lambda_start(&self) -> bool {
        // {| body}
        if self.cursor.check(TokenKind::Pipe) {
            return true;
        }
        // {ident ... |}
        matches!(
            self.cursor.peek_kind(),
            TokenKind::Identifier | TokenKind::StringLiteral
        ) && matches!(
            self.cursor.peek_kind_at(1),
            TokenKind::Pipe | TokenKind::Comma | TokenKind::Colon
        )
    }

    /// Detects bare lambda in expression position (outside braces).
    ///
    /// Only single-param bare lambdas are valid (matching Java grammar):
    /// - `x|body`              — untyped
    /// - `x: Type[mult]|body`  — typed
    pub(crate) fn is_bare_lambda(&self) -> bool {
        // ident followed by pipe: x|body
        if matches!(self.cursor.peek_kind_at(1), TokenKind::Pipe) {
            return true;
        }
        // ident followed by colon: x: Type[mult]|body
        // We need to scan past the type + multiplicity to check for pipe
        if matches!(self.cursor.peek_kind_at(1), TokenKind::Colon) {
            return self.scan_past_type_for_pipe(2);
        }
        false
    }

    /// Scans forward from `offset` past a type reference and multiplicity
    /// bracket to check if the next token is `|` (pipe).
    ///
    /// Used to detect typed bare lambdas: `x: path::Type<Args>[mult]|body`.
    pub(crate) fn scan_past_type_for_pipe(&self, start_offset: usize) -> bool {
        let mut offset = start_offset;
        // Skip the type path: identifier (:: identifier)*
        // Element keywords (`Class`, `Association`, `Profile`, `Measure`,
        // `Primitive`, etc.) are valid type names, so accept anything
        // that's identifier-like — not just plain `Identifier`.
        if !self.cursor.peek_kind_at(offset).is_identifier_like() {
            return false;
        }
        offset += 1;
        // Skip :: segments
        while matches!(self.cursor.peek_kind_at(offset), TokenKind::PathSep) {
            offset += 1; // ::
            offset += 1; // ident
        }
        // Skip <TypeArgs>
        if matches!(self.cursor.peek_kind_at(offset), TokenKind::Less) {
            let mut depth = 1;
            offset += 1;
            while depth > 0 {
                match self.cursor.peek_kind_at(offset) {
                    TokenKind::Less => depth += 1,
                    TokenKind::Greater => depth -= 1,
                    TokenKind::Eof => return false,
                    _ => {}
                }
                offset += 1;
            }
        }
        // Expect [multiplicity]
        if !matches!(self.cursor.peek_kind_at(offset), TokenKind::LBracket) {
            return false;
        }
        offset += 1;
        // Skip multiplicity content until ]
        while !matches!(
            self.cursor.peek_kind_at(offset),
            TokenKind::RBracket | TokenKind::Eof
        ) {
            offset += 1;
        }
        if matches!(self.cursor.peek_kind_at(offset), TokenKind::RBracket) {
            offset += 1;
        }
        // Check for pipe
        matches!(self.cursor.peek_kind_at(offset), TokenKind::Pipe)
    }

    /// Parses a bare lambda: `x|body` or `x: Type[mult]|body`.
    ///
    /// Only single-param bare lambdas are valid per the Java grammar.
    pub(crate) fn parse_bare_lambda(&mut self) -> R<Expression> {
        let si = self.cursor.current_source_info();
        let param = self.parse_lambda_param()?;
        self.cursor.expect(TokenKind::Pipe)?;

        let mut body = vec![self.parse_expression()?];
        while self.cursor.eat(TokenKind::Semicolon) {
            if self.cursor.check(TokenKind::RParen)
                || self.cursor.check(TokenKind::RBrace)
                || self.cursor.check(TokenKind::RBracket)
                || self.cursor.check(TokenKind::Comma)
                || self.cursor.check(TokenKind::Eof)
            {
                break;
            }
            body.push(self.parse_expression()?);
        }

        Ok(Expression::Lambda(Lambda {
            parameters: vec![param],
            body,
            source_info: si,
        }))
    }

    /// Parses a lambda parameter: `name` or `name: Type[multiplicity]`.
    pub(crate) fn parse_lambda_param(&mut self) -> R<Parameter> {
        let si = self.cursor.current_source_info();
        let (name, _) = self.cursor.expect_identifier()?;
        if self.cursor.eat(TokenKind::Colon) {
            let type_ref = self.parse_type_reference()?;
            self.cursor.expect(TokenKind::LBracket)?;
            let multiplicity = self.parse_multiplicity()?;
            self.cursor.expect(TokenKind::RBracket)?;
            Ok(Parameter {
                name,
                type_ref: Some(type_ref),
                multiplicity: Some(multiplicity),
                source_info: si,
            })
        } else {
            Ok(Parameter {
                name,
                type_ref: None,
                multiplicity: None,
                source_info: si,
            })
        }
    }

    pub(crate) fn parse_column_builder(
        &mut self,
        si: legend_pure_parser_ast::SourceInfo,
    ) -> R<Expression> {
        let mut columns = vec![];
        let is_array = if self.cursor.eat(TokenKind::LBracket) {
            if !self.cursor.check(TokenKind::RBracket) {
                loop {
                    columns.push(self.parse_one_col_spec()?);
                    if !self.cursor.eat(TokenKind::Comma) {
                        break;
                    }
                }
            }
            self.cursor.expect(TokenKind::RBracket)?;
            true
        } else {
            columns.push(self.parse_one_col_spec()?);
            false
        };
        Ok(Expression::Column(
            legend_pure_parser_ast::expression::ColumnBuilderExpr {
                columns,
                is_array,
                source_info: si,
            },
        ))
    }

    fn parse_one_col_spec(&mut self) -> R<legend_pure_parser_ast::expression::ColumnSpec> {
        use legend_pure_parser_ast::expression::{ColumnSpec, ColumnTypeSpec};

        let si = self.cursor.current_source_info();
        let stereotypes = self.parse_stereotypes()?;
        let tagged_values = self.parse_tagged_values()?;
        let (name, _) = self.cursor.expect_identifier_or_keyword()?;

        let mut type_spec = None;
        let mut extra_function = None;

        if self.cursor.eat(TokenKind::Colon) {
            // Java grammar:
            //   oneColSpec: ... columnName (COLON (type multiplicity? | anyLambda) extraFunction?)?
            //   extraFunction: COLON anyLambda
            //
            // The first piece after `:` is either a type-with-mult or a
            // primary lambda (init function); the optional second piece
            // after another `:` is the aggregator lambda.
            if self.cursor.check(TokenKind::LBrace) || self.is_bare_lambda() {
                let expr = self.parse_expression()?;
                if let Expression::Lambda(l) = expr {
                    type_spec = Some(ColumnTypeSpec::Lambda(l));
                } else {
                    // Non-lambda expression in lambda position is unusual
                    // but harmless — keep it as the extra-function carrier
                    // so the resolver can still see it.
                    extra_function = Some(Box::new(expr));
                }
            } else {
                let type_ref = self.parse_type_reference()?;
                let mult = if self.cursor.check(TokenKind::LBracket) {
                    self.cursor.advance();
                    let m = self.parse_multiplicity()?;
                    self.cursor.expect(TokenKind::RBracket)?;
                    Some(m)
                } else {
                    None
                };
                type_spec = Some(ColumnTypeSpec::Typed(type_ref, mult));
            }

            // Optional `extraFunction`: `: anyLambda` after the primary.
            // Mirrors Java's `extraFunction: COLON anyLambda`.
            if self.cursor.eat(TokenKind::Colon) {
                let expr = self.parse_expression()?;
                extra_function = Some(Box::new(expr));
            }
        }

        Ok(ColumnSpec {
            stereotypes,
            tagged_values,
            name,
            type_spec,
            extra_function,
            source_info: si,
        })
    }

    /// After parsing a numeric literal, check if it's followed by a
    /// `Measure~Unit` reference (e.g., `5 RomanLength~Pes`).
    /// If so, wrap them into a `UnitInstanceExpr`.
    fn maybe_unit_instance(
        &mut self,
        num_expr: Expression,
        si: legend_pure_parser_ast::SourceInfo,
    ) -> R<Expression> {
        // Look ahead: Identifier followed by Tilde means unit value
        if self.cursor.peek_kind() == TokenKind::Identifier
            && self.cursor.peek_kind_at(1) == TokenKind::Tilde
        {
            let (measure_name, _) = self.cursor.expect_identifier_or_keyword()?;
            self.cursor.expect(TokenKind::Tilde)?;
            let (unit_name, _) = self.cursor.expect_identifier_or_keyword()?;
            let combined = SmolStr::new(format!("{measure_name}~{unit_name}"));
            let end_si = self.cursor.current_source_info();
            Ok(Expression::UnitInstance(UnitInstanceExpr {
                value: Box::new(num_expr),
                unit: PackageableElementPtr {
                    package: None,
                    name: combined,
                    source_info: end_si,
                },
                source_info: si,
            }))
        } else {
            Ok(num_expr)
        }
    }
}
