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

//! # Legend Pure — Diagram DSL
//!
//! Self-contained crate providing the four pieces that make a Legend
//! Pure DSL plug-in:
//!
//! 1. [`ast`] — top-level [`DiagramDef`](ast::DiagramDef) AST type and
//!    its supporting view structs (`TypeView`, `AssociationView`,
//!    `PropertyView`, `GeneralizationView`). Every public element
//!    implements [`legend_pure_parser_ast::dsl::DSLElement`] so it
//!    rides on the core `Element::DSLElement` variant.
//! 2. **Parser** — [`DiagramSectionParser`](parser::DiagramSectionParser)
//!    plugs into [`legend_pure_parser_parser::SectionParser`] and
//!    consumes `###Diagram` section bodies.
//! 3. **Composer** — [`compose_diagram`](compose::compose_diagram)
//!    round-trips the AST back to canonical Pure source.
//! 4. **Compiler extension** —
//!    [`DiagramExtension`](compiler::DiagramExtension) implements
//!    [`legend_pure_parser_pure::extension::CompilerExtension`] to
//!    lower DiagramDef AST into compiled `PureModel` elements.
//!
//! Core crates (`ast`, `parser`, `pure`, `compose`) carry no
//! Diagram-specific code — this crate is the only place the word
//! "Diagram" appears.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ast;
