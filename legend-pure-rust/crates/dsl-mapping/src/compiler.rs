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

//! Stage-2 [`CompilerExtension`] for the Mapping DSL.
//!
//! `declare()` walks each parsed source file's `###Mapping` sections,
//! downcasts every `Element::DSLElement` to [`MappingDef`], and
//! registers it in extension-private state keyed by FQN. Duplicates
//! produce a `DuplicateElement` diagnostic.
//!
//! Other phases (`define_signatures` / `define_bodies` / `validate`)
//! inherit the trait's no-op defaults — Stage 3 fills them in with
//! the `PureInstanceSetImplementationProcessor` /
//! `PureInstanceSetImplementationValidator` / `MappingValidator`
//! ports. The pattern is the same as the precedents in
//! `crates/dsl-diagram/src/compiler.rs:87-101`.

use std::cell::RefCell;
use std::collections::HashMap;

use legend_pure_parser_ast::element::Element as AstElement;
use legend_pure_parser_ast::element::PackageableElement;
use legend_pure_parser_pure::error::{CompilationError, CompilationErrorKind};
use legend_pure_parser_pure::extension::{CompilerExtension, DeclareCtx};
use smol_str::SmolStr;

use crate::ast::MappingDef;

/// Compiler extension for the `###Mapping` DSL.
///
/// Construct one per
/// [`compile_with_extensions`](legend_pure_parser_pure::pipeline::compile_with_extensions)
/// invocation; do not share across compilations because the
/// extension's internal map is reset per call.
#[derive(Default)]
pub struct MappingExtension {
    /// Mappings collected during `declare`, keyed by FQN
    /// (`"pkg::sub::Name"`). Per-extension state — not stored in
    /// `PureModel`. Use [`Self::mappings`] to inspect after compile.
    mappings: RefCell<HashMap<SmolStr, RegisteredMapping>>,
}

/// One registered mapping alongside its bookkeeping FQN.
#[derive(Debug, Clone)]
pub struct RegisteredMapping {
    /// The original AST node.
    pub def: MappingDef,
    /// FQN this mapping is registered under.
    pub fqn: SmolStr,
}

impl MappingExtension {
    /// Construct an empty extension.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all registered mappings keyed by FQN. Useful for
    /// tests, codegen, and downstream stages (Stage 3 reads this map
    /// in `define_bodies` to drive the `PureInstanceSetImplementation`
    /// processor).
    #[must_use]
    pub fn mappings(&self) -> HashMap<SmolStr, RegisteredMapping> {
        self.mappings.borrow().clone()
    }
}

impl CompilerExtension for MappingExtension {
    fn name(&self) -> &'static str {
        "dsl-mapping"
    }

    fn declare(&self, ctx: &mut DeclareCtx<'_>) {
        let mut registry = self.mappings.borrow_mut();
        for source_file in ctx.source_files {
            for section in &source_file.sections {
                if section.kind.as_str() != crate::ast::SECTION_KIND {
                    continue;
                }
                for elem in &section.elements {
                    let AstElement::DSLElement(boxed) = elem else {
                        continue;
                    };
                    let Some(m) = boxed.as_any().downcast_ref::<MappingDef>() else {
                        continue;
                    };

                    let fqn = build_fqn(m);
                    if let Some(prev) = registry.insert(
                        fqn.clone(),
                        RegisteredMapping {
                            def: m.clone(),
                            fqn: fqn.clone(),
                        },
                    ) {
                        ctx.errors.push(CompilationError {
                            message: format!("Duplicate mapping '{fqn}'"),
                            source_info: m.source_info.clone(),
                            kind: CompilationErrorKind::DuplicateElement { name: fqn },
                        });
                        // Restore prior registration so subsequent
                        // passes still see the earlier definition.
                        registry.insert(prev.fqn.clone(), prev);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// FQN
// ---------------------------------------------------------------------------

fn build_fqn(m: &MappingDef) -> SmolStr {
    if let Some(pkg) = m.package() {
        SmolStr::new(format!("{pkg}::{}", m.name.value))
    } else {
        m.name.value.clone()
    }
}
