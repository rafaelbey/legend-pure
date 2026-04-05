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

//! # Legend Pure Parser — Parser
//!
//! Recursive descent parser for the Pure grammar. Consumes tokens from the lexer
//! and produces an AST.
//!
//! # Usage
//!
//! ```
//! use legend_pure_parser_parser::parse;
//!
//! let source = "###Pure\nProfile my::doc { stereotypes: [deprecated]; tags: [todo]; }";
//! let file = parse(source, "test.pure").unwrap();
//! assert_eq!(file.element_count(), 1);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod cursor;
pub mod error;
mod parser;

use legend_pure_parser_ast::SourceFile;

pub use error::ParseError;

/// Parse Pure source text into an AST [`SourceFile`].
///
/// # Errors
///
/// Returns `Err` if the source contains lexer or parser errors.
pub fn parse(source: &str, source_name: &str) -> Result<SourceFile, ParseError> {
    let tokens = legend_pure_parser_lexer::tokenize(source, source_name)?;
    let cursor = cursor::Cursor::new(tokens);
    let mut p = parser::Parser::new(cursor);
    p.parse_source_file()
}
