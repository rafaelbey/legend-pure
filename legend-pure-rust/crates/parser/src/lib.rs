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
//!
//! # Parallel Parsing
//!
//! For multi-file workloads, use [`parse_many`] with [`SourceProvider`](source::SourceProvider)
//! implementations to parse files concurrently across all available CPU cores:
//!
//! ```
//! use legend_pure_parser_parser::{parse_many, source::SourceInput};
//!
//! let sources = vec![
//!     SourceInput::in_memory("a.pure", "Class A {}"),
//!     SourceInput::in_memory("b.pure", "Class B {}"),
//! ];
//! let outputs = parse_many(&sources);
//! assert!(outputs.iter().all(|o| o.is_ok()));
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod cursor;
pub mod error;
pub mod island;
mod parser;
pub mod source;

use legend_pure_parser_ast::SourceFile;

pub use error::ParseError;
pub use island::IslandParser;
pub use source::SourceProvider;

/// Parse Pure source text into an AST [`SourceFile`].
///
/// Uses the default set of island grammar plugins (currently graph fetch).
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

/// Parse Pure source text with a custom set of island grammar plugins.
///
/// Use this when you need to register additional island grammars beyond
/// the built-in set (e.g., path expressions, embedded SQL).
///
/// # Errors
///
/// Returns `Err` if the source contains lexer or parser errors.
pub fn parse_with_islands(
    source: &str,
    source_name: &str,
    island_parsers: Vec<Box<dyn IslandParser>>,
) -> Result<SourceFile, ParseError> {
    let tokens = legend_pure_parser_lexer::tokenize(source, source_name)?;
    let cursor = cursor::Cursor::new(tokens);
    let mut p = parser::Parser::with_island_parsers(cursor, island_parsers);
    p.parse_source_file()
}

/// Result of loading and parsing a single source via [`parse_many`].
///
/// Carries the source name and loaded text alongside the parse result
/// so that callers can render error diagnostics without re-loading the file.
#[derive(Debug)]
pub struct ParseOutput {
    /// The logical source name (from [`SourceProvider::name`]).
    pub name: String,
    /// The loaded source text, or `None` if loading failed.
    pub source_text: Option<String>,
    /// The parse outcome.
    pub outcome: ParseOutcome,
}

/// Outcome of loading + parsing a single source.
#[derive(Debug)]
pub enum ParseOutcome {
    /// Source loaded and parsed successfully.
    Success(SourceFile),
    /// Source loaded but parsing failed.
    ParseError(ParseError),
    /// Source could not be loaded (I/O error).
    IoError(std::io::Error),
}

impl ParseOutput {
    /// Returns `true` if the source was loaded and parsed successfully.
    #[must_use]
    pub fn is_ok(&self) -> bool {
        matches!(self.outcome, ParseOutcome::Success(_))
    }

    /// Returns the AST if parsing succeeded, `None` otherwise.
    #[must_use]
    pub fn ast(&self) -> Option<&SourceFile> {
        match &self.outcome {
            ParseOutcome::Success(ast) => Some(ast),
            _ => None,
        }
    }

    /// Consumes self and returns the AST if parsing succeeded.
    #[must_use]
    pub fn into_ast(self) -> Option<SourceFile> {
        match self.outcome {
            ParseOutcome::Success(ast) => Some(ast),
            _ => None,
        }
    }
}

/// Parse multiple sources in parallel using all available CPU cores.
///
/// Accepts any slice of [`SourceProvider`] implementations. Each source is
/// loaded ([`source_text()`](SourceProvider::source_text)) and parsed in
/// parallel via Rayon. Results are returned in the **same order** as the
/// input, making output deterministic regardless of scheduling.
///
/// This is the canonical entry point for parallel parsing — CLI commands,
/// benchmarks, and IDE integrations should use this rather than manually
/// calling `rayon::par_iter`.
///
/// # Example
///
/// ```
/// use legend_pure_parser_parser::{parse_many, source::SourceInput};
///
/// let sources = vec![
///     SourceInput::in_memory("a.pure", "Class pkg::A {}"),
///     SourceInput::in_memory("b.pure", "Class pkg::B {}"),
/// ];
/// let outputs = parse_many(&sources);
/// assert_eq!(outputs.len(), 2);
/// assert!(outputs[0].is_ok());
/// assert!(outputs[1].is_ok());
/// ```
pub fn parse_many<S: SourceProvider>(sources: &[S]) -> Vec<ParseOutput> {
    use rayon::prelude::*;

    sources
        .par_iter()
        .map(|source| {
            let name = source.name().to_string();
            match source.source_text() {
                Ok(text) => {
                    let text_str = text.into_owned();
                    let outcome = match parse(&text_str, &name) {
                        Ok(ast) => ParseOutcome::Success(ast),
                        Err(e) => ParseOutcome::ParseError(e),
                    };
                    ParseOutput {
                        name,
                        source_text: Some(text_str),
                        outcome,
                    }
                }
                Err(io_err) => ParseOutput {
                    name,
                    source_text: None,
                    outcome: ParseOutcome::IoError(io_err),
                },
            }
        })
        .collect()
}
