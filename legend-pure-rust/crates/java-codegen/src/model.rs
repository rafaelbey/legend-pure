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

//! Public IR — `JavaFile`, `Options`, `FqnInput`, `CodegenError` —
//! plus the requested-function resolver.

use std::path::PathBuf;

use legend_pure_parser_pure::ids::ElementId;
use legend_pure_parser_pure::model::{Element, PureModel};
use legend_pure_parser_pure::types::TypeExpr;
use smol_str::SmolStr;
use thiserror::Error;

/// One Pure FQN supplied by the caller. Currently only the **mangled** form
/// is accepted (e.g. `meta::pure::functions::math::plus_Integer_MANY__Integer_1_`)
/// — the same string the user passes to `PureRustEvaluator.evaluate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FqnInput {
    /// The raw FQN string (mangled form expected).
    pub raw: String,
}

impl FqnInput {
    /// Convenience constructor.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self { raw: raw.into() }
    }
}

/// Code-generation options.
#[derive(Debug, Clone)]
pub struct Options {
    /// Java root package — every emitted class lives under this prefix
    /// (e.g. `"com.example.gen"`).
    pub java_root_package: String,
    /// Simple class name for the static-functions facade. Defaults to
    /// `"PureFunctions"` when `None`.
    pub functions_class_name: Option<String>,
}

impl Options {
    /// Constructs options with the default facade class name (`PureFunctions`).
    #[must_use]
    pub fn new(java_root_package: impl Into<String>) -> Self {
        Self {
            java_root_package: java_root_package.into(),
            functions_class_name: None,
        }
    }

    /// Returns the chosen facade class name (or the default).
    #[must_use]
    pub fn functions_class(&self) -> &str {
        self.functions_class_name
            .as_deref()
            .unwrap_or("PureFunctions")
    }
}

/// One emitted Java source file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JavaFile {
    /// Path relative to the user-chosen output root, including the package
    /// directory layout (e.g. `com/example/gen/meta/pure/Person.java`).
    pub relative_path: PathBuf,
    /// File contents.
    pub contents: String,
}

/// Errors produced by codegen.
#[derive(Debug, Error)]
pub enum CodegenError {
    /// A requested FQN does not resolve to a `Function` element.
    #[error("function `{fqn}` could not be resolved in the model")]
    UnresolvedFunction {
        /// The offending FQN.
        fqn: String,
    },
    /// A requested FQN resolved to a non-function element.
    #[error("`{fqn}` resolved to a non-function element ({kind})")]
    NotAFunction {
        /// The offending FQN.
        fqn: String,
        /// The element kind that was found.
        kind: &'static str,
    },
    /// A requested function has a function-typed parameter or return —
    /// deferred to v2.
    #[error("function `{fqn}` has a function-typed {position} which is not supported in v1")]
    FunctionTypedParameter {
        /// The offending FQN.
        fqn: String,
        /// `"parameter '<name>'"` or `"return type"`.
        position: String,
    },
    /// A requested function has a relation-typed parameter or return —
    /// not supported in v1.
    #[error("function `{fqn}` has a relation-typed {position} which is not supported in v1")]
    RelationTyped {
        /// The offending FQN.
        fqn: String,
        /// `"parameter '<name>'"` or `"return type"`.
        position: String,
    },
    /// A requested function has a generic / unresolved type that cannot be
    /// turned into a concrete Java type.
    #[error(
        "function `{fqn}` has a generic-typed {position} (`{ty}`) which is not supported in v1"
    )]
    GenericTyped {
        /// The offending FQN.
        fqn: String,
        /// `"parameter '<name>'"` or `"return type"`.
        position: String,
        /// Rendered type (typically the type variable name).
        ty: String,
    },
    /// A `--classes <FQN>` argument did not resolve.
    #[error("class `{fqn}` could not be resolved in the model")]
    UnresolvedClass {
        /// The offending FQN.
        fqn: String,
    },
    /// A `--classes <FQN>` argument resolved to something that isn't a Class.
    #[error("`{fqn}` resolved to a non-class element ({kind})")]
    NotAClass {
        /// The offending FQN.
        fqn: String,
        /// The element kind that was found.
        kind: &'static str,
    },
    /// A `--associations <FQN>` argument did not resolve.
    #[error("association `{fqn}` could not be resolved in the model")]
    UnresolvedAssociation {
        /// The offending FQN.
        fqn: String,
    },
    /// A `--associations <FQN>` argument resolved to something that isn't an
    /// Association.
    #[error("`{fqn}` resolved to a non-association element ({kind})")]
    NotAnAssociation {
        /// The offending FQN.
        fqn: String,
        /// The element kind that was found.
        kind: &'static str,
    },
}

