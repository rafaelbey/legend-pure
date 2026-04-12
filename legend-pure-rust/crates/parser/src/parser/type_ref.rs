use super::Parser;
use super::R;
use super::{split_package_name, unquote_string};
use crate::error::ParseError;
use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::type_ref::{
    Identifier, Multiplicity, MultiplicityArgument, Package, RelationColumn, RelationType,
    TypeReference, TypeSpec, TypeVariableValue, UnitReference,
};
use legend_pure_parser_lexer::TokenKind;
use smol_str::SmolStr;

impl Parser {
    // ── Type references ─────────────────────────────────────────────────

    pub(crate) fn parse_type_reference(&mut self) -> R<TypeReference> {
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
            } else {
                loop {
                    args.push(self.parse_type_reference()?);
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
            self.cursor.expect(TokenKind::Greater)?;
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
            self.cursor.expect(TokenKind::Greater)?;
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
            let (col_name, _) = self.cursor.expect_identifier_or_keyword()?;
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
            self.cursor.expect(TokenKind::Greater)?;
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

        self.cursor.expect(TokenKind::Greater)?;
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
