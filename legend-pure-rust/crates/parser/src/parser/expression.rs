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
use legend_pure_parser_ast::annotation::{PackageableElementPtr, Parameter};
use legend_pure_parser_ast::expression::{
    ArithmeticExpr, ArithmeticOp, ArrowFunction, BooleanLiteral, CollectionExpr, ComparisonExpr,
    ComparisonOp, CopyExpr, DateTimeLiteral, DecimalLiteral, Expression, FloatLiteral,
    FunctionApplication, IntegerLiteral, KeyValuePair, Lambda, LetExpr, Literal, LogicalExpr,
    LogicalOp, MemberAccess, NewInstanceExpr, NotExpr, PackageableElementRef,
    QualifiedMemberAccess, SimpleMemberAccess, SliceExpr, StrictDateLiteral, StrictTimeLiteral,
    StringLiteral, TypeReferenceExpr, UnaryMinusExpr, Variable,
};
use legend_pure_parser_ast::island::IslandExpression;
use legend_pure_parser_ast::type_ref::Package;
use legend_pure_parser_lexer::TokenKind;

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
                let si = self.cursor.current_source_info();
                self.cursor.advance();
                let (func_name, func_si) = self.cursor.expect_identifier_or_keyword()?;
                // Build fully qualified path for the function
                let mut path = Package::root(func_name, func_si);
                while self.cursor.eat(TokenKind::PathSep) {
                    let (seg, seg_si) = self.cursor.expect_identifier_or_keyword()?;
                    path = path.child(seg, seg_si);
                }
                let (pkg, name) = split_package_name(&path);
                let func = PackageableElementPtr {
                    package: pkg,
                    name,
                    source_info: si.clone(),
                };
                self.cursor.expect(TokenKind::LParen)?;
                let mut args = Vec::new();
                while !self.cursor.check(TokenKind::RParen) {
                    args.push(self.parse_expression()?);
                    self.cursor.eat(TokenKind::Comma);
                }
                self.cursor.expect(TokenKind::RParen)?;
                expr = Expression::ArrowFunction(ArrowFunction {
                    target: Box::new(expr),
                    function: func,
                    arguments: args,
                    source_info: si,
                });
            } else if self.cursor.check(TokenKind::Dot) {
                let si = self.cursor.current_source_info();
                self.cursor.advance();
                let (member, _) = self.cursor.expect_identifier_or_keyword()?;
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
                            arguments: args,
                            source_info: si,
                        }));
                } else {
                    expr = Expression::MemberAccess(MemberAccess::Simple(SimpleMemberAccess {
                        target: Box::new(expr),
                        member,
                        source_info: si,
                    }));
                }
            } else {
                break;
            }
        }
        Ok(expr)
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn parse_primary(&mut self) -> R<Expression> {
        let si = self.cursor.current_source_info();
        match self.cursor.peek_kind() {
            // Literals
            TokenKind::IntegerLiteral => {
                let tok = self.cursor.advance().clone();
                let value: i64 = tok.text.parse().unwrap_or(0);
                Ok(Expression::Literal(Literal::Integer(IntegerLiteral {
                    value,
                    source_info: si,
                })))
            }
            TokenKind::FloatLiteral => {
                let tok = self.cursor.advance().clone();
                let value: f64 = tok.text.parse().unwrap_or(0.0);
                Ok(Expression::Literal(Literal::Float(FloatLiteral {
                    value,
                    source_info: si,
                })))
            }
            TokenKind::DecimalLiteral => {
                let tok = self.cursor.advance().clone();
                let text = tok.text.trim_end_matches(['D', 'd']);
                Ok(Expression::Literal(Literal::Decimal(DecimalLiteral {
                    value: text.to_string(),
                    source_info: si,
                })))
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
                // Classify: DateTime (has 'T'), StrictDate (has '-' in date part), StrictTime
                if raw.contains('T') {
                    Ok(Expression::Literal(Literal::DateTime(DateTimeLiteral {
                        value: raw.to_string(),
                        source_info: si,
                    })))
                } else {
                    // Strip optional leading minus for negative-year dates (%-YYYY-MM-DD)
                    let unsigned = raw.trim_start_matches('-');
                    if unsigned.starts_with(|c: char| c.is_ascii_digit()) && unsigned.contains('-')
                    {
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
                let body = self.parse_expression()?;
                Ok(Expression::Lambda(Lambda {
                    parameters: vec![],
                    body: vec![body],
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
                        let (prop, _) = self.cursor.expect_identifier()?;
                        self.cursor.eat(TokenKind::Plus); // += (append) syntax
                        self.cursor.expect(TokenKind::Equals)?;
                        let val = self.parse_expression()?;
                        assignments.push(KeyValuePair {
                            key: prop,
                            value: val,
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

                // Optional type arguments: ^List<U>(...)
                let type_arguments = if self.cursor.eat(TokenKind::Less) {
                    let mut args = Vec::new();
                    loop {
                        args.push(self.parse_type_reference()?);
                        if !self.cursor.eat(TokenKind::Comma) {
                            break;
                        }
                    }
                    self.cursor.expect_closing_angle_bracket()?;
                    args
                } else {
                    vec![]
                };
                let _ = type_arguments; // TODO: store in NewInstanceExpr when compiler needs it

                let class_ref = PackageableElementPtr {
                    package: pkg,
                    name,
                    source_info: si.clone(),
                };
                self.cursor.expect(TokenKind::LParen)?;
                let mut assignments = Vec::new();
                while !self.cursor.check(TokenKind::RParen) {
                    let kv_si = self.cursor.current_source_info();
                    let (prop, _) = self.cursor.expect_identifier()?;
                    self.cursor.eat(TokenKind::Plus); // += (append) syntax
                    self.cursor.expect(TokenKind::Equals)?;
                    let val = self.parse_expression()?;
                    assignments.push(KeyValuePair {
                        key: prop,
                        value: val,
                        source_info: kv_si,
                    });
                    self.cursor.eat(TokenKind::Comma);
                }
                self.cursor.expect(TokenKind::RParen)?;
                Ok(Expression::NewInstance(NewInstanceExpr {
                    class: class_ref,
                    assignments,
                    source_info: si,
                }))
            }
            // Cast: @Type
            TokenKind::At => {
                self.cursor.advance();
                let type_ref = self.parse_type_reference()?;
                Ok(Expression::TypeReferenceExpr(TypeReferenceExpr {
                    type_ref,
                    source_info: si,
                }))
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
            // Element keywords (Class, Enum, etc.) are valid identifiers in
            // expression position — e.g., `Class('arg')` or `my::Enum::VAL`.
            TokenKind::Identifier
            | TokenKind::Class
            | TokenKind::Enum
            | TokenKind::Profile
            | TokenKind::Function
            | TokenKind::PathSep => {
                let path = self.parse_package_path()?;
                if self.cursor.check(TokenKind::LParen) {
                    // Function call: name(args)
                    self.cursor.advance();
                    let mut args = Vec::new();
                    while !self.cursor.check(TokenKind::RParen) {
                        args.push(self.parse_expression()?);
                        self.cursor.eat(TokenKind::Comma);
                    }
                    self.cursor.expect(TokenKind::RParen)?;
                    let (pkg, name) = split_package_name(&path);
                    let func = PackageableElementPtr {
                        package: pkg,
                        name,
                        source_info: si.clone(),
                    };
                    Ok(Expression::FunctionApplication(FunctionApplication {
                        function: func,
                        arguments: args,
                        source_info: si,
                    }))
                } else {
                    // Bare element reference (no parens): String, my::Enum, MyClass
                    let (pkg, name) = split_package_name(&path);
                    let element = PackageableElementPtr {
                        package: pkg,
                        name,
                        source_info: si.clone(),
                    };
                    Ok(Expression::PackageableElementRef(PackageableElementRef {
                        element,
                        source_info: si,
                    }))
                }
            }
            // Island grammar: #{ content }# or #tag{ content }#
            TokenKind::HashLBrace => {
                self.cursor.advance();
                // Currently only the default tag "" is supported.
                // When tagged islands (#>{}#, #sql{}#) are added, the tag
                // will be extracted from the token stream here.
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
            _ => Err(ParseError::expected(
                "expression",
                self.cursor.peek_kind(),
                si,
            )),
        }
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
        if !matches!(
            self.cursor.peek_kind_at(offset),
            TokenKind::Identifier | TokenKind::StringLiteral
        ) {
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
        let body = self.parse_expression()?;
        Ok(Expression::Lambda(Lambda {
            parameters: vec![param],
            body: vec![body],
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
}
