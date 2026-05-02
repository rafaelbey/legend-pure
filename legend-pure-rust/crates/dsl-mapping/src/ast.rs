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

//! Mapping DSL AST.
//!
//! Top-level shape: a `###Mapping` section contains zero or more
//! [`MappingDef`]s. Each definition has a name, an optional list of
//! [`MappingInclude`]s, and a list of [`ClassMapping`]s. Each class
//! mapping selects a body sub-grammar by name (the `parserName` token
//! in the Java grammar) and contains a [`ClassMappingBody`] whose
//! variants correspond to the sub-parsers.
//!
//! Stage 2 ships only the [`ClassMappingBody::Pure`] variant
//! (model-to-model, `PureInstanceSetImplementation`). Other variants
//! (Enumeration, Operation, AggregationAware, XStore, Relation) are
//! added in Stages 4+; the enum is `#[non_exhaustive]` so adding a
//! variant is non-breaking. Hitting an unknown `parserName` at parse
//! time produces an `UnsupportedSubParser` error pointing at the
//! roadmap.

use std::any::Any;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, SpannedString};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::element::{Annotated, PackageableElement};
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::{Identifier, Package};
use smol_str::SmolStr;

/// Section kind string this DSL claims (`###Mapping`).
pub const SECTION_KIND: &str = "Mapping";

// ---------------------------------------------------------------------------
// MappingDef
// ---------------------------------------------------------------------------

/// A `Mapping` definition: `Mapping pkg::M ( includes* classMappings* )`.
#[derive(Debug, Clone, PartialEq)]
pub struct MappingDef {
    /// Package qualifier on the mapping name.
    pub package: Option<Package>,
    /// The mapping's simple name plus its name token's span.
    pub name: SpannedString,
    /// Mapping-include declarations (`include other::Mapping`), in
    /// source order. Empty when none are written.
    pub includes: Vec<MappingInclude>,
    /// Class-mapping declarations, in source order.
    pub class_mappings: Vec<ClassMapping>,
    /// Span of the entire `Mapping … ( … )` declaration.
    pub source_info: SourceInfo,
}

impl Spanned for MappingDef {
    fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }
}

impl Annotated for MappingDef {
    fn stereotypes(&self) -> &[legend_pure_parser_ast::annotation::StereotypePtr] {
        &[]
    }
    fn tagged_values(&self) -> &[legend_pure_parser_ast::annotation::TaggedValue] {
        &[]
    }
}

impl PackageableElement for MappingDef {
    fn package(&self) -> Option<&Package> {
        self.package.as_ref()
    }
    fn name(&self) -> &Identifier {
        &self.name.value
    }
}

impl DSLElement for MappingDef {
    fn kind(&self) -> &str {
        SECTION_KIND
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn clone_box(&self) -> Box<dyn DSLElement> {
        Box::new(self.clone())
    }
    fn eq_content(&self, other: &dyn DSLElement) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self == o)
    }
}

// ---------------------------------------------------------------------------
// MappingInclude
// ---------------------------------------------------------------------------

/// `include pkg::OtherMapping [storeSubstitutions?]`.
///
/// The optional bracket clause is reserved for store-substitution
/// directives (`include m [src::Store -> dst::Store]`), used by
/// store-aware DSLs (Relational). Stage 2 parses but does not validate
/// these substitutions.
#[derive(Debug, Clone, PartialEq)]
pub struct MappingInclude {
    /// FQN of the included mapping.
    pub included: PackageableElementPtr,
    /// Store substitutions (`src -> dst`), if any.
    pub store_substitutions: Vec<StoreSubstitution>,
    /// Span of the entire include declaration.
    pub source_info: SourceInfo,
}

