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
use super::{split_package_name, unquote_string};
use crate::error::ParseError;
use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::type_ref::{
    FUNCTION_TYPE_SENTINEL, Identifier, Multiplicity, MultiplicityArgument, Package,
    RelationColumn, RelationType, TypeReference, TypeSpec, TypeVariableValue, UnitReference,
};
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

impl Parser {
    // ── Type references ─────────────────────────────────────────────────

    pub(crate) fn parse_type_reference(&mut self) -> R<TypeReference> {
        // Function type: {ParamType[mult] -> ReturnType[mult]}
        if self.cursor.check(TokenKind::LBrace) {
            return self.parse_function_type_as_type_ref();
        }
        let start = self.cursor.current_source_info();
        let path = self.parse_package_path()?;
        let (pkg, name) = split_package_name(&path);
        self.finish_type_reference(start, pkg, name)
    }

    /// Completes type reference parsing after the package path has been consumed.
    ///
    /// This allows callers (like `parse_type_spec`) to parse the path first,
    /// inspect ahead for `<(` column-spec syntax, and then delegate here for
    /// the standard `<TypeArgs>(TypeVarValues)` continuation.
    pub(crate) fn finish_type_reference(
        &mut self,
        start: SourceInfo,
        pkg: Option<Package>,
        name: SmolStr,
    ) -> R<TypeReference> {
        let (type_arguments, multiplicity_arguments) = if self.cursor.eat(TokenKind::Less) {
            let mut args = Vec::new();
            if self.cursor.check(TokenKind::LParen) {
                // Column specification: <(a:Integer, b:String)>
                // Encode columns as type_arguments for backward compatibility.
                // In parse_type_spec context, these are promoted to TypeSpec::Relation.
                let cols = self.parse_relation_columns()?;
                for col in cols {
                    args.push(TypeReference {
                        package: None,
                        name: col.name,
                        type_arguments: vec![col.type_ref],
                        multiplicity_arguments: vec![],
                        type_variable_values: vec![],
                        source_info: col.source_info,
                    });
                }
                // Subtype constraint can also follow a column-spec form,
                // e.g. `ColSpec<(?:Z)⊆T>` in eval.pure. Same drop-the-
                // bound treatment as in the non-column branch below.
                if self.cursor.eat(TokenKind::Subset) {
                    let _bound = self.parse_type_reference()?;
                }
            } else {
                loop {
                    args.push(self.parse_type_reference()?);
                    // Type-union operator `T+V`: the M3 grammar allows
                    // `+`-chained types inside a generic-type-arg
                    // position (e.g. `Relation<T+V>`). The AST has no
                    // dedicated union slot; we keep the first operand
                    // and drop the rest, matching Java's structural-
                    // union resolution at compile time. Whichever
                    // operand is captured doesn't affect downstream
                    // dispatch since the lowerer treats unions as
                    // opaque shapes.
                    while self.cursor.eat(TokenKind::Plus) {
                        let _discarded = self.parse_type_reference()?;
                    }
                    // Subtype-constraint operator `X⊆T` (U+2286): the
                    // type to the right is a *bound* on the type-arg
                    // (`X` must be a subtype of `T`). The AST has no
                    // bounds slot today; we accept the syntax and drop
                    // the bound — Stage-1 resolution doesn't enforce
                    // it. Real shape: `ColSpec<(?:Z)⊆T>` in
                    // `core_functions_relation/relation/functions/eval.pure`.
                    if self.cursor.eat(TokenKind::Subset) {
                        let _bound = self.parse_type_reference()?;
                    }
                    if !self.cursor.eat(TokenKind::Comma) {
                        break;
                    }
                }
            }
            // Multiplicity arguments: <TypeArgs | MultArgs>
            let mult_args = if self.cursor.eat(TokenKind::Pipe) {
                self.parse_multiplicity_arguments()?
            } else {
                vec![]
            };
            self.cursor.expect_closing_angle_bracket()?;
            (args, mult_args)
        } else {
            (vec![], vec![])
        };
        let type_variable_values = if self.cursor.eat(TokenKind::LParen) {
            let mut vals = Vec::new();
            loop {
                vals.push(self.parse_type_variable_value()?);
                if !self.cursor.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.cursor.expect(TokenKind::RParen)?;
            vals
        } else {
            vec![]
        };
        Ok(TypeReference {
            package: pkg,
            name,
            type_arguments,
            multiplicity_arguments,
            type_variable_values,
            source_info: start,
        })
    }

    /// Parses a type specification that can be a type, unit reference, or relation type.
    ///
    /// Used in positions where the Pure grammar accepts:
    /// - Regular types: `String`, `Map<K, V>`
    /// - Unit references: `NewMeasure~UnitOne`
    /// - Relation types: `(a:Integer, b:String)` or `Relation<(a:Integer, b:String)>`
    pub(crate) fn parse_type_spec(&mut self) -> R<TypeSpec> {
        // Function type: {ParamType[mult] -> ReturnType[mult]}
        if self.cursor.check(TokenKind::LBrace) {
            let ft_ref = self.parse_function_type_as_type_ref()?;
            // In TypeSpec context, we have the full FunctionType struct available
            // but parse_function_type_as_type_ref returns a TypeReference with
            // sentinel name. For simplicity, wrap it as TypeSpec::Type.
            return Ok(TypeSpec::Type(ft_ref));
        }

        // Bare relation type: (col:Type, ...)
        if self.cursor.check(TokenKind::LParen) {
            return self.parse_relation_type();
        }

        let start = self.cursor.current_source_info();

        // Parse the qualified name first (e.g. `Relation`, `meta::pure::Relation`).
        let path = self.parse_package_path()?;

        // Check for `<(` — this is column-spec syntax (e.g. `Relation<(a:Integer)>`).
        // We detect it structurally by token sequence rather than name-matching,
        // so user types named "Relation" with regular type args aren't affected.
        if self.cursor.check(TokenKind::Less) && self.cursor.peek_kind_at(1) == TokenKind::LParen {
            self.cursor.expect(TokenKind::Less)?;
            let columns = self.parse_relation_columns()?;
            self.cursor.expect_closing_angle_bracket()?;
            return Ok(TypeSpec::Relation(RelationType {
                columns,
                source_info: start,
            }));
        }

        // Standard type reference: parse remaining <TypeArgs>(TypeVarValues)
        let (pkg, name) = split_package_name(&path);
        let type_ref = self.finish_type_reference(start, pkg, name)?;

        if self.cursor.eat(TokenKind::Tilde) {
            let si = self.cursor.current_source_info();
            let (unit_name, _) = self.cursor.expect_identifier_or_keyword()?;
            Ok(TypeSpec::Unit(UnitReference {
                measure: type_ref,
                unit: unit_name,
                source_info: si,
            }))
        } else {
            Ok(TypeSpec::Type(type_ref))
        }
    }

    /// Parse a bare relation type: `(col:Type[mult]?, ...)`
    pub(crate) fn parse_relation_type(&mut self) -> R<TypeSpec> {
        let si = self.cursor.current_source_info();
        let columns = self.parse_relation_columns()?;
        Ok(TypeSpec::Relation(RelationType {
            columns,
            source_info: si,
        }))
    }

    /// Parse relation columns: `(name:Type[mult]?, ...)`
    ///
    /// Shared by both bare relation types and `Relation<(...)>` syntax.
    pub(crate) fn parse_relation_columns(&mut self) -> R<Vec<RelationColumn>> {
        self.cursor.expect(TokenKind::LParen)?;
        let mut cols = Vec::new();
        loop {
            let col_si = self.cursor.current_source_info();
            // `?` is the wildcard column-name placeholder (e.g.
            // `ColSpec<(?:Type)>` in eval.pure) — it stands for "any
            // column with this type, name unspecified". Capture as `?`
            // verbatim; the resolver treats wildcard names as positional.
            let col_name = if self.cursor.eat(TokenKind::Question) {
                SmolStr::new("?")
            } else {
                let (n, _) = self.cursor.expect_identifier_or_keyword()?;
                n
            };
            self.cursor.expect(TokenKind::Colon)?;
            let col_type = self.parse_type_reference()?;
            let multiplicity = if self.cursor.check(TokenKind::LBracket) {
                self.cursor.expect(TokenKind::LBracket)?;
                let mult = self.parse_multiplicity()?;
                self.cursor.expect(TokenKind::RBracket)?;
                Some(mult)
            } else {
                None
            };
            cols.push(RelationColumn {
                name: col_name,
                type_ref: col_type,
                multiplicity,
                source_info: col_si,
            });
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        self.cursor.expect(TokenKind::RParen)?;
        Ok(cols)
    }

    /// Parses a function type `{ParamType[mult], ... -> ReturnType[mult]}` and
    /// encodes it as a `TypeReference` with [`FUNCTION_TYPE_SENTINEL`] name.
    ///
    /// The encoding stores parameter types in `type_arguments`, each with its
    /// multiplicity in that type reference's `multiplicity_arguments[0]`.
    /// The return type is the last entry in `type_arguments`, with the return
    /// multiplicity in `multiplicity_arguments`.
    ///
    /// # Grammar
    ///
    /// ```text
    /// functionType: '{' (functionTypeParam (',' functionTypeParam)*)? '->' typeRef '[' mult ']' '}'
    /// functionTypeParam: typeRef '[' mult ']'
    /// ```
    pub(crate) fn parse_function_type_as_type_ref(&mut self) -> R<TypeReference> {
        let start = self.cursor.current_source_info();
        self.cursor.expect(TokenKind::LBrace)?;

        let mut param_types = Vec::new();

        // Parse parameter types until we hit `->`
        // Tricky: we need to distinguish `{-> RetType[m]}` (no params) from
        // `{ParamType[m] -> RetType[m]}` (one param).
        if !self.cursor.check(TokenKind::Arrow) {
            loop {
                let param_si = self.cursor.current_source_info();
                let param_type_ref = self.parse_type_reference()?;
                self.cursor.expect(TokenKind::LBracket)?;
                let param_mult = self.parse_multiplicity()?;
                self.cursor.expect(TokenKind::RBracket)?;

                // Encode param multiplicity into the type reference's multiplicity_arguments
                let mut param_with_mult = param_type_ref;
                param_with_mult
                    .multiplicity_arguments
                    .push(MultiplicityArgument::Concrete(param_mult, param_si));

                param_types.push(param_with_mult);

                if self.cursor.check(TokenKind::Arrow) {
                    break;
                }
                self.cursor.expect(TokenKind::Comma)?;
            }
        }

        self.cursor.expect(TokenKind::Arrow)?;

        // Return type and multiplicity
        let return_type = self.parse_type_reference()?;
        self.cursor.expect(TokenKind::LBracket)?;
        let return_mult = self.parse_multiplicity()?;
        self.cursor.expect(TokenKind::RBracket)?;

        self.cursor.expect(TokenKind::RBrace)?;

        // Assemble: param types + return type all in type_arguments.
        // Return multiplicity goes in the top-level multiplicity_arguments.
        param_types.push(return_type);

        Ok(TypeReference {
            package: None,
            name: SmolStr::new(FUNCTION_TYPE_SENTINEL),
            type_arguments: param_types,
            multiplicity_arguments: vec![MultiplicityArgument::Concrete(
                return_mult,
                start.clone(),
            )],
            type_variable_values: vec![],
            source_info: start,
        })
    }

    /// Parses optional type variable parameter definitions on classes and primitives: `(x:Integer[1], y:String[1])`.
    pub(crate) fn parse_type_variable_parameters(
        &mut self,
    ) -> R<Vec<legend_pure_parser_ast::type_ref::TypeVariableParameter>> {
        if !self.cursor.eat(TokenKind::LParen) {
            return Ok(vec![]);
        }

        let mut params = Vec::new();
        while !self.cursor.check(TokenKind::RParen) {
            let si = self.cursor.current_source_info();
            let (name, _) = self.cursor.expect_identifier_or_keyword()?;
            self.cursor.expect(TokenKind::Colon)?;
            let type_ref = self.parse_type_reference()?;
            self.cursor.expect(TokenKind::LBracket)?;
            let multiplicity = self.parse_multiplicity()?;
            self.cursor.expect(TokenKind::RBracket)?;

            params.push(legend_pure_parser_ast::type_ref::TypeVariableParameter {
                name,
                type_ref,
                multiplicity,
                source_info: si,
            });

            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        self.cursor.expect(TokenKind::RParen)?;
        Ok(params)
    }

    /// Parses optional `<TypeParams | MultParams>` on class/function declarations.
    ///
    /// Grammar:
    /// ```text
    /// typeAndMultiplicityParameters: '<' ((typeParameters multiplictyParameters?) | multiplictyParameters) '>'
    /// typeParameters:                identifier (',' identifier)*
    /// multiplictyParameters:         '|' identifier (',' identifier)*
    /// ```
    ///
    /// Returns `(type_params, mult_params)`. Both may be empty if no `<` is present.
    pub(crate) fn parse_type_and_multiplicity_parameters(
        &mut self,
    ) -> R<(Vec<Identifier>, Vec<Identifier>)> {
        if !self.cursor.eat(TokenKind::Less) {
            return Ok((vec![], vec![]));
        }

        // Check if it starts with `|` — that means no type params, only mult params.
        if self.cursor.check(TokenKind::Pipe) {
            self.cursor.advance(); // eat |
            let mut mult_params = Vec::new();
            loop {
                let (p, _) = self.cursor.expect_identifier()?;
                mult_params.push(p);
                if !self.cursor.eat(TokenKind::Comma) {
                    break;
                }
            }
            self.cursor.expect_closing_angle_bracket()?;
            return Ok((vec![], mult_params));
        }

        // Parse type parameters
        let mut type_params = Vec::new();
        loop {
            let (p, _) = self.cursor.expect_identifier()?;
            type_params.push(p);
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }

        // Optional multiplicity parameters after |
        let mult_params = if self.cursor.eat(TokenKind::Pipe) {
            let mut params = Vec::new();
            loop {
                let (p, _) = self.cursor.expect_identifier()?;
                params.push(p);
                if !self.cursor.eat(TokenKind::Comma) {
                    break;
                }
            }
            params
        } else {
            vec![]
        };

        self.cursor.expect_closing_angle_bracket()?;
        Ok((type_params, mult_params))
    }

    /// Parses multiplicity arguments after `|` in a type reference: `<TypeArgs | MultArgs>`.
    ///
    /// Grammar:
    /// ```text
    /// multiplicityArguments: multiplicityArgument (',' multiplicityArgument)*
    /// multiplicityArgument: identifier | ((fromMultiplicity '..')? toMultiplicity)
    /// ```
    pub(crate) fn parse_multiplicity_arguments(&mut self) -> R<Vec<MultiplicityArgument>> {
        let mut args = Vec::new();
        loop {
            args.push(self.parse_multiplicity_argument()?);
            if !self.cursor.eat(TokenKind::Comma) {
                break;
            }
        }
        Ok(args)
    }

    /// Parses a single multiplicity argument: either an identifier or a concrete multiplicity.
    pub(crate) fn parse_multiplicity_argument(&mut self) -> R<MultiplicityArgument> {
        let si = self.cursor.current_source_info();
        match self.cursor.peek_kind() {
            TokenKind::Star => {
                self.cursor.advance();
                Ok(MultiplicityArgument::Concrete(
                    Multiplicity::zero_or_many(),
                    si,
                ))
            }
            TokenKind::IntegerLiteral => {
                let lo_tok = self.cursor.advance().clone();
                let lo: u32 = lo_tok.text.parse().unwrap_or(0);
                if self.cursor.check(TokenKind::Dot) {
                    self.cursor.advance(); // first dot
                    self.cursor.expect(TokenKind::Dot)?; // second dot
                    if self.cursor.check(TokenKind::Star) {
                        self.cursor.advance();
                        Ok(MultiplicityArgument::Concrete(
                            Multiplicity::range(lo, None),
                            si,
                        ))
                    } else {
                        let hi_tok = self.cursor.expect(TokenKind::IntegerLiteral)?;
                        let hi: u32 = hi_tok.text.parse().unwrap_or(lo);
                        Ok(MultiplicityArgument::Concrete(
                            Multiplicity::range(lo, Some(hi)),
                            si,
                        ))
                    }
                } else {
                    Ok(MultiplicityArgument::Concrete(
                        Multiplicity::range(lo, Some(lo)),
                        si,
                    ))
                }
            }
            TokenKind::Identifier => {
                let (id, _) = self.cursor.expect_identifier()?;
                Ok(MultiplicityArgument::Identifier(id, si))
            }
            _ => Err(ParseError::expected(
                "multiplicity argument (identifier, integer, or *)",
                self.cursor.peek_kind(),
                si,
            )),
        }
    }

    pub(crate) fn parse_type_variable_value(&mut self) -> R<TypeVariableValue> {
        let si = self.cursor.current_source_info();
        match self.cursor.peek_kind() {
            TokenKind::IntegerLiteral => {
                let tok = self.cursor.advance().clone();
                let val: i64 = tok.text.parse().unwrap_or(0);
                Ok(TypeVariableValue::Integer(val, si))
            }
            TokenKind::StringLiteral => {
                let tok = self.cursor.advance().clone();
                Ok(TypeVariableValue::String(unquote_string(&tok.text), si))
            }
            _ => {
                let (id, _) = self.cursor.expect_identifier()?;
                Ok(TypeVariableValue::String(id.to_string(), si))
            }
        }
    }

    pub(crate) fn parse_multiplicity(&mut self) -> R<Multiplicity> {
        match self.cursor.peek_kind() {
            TokenKind::Star => {
                self.cursor.advance();
                Ok(Multiplicity::zero_or_many())
            }
            TokenKind::IntegerLiteral => {
                let lo_tok = self.cursor.advance().clone();
                let lo: u32 = lo_tok.text.parse().unwrap_or(0);
                if self.cursor.eat(TokenKind::Dot) {
                    self.cursor.expect(TokenKind::Dot)?;
                    if self.cursor.check(TokenKind::Star) {
                        self.cursor.advance();
                        Ok(Multiplicity::range(lo, None))
                    } else {
                        let hi_tok = self.cursor.expect(TokenKind::IntegerLiteral)?;
                        let hi: u32 = hi_tok.text.parse().unwrap_or(lo);
                        Ok(Multiplicity::range(lo, Some(hi)))
                    }
                } else {
                    Ok(Multiplicity::range(lo, Some(lo)))
                }
            }
            TokenKind::Identifier => {
                let tok = self.cursor.advance().clone();
                Ok(Multiplicity::Variable(SmolStr::from(&*tok.text)))
            }
            _ => Err(ParseError::expected(
                "multiplicity",
                self.cursor.peek_kind(),
                self.cursor.current_source_info(),
            )),
        }
    }
}
