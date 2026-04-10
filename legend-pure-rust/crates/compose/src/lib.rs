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

//! AST to Pure grammar text composer.
//!
//! This crate provides the "compose" direction: taking an AST produced by
//! the parser (or reconstructed from protocol JSON) and rendering it back
//! to syntactically valid, canonically formatted Pure grammar text.
//!
//! # Usage
//!
//! ```rust,ignore
//! use legend_pure_parser_compose::compose_source_file;
//!
//! let source_file = parser::parse(source, "test.pure").unwrap();
//! let text = compose_source_file(&source_file);
//! ```
//!
//! # Design
//!
//! - **AST-driven**: Walks the AST directly — no intermediate protocol model.
//! - **Canonical formatting**: Output is deterministic with 2-space indentation.
//! - **Operator precedence**: Emits minimal-but-correct parentheses.
//! - **Identifier quoting**: Quotes identifiers that need it (`'30_360'`).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod element;
pub mod expression;
pub mod identifier;
pub mod island;
pub mod section;
pub mod type_ref;
pub mod writer;

pub use section::compose_source_file;

// ---------------------------------------------------------------------------
// Protocol convenience functions
// ---------------------------------------------------------------------------

/// Composes a `PureModelContextData` (protocol model) directly to Pure grammar text.
///
/// This is the Rust equivalent of Java's
/// `PureGrammarComposer.renderPureModelContextData()`.
///
/// Requires the `protocol` feature.
///
/// # Errors
///
/// Returns an error if the protocol-to-AST conversion fails.
#[cfg(feature = "protocol")]
pub fn compose_from_protocol(
    pmcd: &legend_pure_parser_protocol::v1::context::PureModelContextData,
) -> Result<String, legend_pure_parser_protocol::v1::from_protocol::ConversionError> {
    let source_file =
        legend_pure_parser_protocol::v1::from_protocol::convert_context_to_source_file(pmcd)?;
    Ok(compose_source_file(&source_file))
}

/// Composes Protocol JSON (as a string) directly to Pure grammar text.
///
/// This is the most convenient function for the full JSON → grammar pipeline.
///
/// Requires the `protocol` feature.
///
/// # Errors
///
/// Returns an error if JSON deserialization or protocol conversion fails.
#[cfg(feature = "protocol")]
pub fn compose_from_json(json: &str) -> Result<String, Box<dyn std::error::Error>> {
    let pmcd: legend_pure_parser_protocol::v1::context::PureModelContextData =
        serde_json::from_str(json)?;
    let grammar = compose_from_protocol(&pmcd)?;
    Ok(grammar)
}