/// Canonical bootstrap-element IDs the codegen needs to recognise by
/// identity. The M3 bootstrap classes carry `parent_package = root` in
/// the model, so `pure_fqn_segments` returns just the leaf name for
/// them (`["Any"]`, `["Nil"]`) instead of the full
/// `meta::pure::metamodel::type::*` path; an FQN-based string match
/// would always miss. Resolving via `resolve_fqn_str` walks the package
/// tree top-down and finds the canonical element regardless.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Bootstrap {
    pub(crate) any_id: Option<ElementId>,
    pub(crate) nil_id: Option<ElementId>,
}

impl Bootstrap {
    pub(crate) fn resolve(model: &PureModel) -> Self {
        Self {
            any_id: model.resolve_fqn_str("meta::pure::metamodel::type::Any"),
            nil_id: model.resolve_fqn_str("meta::pure::metamodel::type::Nil"),
        }
    }

    pub(crate) fn is_any(self, id: ElementId) -> bool {
        self.any_id == Some(id)
    }

    pub(crate) fn is_nil(self, id: ElementId) -> bool {
        self.nil_id == Some(id)
    }
}

/// One resolved requested function — carries the looked-up `ElementId`
/// and the mangled FQN we'll pass to `PureRustEvaluator.evaluate`.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedFn {
    pub(crate) element_id: ElementId,
    pub(crate) fqn: SmolStr,
}

pub(crate) fn resolve_requested(
    model: &PureModel,
    fns: &[FqnInput],
) -> Result<Vec<ResolvedFn>, CodegenError> {
    let mut out = Vec::with_capacity(fns.len());
    for fn_input in fns {
        let id = model.resolve_fqn_str(&fn_input.raw).ok_or_else(|| {
            CodegenError::UnresolvedFunction {
                fqn: fn_input.raw.clone(),
            }
        })?;
        match model.get_element(id) {
            Element::Function(_) => {
                out.push(ResolvedFn {
                    element_id: id,
                    fqn: SmolStr::new(&fn_input.raw),
                });
            }
            other => {
                return Err(CodegenError::NotAFunction {
                    fqn: fn_input.raw.clone(),
                    kind: element_kind(other),
                });
            }
        }
    }
    Ok(out)
}

/// Resolve `--classes` and `--associations` FQNs into a deduplicated
/// list of `Class` element IDs to seed the reachability walk with.
///
/// Each `--classes <FQN>` contributes its own ID. Each
/// `--associations <FQN>` unfolds into the participating-class IDs
/// (typically two — one per end). Order follows the input order; duplicate
/// IDs are dropped on first reuse.
pub(crate) fn resolve_extra_seeds(
    model: &PureModel,
    classes: &[FqnInput],
    associations: &[FqnInput],
) -> Result<Vec<ElementId>, CodegenError> {
    let mut seen = std::collections::HashSet::<ElementId>::new();
    let mut out = Vec::with_capacity(classes.len() + associations.len() * 2);

    for cls in classes {
        let id = model
            .resolve_fqn_str(&cls.raw)
            .ok_or_else(|| CodegenError::UnresolvedClass {
                fqn: cls.raw.clone(),
            })?;
        match model.get_element(id) {
            Element::Class(_) => {
                if seen.insert(id) {
                    out.push(id);
                }
            }
            other => {
                return Err(CodegenError::NotAClass {
                    fqn: cls.raw.clone(),
                    kind: element_kind(other),
                });
            }
        }
    }

    for assoc in associations {
        let id = model.resolve_fqn_str(&assoc.raw).ok_or_else(|| {
            CodegenError::UnresolvedAssociation {
                fqn: assoc.raw.clone(),
            }
        })?;
        let association = match model.get_element(id) {
            Element::Association(a) => a,
            other => {
                return Err(CodegenError::NotAnAssociation {
                    fqn: assoc.raw.clone(),
                    kind: element_kind(other),
                });
            }
        };
        for prop in &association.properties {
            if let TypeExpr::Named { element, .. } = &prop.type_expr
                && matches!(model.get_element(*element), Element::Class(_))
                && seen.insert(*element)
            {
                out.push(*element);
            }
        }
    }

    Ok(out)
}

fn element_kind(e: &Element) -> &'static str {
    match e {
        Element::Class(_) => "Class",
        Element::Enumeration(_) => "Enumeration",
        Element::Function(_) => "Function",
        Element::Profile(_) => "Profile",
        Element::Association(_) => "Association",
        Element::Measure(_) => "Measure",
        Element::PrimitiveType(_) => "PrimitiveType",
        Element::Unit(_) => "Unit",
        Element::PackageableMultiplicity(_) => "Multiplicity",
        Element::Package(_) => "Package",
    }
}
