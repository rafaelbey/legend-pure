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

//! Hand-written tokenizer for the Pure grammar.
//!
//! The lexer operates on a UTF-8 string and produces a `Vec<Token>` with source spans.
//! It handles:
//! - Keywords and identifiers (including quoted `'with spaces'`)
//! - Numeric literals (integer, float, decimal with `D` suffix)
//! - String literals (single-quoted with `\'` escape)
//! - Date/time literals (`%2024-01-15`, `%10:30:00`)
//! - Section headers (`###Pure`)
//! - Island grammar markers (`#{`, `}#`)
//! - Comments (`//` line, `/* */` block)

use legend_pure_parser_ast::SourceInfo;
use smol_str::SmolStr;

use crate::token::{Token, TokenKind};
use crate::LexError;

/// Tokenizes Pure source text into a vector of tokens.
///
/// Skips whitespace and comments. The returned token stream always ends
/// with [`TokenKind::Eof`].
///
/// # Errors
///
/// Returns `Err` for malformed tokens (unterminated strings, invalid characters).
pub fn tokenize(source: &str, source_name: &str) -> Result<Vec<Token>, LexError> {
    let mut lexer = Lexer::new(source, source_name);
    let mut tokens = Vec::new();

    loop {
        let token = lexer.next_token()?;
        let is_eof = token.kind == TokenKind::Eof;
        tokens.push(token);
        if is_eof {
            break;
        }
    }

    Ok(tokens)
}

/// Stateful cursor over source text.
struct Lexer<'a> {
    /// The full source text.
    source: &'a str,
    /// Source identifier (file name).
    source_name: SmolStr,
    /// Current byte offset in source.
    pos: usize,
    /// Current line (1-indexed).
    line: u32,
    /// Current column (1-indexed).
    column: u32,
}

impl<'a> Lexer<'a> {
    fn new(source: &'a str, source_name: &str) -> Self {
        Self {
            source,
            source_name: SmolStr::new(source_name),
            pos: 0,
            line: 1,
            column: 1,
        }
    }

    // -- Cursor helpers -------------------------------------------------------

    /// Returns the current byte without advancing, or `None` at EOF.
    fn peek(&self) -> Option<char> {
        self.source[self.pos..].chars().next()
    }

    /// Returns the byte at `pos + offset` without advancing.
    fn peek_at(&self, offset: usize) -> Option<char> {
        self.source[self.pos..].chars().nth(offset)
    }

    /// Advances the cursor by one character.
    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    /// Returns true if the cursor has reached the end.
    fn is_at_end(&self) -> bool {
        self.pos >= self.source.len()
    }

    /// Creates a `SourceInfo` from a saved start position to the current position.
    fn span_from(&self, start_line: u32, start_column: u32) -> SourceInfo {
        SourceInfo::new(
            self.source_name.clone(),
            start_line,
            start_column,
            self.line,
            // column is 1 past the last char, so subtract 1 for inclusive end
            if self.column > 1 {
                self.column - 1
            } else {
                self.column
            },
        )
    }

    /// Extracts the text from `start_pos` to the current position.
    fn text_from(&self, start_pos: usize) -> SmolStr {
        SmolStr::new(&self.source[start_pos..self.pos])
    }

    // -- Skip helpers ---------------------------------------------------------

