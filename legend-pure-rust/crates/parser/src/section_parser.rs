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

//! Section grammar parser plug-in infrastructure.
//!
//! The Pure file format separates declarations into sections via
//! `###Identifier` headers (`###Pure`, `###Diagram`, `###Mapping`,
//! …). The default `Pure` section parses M3 elements (Class,
//! Function, Profile, …). Other section kinds belong to M2 DSLs and
//! aren't part of M3's grammar — each DSL crate registers a
//! [`SectionParser`] that the core parser dispatches to when it sees
//! the matching section header.
//!
//! This trait is the section-level analog of
//! [`IslandParser`](crate::island::IslandParser):
//!
//! | Scope | Trait | Body shape |
//! |---|---|---|
//! | Inline (`#tag{ … }#`) | `IslandParser` | `Box<dyn IslandContent>` |
//! | Section (`###Tag\n …`) | `SectionParser` | `Vec<Box<dyn DSLElement>>` |
//!
//! # Adding a new DSL section
//!
//! 1. Define your AST types implementing
//!    [`DSLElement`](legend_pure_parser_ast::dsl::DSLElement).
//! 2. Implement [`SectionParser`] for your DSL.
//! 3. Pass it via [`parse_with_sections`](crate::parse_with_sections).
//!
//! # Thread-safety
//!
//! Implementations must be `Send + Sync` (matching [`IslandParser`])
//! because the CLI parallelises file parsing via Rayon.

use legend_pure_parser_ast::dsl::DSLElement;

use crate::ParserContext;
use crate::error::ParseError;

/// Plug-in for parsing the body of a non-`Pure` section (e.g. `###Diagram`).
///
/// The core parser handles section detection, header parsing, and
/// import statements. Once the body begins, dispatch delegates to a
/// matching `SectionParser` if one is registered for the section
/// kind. The plug-in consumes tokens through the supplied
/// [`ParserContext`] up to the next section boundary or EOF and
/// returns the parsed [`DSLElement`]s.
///
/// `ParserContext` is the same handle that island parsers receive —
/// it gives both raw cursor access (`ctx.cursor()`) and high-level
/// helpers (`ctx.parse_expression()`, `ctx.parse_qualified_name()`,
/// …). DSLs whose body grammar embeds Pure expressions (Mapping's
/// filter and transform lambdas, future Function-DSL bodies) need
/// `parse_expression`; lower-level DSLs (Diagram) can ignore the
/// helpers and just use the cursor.
pub trait SectionParser: Send + Sync {
    /// Section header that this parser handles, without the leading
    /// `###`. For example, `"Diagram"` matches `###Diagram`.
    fn kind(&self) -> &str;

    /// Parse the body of a section of [`kind`](Self::kind). Consumes
    /// from `ctx.cursor()` up to (but not past) the next section
    /// header or EOF.
    ///
    /// Element-level errors are pushed onto `errors`; the parser is
    /// expected to recover and continue rather than abort the whole
    /// section.
    fn parse_body(
        &self,
        ctx: &mut ParserContext<'_>,
        errors: &mut Vec<ParseError>,
    ) -> Vec<Box<dyn DSLElement>>;
}