/// One `srcStore -> dstStore` pair inside a [`MappingInclude`]'s
/// optional bracket clause.
#[derive(Debug, Clone, PartialEq)]
pub struct StoreSubstitution {
    /// FQN of the source store.
    pub source: PackageableElementPtr,
    /// FQN of the destination store.
    pub target: PackageableElementPtr,
    /// Span of the `src -> dst` pair.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// ClassMapping
// ---------------------------------------------------------------------------

/// One class mapping inside a [`MappingDef`].
///
/// Syntax: `*?qualifiedName [id]? (extends [super])? : parserName mappingName? { body }`.
/// The leading `*` marks the mapping as the root for its class.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassMapping {
    /// `*` prefix indicating this mapping is the root for its class.
    pub is_root: bool,
    /// FQN of the class being mapped.
    pub class: PackageableElementPtr,
    /// Optional class-mapping ID `[id]`. Defaults to the class FQN when absent.
    pub id: Option<SmolStr>,
    /// Optional super-mapping ID via `extends [superId]`.
    pub extends: Option<SmolStr>,
    /// Optional mapping-instance display name following `parserName`.
    pub mapping_name: Option<SmolStr>,
    /// Body, parsed by the sub-parser selected via `parserName`.
    pub body: ClassMappingBody,
    /// Span of the entire class-mapping declaration.
    pub source_info: SourceInfo,
}

/// Class-mapping body sub-grammars.
///
/// Each variant corresponds to a `parserName` keyword in the Java
/// grammar. Stage 2 ships only [`ClassMappingBody::Pure`]; Stages 4+
/// add the rest. Marked `#[non_exhaustive]` so adding variants is
/// non-breaking.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ClassMappingBody {
    /// `parserName == "Pure"` — model-to-model
    /// (`PureInstanceSetImplementation` in the metamodel).
    Pure(PureClassMappingBody),
}

// ---------------------------------------------------------------------------
// PureClassMappingBody — the only Stage-2 body variant
// ---------------------------------------------------------------------------

/// Body of a `Pure` (model-to-model) class mapping.
///
/// Shape:
/// ```text
/// {
///   ~src pkg::SrcClass        // optional source-class clause
///   ~filter $src.foo > 0      // optional filter expression
///   prop1 : $src.value1,
///   prop2 : $src.value2
/// }
/// ```
///
/// The filter and transform values are stored as bare [`Expression`]s
/// — that's the surface syntax (`~filter $src.x == 'ok'` is an
/// expression, not a `| body` lambda). The Stage-3 processor wraps
/// each into a `LambdaFunction` with `src` bound to the source class
/// before lowering, matching the Java `PureInstanceSetImplementationProcessor`.
#[derive(Debug, Clone, PartialEq)]
pub struct PureClassMappingBody {
    /// Optional `~src` clause naming the source class.
    pub src_class: Option<PackageableElementPtr>,
    /// Optional `~filter` clause holding the filter expression. Stage
    /// 2 stores it raw; Stage 3 wraps it as a `Lambda<{src}>` and
    /// validates that it returns `Boolean[1]`.
    pub filter: Option<Expression>,
    /// Per-property transform mappings.
    pub property_mappings: Vec<PurePropertyMapping>,
}

/// One `propertyName : transformExpression` entry inside a
/// [`PureClassMappingBody`].
#[derive(Debug, Clone, PartialEq)]
pub struct PurePropertyMapping {
    /// Target property name on the class being mapped.
    pub property_name: SmolStr,
    /// Transform expression producing the property's value. Stage 2
    /// stores it raw; Stage 3 wraps it as a `Lambda<{src}>` and
    /// validates the return type against the property's declared type.
    pub transform: Expression,
    /// Optional value transformer FQN (`+pkg::EnumMap`). Reserved for
    /// Stage 4 (enumeration mappings); Stage 2 always parses as `None`.
    pub transformer: Option<PackageableElementPtr>,
    /// `+` modifier marking the transform as exploding into multiple
    /// values. Reserved; Stage 2 always parses as `false`.
    pub explode: bool,
    /// Span of the entire `propertyName : transform` entry.
    pub source_info: SourceInfo,
}