    fn skip_whitespace(&mut self) {
        while let Some(ch) = self.peek() {
            if ch.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_line_comment(&mut self) {
        while let Some(ch) = self.advance() {
            if ch == '\n' {
                break;
            }
        }
    }

    fn skip_block_comment(&mut self) -> Result<(), LexError> {
        let start_line = self.line;
        let start_col = self.column;
        // Skip past the opening /*
        self.advance(); // *
        loop {
            match self.advance() {
                Some('*') if self.peek() == Some('/') => {
                    self.advance(); // /
                    return Ok(());
                }
                Some(_) => {}
                Option::None => {
                    return Err(LexError::UnterminatedBlockComment {
                        source_info: self.span_from(start_line, start_col),
                    });
                }
            }
        }
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), LexError> {
        loop {
            self.skip_whitespace();
            if self.peek() == Some('/') {
                match self.peek_at(1) {
                    Some('/') => {
                        self.advance(); // first /
                        self.advance(); // second /
                        self.skip_line_comment();
                    }
                    Some('*') => {
                        self.advance(); // /
                        self.skip_block_comment()?;
                    }
                    _ => break,
                }
            } else {
                break;
            }
        }
        Ok(())
    }

    // -- Token production -----------------------------------------------------

    /// Produces the next token, skipping whitespace and comments.
    #[allow(clippy::too_many_lines)]
    fn next_token(&mut self) -> Result<Token, LexError> {
        self.skip_whitespace_and_comments()?;

        if self.is_at_end() {
            return Ok(Token {
                kind: TokenKind::Eof,
                text: SmolStr::default(),
                source_info: self.span_from(self.line, self.column),
            });
        }

        let start_line = self.line;
        let start_col = self.column;
        let start_pos = self.pos;

        let ch = self.advance().unwrap();

        let kind = match ch {
            // -- Section header: ### --
            '#' if self.peek() == Some('#') && self.peek_at(1) == Some('#') => {
                self.advance(); // second #
                self.advance(); // third #
                // Read the section name (identifier chars)
                while let Some(c) = self.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        self.advance();
                    } else {
                        break;
                    }
                }
                TokenKind::SectionHeader
            }

            // -- Island markers: #{ --
            '#' if self.peek() == Some('{') => {
                self.advance();
                TokenKind::HashLBrace
            }

            // -- Delimiters --
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' if self.peek() == Some('#') => {
                self.advance();
                TokenKind::RBraceHash
            }
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ';' => TokenKind::Semicolon,
            ',' => TokenKind::Comma,
            '.' => TokenKind::Dot,
            '@' => TokenKind::At,
            '$' => TokenKind::Dollar,
            '~' => TokenKind::Tilde,
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,

            // -- : or :: --
            ':' if self.peek() == Some(':') => {
                self.advance();
                TokenKind::PathSep
            }
            ':' => TokenKind::Colon,

            // -- - or -> --
            '-' if self.peek() == Some('>') => {
                self.advance();
                TokenKind::Arrow
            }
            '-' => TokenKind::Minus,

            // -- = or == or => --
            '=' if self.peek() == Some('=') => {
                self.advance();
                TokenKind::EqualEqual
            }
            '=' if self.peek() == Some('>') => {
                self.advance();
                TokenKind::FatArrow
            }
            '=' => TokenKind::Equals,

            // -- ! or != --
            '!' if self.peek() == Some('=') => {
                self.advance();
                TokenKind::BangEqual
            }
            '!' => TokenKind::Bang,

            // -- < or <= or << --
            '<' if self.peek() == Some('=') => {
                self.advance();
                TokenKind::LessEqual
            }
            '<' if self.peek() == Some('<') => {
                self.advance();
                TokenKind::LessLess
            }
            '<' => TokenKind::Less,

            // -- > or >= or >> --
            '>' if self.peek() == Some('=') => {
                self.advance();
                TokenKind::GreaterEqual
            }
            '>' if self.peek() == Some('>') => {
                self.advance();
                TokenKind::GreaterGreater
            }
            '>' => TokenKind::Greater,

            // -- & or && --
            '&' if self.peek() == Some('&') => {
                self.advance();
                TokenKind::AmpAmp
            }
            '&' => TokenKind::Amp,

            // -- | or || --
            '|' if self.peek() == Some('|') => {
                self.advance();
                TokenKind::PipePipe
            }
            '|' => TokenKind::Pipe,

            // -- ^ (caret/xor) --
            '^' => TokenKind::Caret,

            // -- / (slash, comments already handled above) --
            '/' => TokenKind::Slash,

            // -- String literal: 'hello' with \' escapes --
            '\'' => self.lex_string(start_line, start_col)?,

            // -- Date literal: %2024-01-15, %10:30:00 --
            '%' => self.lex_date_or_percent(start_line, start_col),

            // -- Numeric literal: 42, 3.14, 1.0D --
            c if c.is_ascii_digit() => self.lex_number(start_line, start_col),

            // -- Identifier or keyword --
            c if is_ident_start(c) => self.lex_identifier(start_pos),

            _ => {
                return Err(LexError::UnexpectedCharacter {
                    ch,
                    source_info: self.span_from(start_line, start_col),
                });
            }
        };

        Ok(Token {
            kind,
            text: self.text_from(start_pos),
            source_info: self.span_from(start_line, start_col),
        })
    }

    // -- Complex token lexers -------------------------------------------------

    fn lex_string(&mut self, start_line: u32, start_col: u32) -> Result<TokenKind, LexError> {
        loop {
            match self.advance() {
                Some('\\') => {
                    // Skip escaped character
                    self.advance();
                }
                Some('\'') => return Ok(TokenKind::StringLiteral),
                Some(_) => {}
                Option::None => {
                    return Err(LexError::UnterminatedString {
                        source_info: self.span_from(start_line, start_col),
                    });
                }
            }
        }
    }

    fn lex_date_or_percent(&mut self, _start_line: u32, _start_col: u32) -> TokenKind {
        // Date literal: %YYYY-MM-DD or %HH:MM:SS or %YYYY-MM-DDTHH:MM:SS
        // If no digit follows %, it's just a percent operator
        if self.peek().is_some_and(|c| c.is_ascii_digit()) {
            while let Some(c) = self.peek() {
                if c.is_ascii_digit()
                    || c == '-'
                    || c == 'T'
                    || c == ':'
                    || c == '.'
                    || c == '+'
                    || c == 'Z'
                {
                    self.advance();
                } else {
                    break;
                }
            }
            TokenKind::DateLiteral
        } else {
            TokenKind::Percent
        }
    }

    fn lex_number(&mut self, _start_line: u32, _start_col: u32) -> TokenKind {
        // Consume digits
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }

        // Check for decimal point
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.advance(); // consume '.'
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.advance();
            }

            // Check for decimal suffix 'D'
            if self.peek() == Some('D') || self.peek() == Some('d') {
                self.advance();
                return TokenKind::DecimalLiteral;
            }

            return TokenKind::FloatLiteral;
        }

        TokenKind::IntegerLiteral
    }

    fn lex_identifier(&mut self, start_pos: usize) -> TokenKind {
        while self.peek().is_some_and(is_ident_continue) {
            self.advance();
        }

        let text = &self.source[start_pos..self.pos];
        TokenKind::keyword(text).unwrap_or(TokenKind::Identifier)
    }
}

/// Returns true if `ch` can start an identifier.
fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

/// Returns true if `ch` can continue an identifier.
fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        tokenize(source, "test.pure")
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect()
    }

    fn texts(source: &str) -> Vec<String> {
        tokenize(source, "test.pure")
            .unwrap()
            .into_iter()
            .map(|t| t.text.to_string())
            .collect()
    }

    // -- Section headers --

    #[test]
    fn section_header() {
        let tokens = tokenize("###Pure", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::SectionHeader);
        assert_eq!(tokens[0].text, "###Pure");
    }

    #[test]
    fn section_header_mapping() {
        let tokens = tokenize("###Mapping", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::SectionHeader);
        assert_eq!(tokens[0].text, "###Mapping");
    }

    // -- Keywords --

    #[test]
    fn keywords() {
        assert_eq!(
            kinds("Class Enum Profile Association Measure function"),
            vec![
                TokenKind::Class,
                TokenKind::Enum,
                TokenKind::Profile,
                TokenKind::Association,
                TokenKind::Measure,
                TokenKind::Function,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn other_keywords() {
        assert_eq!(
            kinds("import extends let native true false"),
            vec![
                TokenKind::Import,
                TokenKind::Extends,
                TokenKind::Let,
                TokenKind::Native,
                TokenKind::True,
                TokenKind::False,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn profile_body_keywords() {
        assert_eq!(
            kinds("stereotypes tags"),
            vec![TokenKind::Stereotypes, TokenKind::Tags, TokenKind::Eof]
        );
    }

    // -- Identifiers --

    #[test]
    fn identifier() {
        let tokens = tokenize("myVar _private FOO_BAR", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::Identifier);
        assert_eq!(tokens[0].text, "myVar");
        assert_eq!(tokens[1].kind, TokenKind::Identifier);
        assert_eq!(tokens[1].text, "_private");
        assert_eq!(tokens[2].kind, TokenKind::Identifier);
        assert_eq!(tokens[2].text, "FOO_BAR");
    }

    // -- String literals --

    #[test]
    fn string_literal() {
        let tokens = tokenize("'hello world'", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[0].text, "'hello world'");
    }

    #[test]
    fn string_with_escape() {
        let tokens = tokenize(r"'test1\'s'", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::StringLiteral);
        assert_eq!(tokens[0].text, r"'test1\'s'");
    }

    #[test]
    fn unterminated_string() {
        let result = tokenize("'unterminated", "test.pure");
        assert!(result.is_err());
    }

    // -- Numeric literals --

    #[test]
    fn integer_literal() {
        let tokens = tokenize("42", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::IntegerLiteral);
        assert_eq!(tokens[0].text, "42");
    }

    #[test]
    fn float_literal() {
        let tokens = tokenize("3.14", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::FloatLiteral);
        assert_eq!(tokens[0].text, "3.14");
    }

    #[test]
    fn decimal_literal() {
        let tokens = tokenize("1.0D", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DecimalLiteral);
        assert_eq!(tokens[0].text, "1.0D");
    }

    // -- Date literals --

    #[test]
    fn date_literal_strict_date() {
        let tokens = tokenize("%2024-01-15", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DateLiteral);
        assert_eq!(tokens[0].text, "%2024-01-15");
    }

    #[test]
    fn date_literal_datetime() {
        let tokens = tokenize("%2024-01-15T10:30:00", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DateLiteral);
        assert_eq!(tokens[0].text, "%2024-01-15T10:30:00");
    }

    #[test]
    fn date_literal_strict_time() {
        let tokens = tokenize("%10:30:00", "test.pure").unwrap();
        assert_eq!(tokens[0].kind, TokenKind::DateLiteral);
        assert_eq!(tokens[0].text, "%10:30:00");
    }

    // -- Operators --

    #[test]
    fn two_char_operators() {
        assert_eq!(
            kinds("-> :: == != <= >= << >> && || =>"),
            vec![
                TokenKind::Arrow,
                TokenKind::PathSep,
                TokenKind::EqualEqual,
                TokenKind::BangEqual,
                TokenKind::LessEqual,
                TokenKind::GreaterEqual,
                TokenKind::LessLess,
                TokenKind::GreaterGreater,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::FatArrow,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn single_char_operators() {
        assert_eq!(
            kinds("+ - * / = ! < > & | ^ ~ @ $"),
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Equals,
                TokenKind::Bang,
                TokenKind::Less,
                TokenKind::Greater,
                TokenKind::Amp,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::At,
                TokenKind::Dollar,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn percent_vs_date() {
        // % followed by non-digit is the percent operator
        assert_eq!(
            kinds("% x"),
            vec![TokenKind::Percent, TokenKind::Identifier, TokenKind::Eof]
        );
        // % followed by digit starts a date literal
        assert_eq!(
            kinds("%2024-01-15"),
            vec![TokenKind::DateLiteral, TokenKind::Eof]
        );
    }

    // -- Delimiters --

    #[test]
    fn delimiters() {
        assert_eq!(
            kinds("( ) { } [ ] ; , : ."),
            vec![
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Semicolon,
                TokenKind::Comma,
                TokenKind::Colon,
                TokenKind::Dot,
                TokenKind::Eof,
            ]
        );
    }

    // -- Island markers --

    #[test]
    fn island_markers() {
        assert_eq!(
            kinds("#{ }#"),
            vec![TokenKind::HashLBrace, TokenKind::RBraceHash, TokenKind::Eof]
        );
    }

    // -- Comments --

    #[test]
    fn line_comment() {
        assert_eq!(
            kinds("Class // comment\nEnum"),
            vec![TokenKind::Class, TokenKind::Enum, TokenKind::Eof]
        );
    }

    #[test]
    fn block_comment() {
        assert_eq!(
            kinds("Class /* block\ncomment */ Enum"),
            vec![TokenKind::Class, TokenKind::Enum, TokenKind::Eof]
        );
    }

    #[test]
    fn unterminated_block_comment() {
        let result = tokenize("/* unterminated", "test.pure");
        assert!(result.is_err());
    }

    // -- Source info --

    #[test]
    fn source_info_tracking() {
        let tokens = tokenize("Class\n  Person", "test.pure").unwrap();
        assert_eq!(tokens[0].source_info.start_line, 1);
        assert_eq!(tokens[0].source_info.start_column, 1);
        assert_eq!(tokens[0].source_info.end_column, 5);
        assert_eq!(tokens[1].source_info.start_line, 2);
        assert_eq!(tokens[1].source_info.start_column, 3);
    }

    // -- Full token stream for a real-ish input --

    #[test]
    fn profile_definition() {
        let source = "###Pure\nProfile my::doc\n{\n    stereotypes: [deprecated];\n}";
        let tks = texts(source);
        assert_eq!(
            tks,
            vec![
                "###Pure", "Profile", "my", "::", "doc", "{", "stereotypes", ":", "[",
                "deprecated", "]", ";", "}", ""
            ]
        );
    }

    #[test]
    fn class_with_stereotype() {
        let source = "<<temporal.bt>> Class my::P { name: String[1]; }";
        assert_eq!(
            kinds(source),
            vec![
                TokenKind::LessLess,
                TokenKind::Identifier, // temporal
                TokenKind::Dot,
                TokenKind::Identifier, // bt
                TokenKind::GreaterGreater,
                TokenKind::Class,
                TokenKind::Identifier, // my
                TokenKind::PathSep,
                TokenKind::Identifier, // P
                TokenKind::LBrace,
                TokenKind::Identifier, // name
                TokenKind::Colon,
                TokenKind::Identifier, // String
                TokenKind::LBracket,
                TokenKind::IntegerLiteral, // 1
                TokenKind::RBracket,
                TokenKind::Semicolon,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn arrow_and_new_instance() {
        let source = "$x->cast(@Float); ^Cls(a='b')";
        assert_eq!(
            kinds(source),
            vec![
                TokenKind::Dollar,
                TokenKind::Identifier, // x
                TokenKind::Arrow,
                TokenKind::Identifier, // cast
                TokenKind::LParen,
                TokenKind::At,
                TokenKind::Identifier, // Float
                TokenKind::RParen,
                TokenKind::Semicolon,
                TokenKind::Caret,
                TokenKind::Identifier, // Cls
                TokenKind::LParen,
                TokenKind::Identifier, // a
                TokenKind::Equals,
                TokenKind::StringLiteral, // 'b'
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unexpected_character() {
        let result = tokenize("Class ¿", "test.pure");
        assert!(result.is_err());
    }
}
