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
//! Currently supported variants: [`ClassMappingBody::Pure`]
//! (model-to-model, `PureInstanceSetImplementation`),
//! [`ClassMappingBody::Enumeration`] (Stage 4),
//! [`ClassMappingBody::Operation`] (Stage 5, simple parameters
//! form — merge form deferred), and
//! [`ClassMappingBody::AggregationAware`] (Stage 6 — `Views`,
//! `~modelOperation`, `~mainMapping`; nested mapping bodies recurse
//! into `ClassMappingBody`). Remaining variants (`XStore`,
//! `Relation`) arrive in Stages 7–8; the enum is `#[non_exhaustive]`
//! so adding a variant is non-breaking. Hitting an unknown
//! `parserName` at parse time produces an `UnsupportedSubParser`
//! error pointing at the roadmap.

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
/// grammar. Remaining variants (`XStore`, `Relation`) arrive in
/// Stages 7–8. Marked `#[non_exhaustive]` so adding variants is
/// non-breaking.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ClassMappingBody {
    /// `parserName == "Pure"` — model-to-model
    /// (`PureInstanceSetImplementation` in the metamodel).
    /// Boxed because `PureClassMappingBody` is materially larger
    /// than the other variants (filter `Expression` + per-property
    /// `Lambda` bodies); keeping the enum compact avoids paying the
    /// largest-variant cost on every `ClassMapping`.
    Pure(Box<PureClassMappingBody>),
    /// `parserName == "EnumerationMapping"` — maps source values
    /// (strings / integers / external enum values) onto target enum
    /// values (`EnumerationMapping<T>` in the metamodel).
    Enumeration(EnumerationClassMappingBody),
    /// `parserName == "Operation"` — function-driven combinator that
    /// composes other set implementations (e.g. union, inheritance).
    /// Maps onto `OperationSetImplementation` in the metamodel.
    Operation(OperationClassMappingBody),
    /// `parserName == "AggregationAware"` — composes a main
    /// set-implementation with one or more pre-aggregated views,
    /// each guarded by a `~modelOperation` aggregate-specification
    /// expression. Maps onto `AggregationAwareSetImplementation`
    /// in the metamodel. Boxed because the body owns a recursive
    /// `ClassMappingBody` tree (`~mainMapping` + per-view
    /// `~aggregateMapping`) plus several nested expression lists,
    /// so the variant is by far the largest.
    AggregationAware(Box<AggregationAwareClassMappingBody>),
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

// ---------------------------------------------------------------------------
// EnumerationClassMappingBody — Stage 4
// ---------------------------------------------------------------------------

/// Body of an `EnumerationMapping` class mapping.
///
/// Shape:
/// ```text
/// {
///   TARGET_VAL_1 : 'sourceString',
///   TARGET_VAL_2 : 42,
///   TARGET_VAL_3 : pkg::OtherEnum.VAL,
///   TARGET_VAL_4 : ['multi', 'value']
/// }
/// ```
///
/// The enclosing [`ClassMapping::class`] field carries the FQN of
/// the target enumeration; this body holds only the per-target-value
/// source mappings.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumerationClassMappingBody {
    /// One entry per target enum value, in source order.
    pub value_mappings: Vec<EnumValueMapping>,
}

/// One `targetEnumValue : sourceValue(s)` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct EnumValueMapping {
    /// Target enum value name on the enumeration being mapped.
    pub enum_value_name: SmolStr,
    /// Source values that map to this target. Single-value entries
    /// produce a one-element vec; bracketed `[a, b, c]` entries
    /// produce a multi-element vec.
    pub source_values: Vec<EnumSourceValue>,
    /// Span of the entire `enumValueName : …` entry.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// OperationClassMappingBody — Stage 5
// ---------------------------------------------------------------------------

/// Body of an `Operation` class mapping.
///
/// Shape:
/// ```text
/// {
///   pkg::operations::union_OperationSetImplementation_1__SetImplementation_MANY_(rel1, rel2)
/// }
/// ```
///
/// The function path is typically a mangled FQN (Java's exact-FQN
/// dispatch convention) of a function whose signature is
/// `OperationSetImplementation[1] -> SetImplementation[*]`. Each
/// parameter is a sibling class-mapping ID (whether explicit `[id]`
/// or implicit class-name default).
///
/// This Stage-5 variant covers the simple `parameters` form; the
/// `mergeParameters` form (with a validation lambda) is reserved for
/// a follow-up sub-stage and would extend this struct (or split into
/// a sibling `Merge` variant) when added.
#[derive(Debug, Clone, PartialEq)]
pub struct OperationClassMappingBody {
    /// FQN of the operation function.
    pub operation: PackageableElementPtr,
    /// Set-implementation IDs combined by the operation, in source
    /// order. Empty allowed (matches Java grammar's `()` form).
    pub parameters: Vec<OperationParameter>,
}

/// One set-implementation-ID parameter inside an
/// [`OperationClassMappingBody`].
#[derive(Debug, Clone, PartialEq)]
pub struct OperationParameter {
    /// The referenced class-mapping ID.
    pub id: SmolStr,
    /// Span of the ID token, used to pin per-parameter validator
    /// diagnostics back to source.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// AggregationAwareClassMappingBody — Stage 6
// ---------------------------------------------------------------------------

/// Body of an `AggregationAware` class mapping.
///
/// Shape:
/// ```text
/// {
///   Views : [
///     (
///       ~modelOperation : {
///         ~canAggregate true,
///         ~groupByFunctions ( $this.salesDate ),
///         ~aggregateValues ( ( ~mapFn: $this.revenue, ~aggregateFn: $mapped->sum() ) )
///       },
///       ~aggregateMapping : Pure { ~src AggSrc … }
///     )
///   ],
///   ~mainMapping : Pure { ~src MainSrc … }
/// }
/// ```
///
/// The two nested mappings (`~mainMapping`, per-view
/// `~aggregateMapping`) recurse into [`ClassMappingBody`]. Both
/// inherit the outer class-mapping's target class — the nested
/// shapes themselves carry no class FQN, no `[id]`, no `extends`.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregationAwareClassMappingBody {
    /// Per-view aggregate specifications + their aggregate mapping.
    /// The Java grammar requires at least one entry; this AST does
    /// not currently enforce that — emptiness is validator-territory.
    pub views: Vec<AggregateView>,
    /// The fall-through main mapping used when no view's
    /// `canAggregate` predicate matches the query shape.
    pub main_mapping: NestedClassMapping,
}

/// One `(modelOperation, aggregateMapping)` pair inside an
/// [`AggregationAwareClassMappingBody`].
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateView {
    /// `~modelOperation : { … }` block describing when this view
    /// can be used and how its aggregate values map back.
    pub model_operation: AggregateSpecification,
    /// `~aggregateMapping : <parserName> { … }` — the nested
    /// set-implementation used when the model operation matches.
    pub aggregate_mapping: NestedClassMapping,
    /// Span of the entire `( ~modelOperation … , ~aggregateMapping … )`
    /// pair.
    pub source_info: SourceInfo,
}

/// A `~modelOperation : { ~canAggregate <bool>, ~groupByFunctions
/// ( … ), ~aggregateValues ( … ) }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateSpecification {
    /// `~canAggregate true|false` — declares whether this view
    /// covers exact aggregation (true) or a pre-aggregated
    /// approximation (false). Kept as a bool literal at parse time;
    /// validators downstream may reject other forms.
    pub can_aggregate: bool,
    /// `~groupByFunctions ( expr, expr, … )` — one expression per
    /// grouping key. Stored raw; validators wrap each as a lambda
    /// with `this` bound to the outer class type.
    pub group_by_functions: Vec<Expression>,
    /// `~aggregateValues ( ( ~mapFn: …, ~aggregateFn: … ), … )` —
    /// one map/aggregate pair per aggregated property.
    pub aggregate_values: Vec<AggregationFunctionSpec>,
    /// Span of the entire `~modelOperation : { … }` block.
    pub source_info: SourceInfo,
}

/// One `( ~mapFn: <expr>, ~aggregateFn: <expr> )` entry inside an
/// [`AggregateSpecification`]'s `~aggregateValues` list.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregationFunctionSpec {
    /// `~mapFn` — projects a row to the value being aggregated.
    /// Validator rule: must return a `DataType` (primitive or
    /// enumeration), per Java's `AggregationAwareValidator`.
    pub map_fn: Expression,
    /// `~aggregateFn` — folds many `$mapped` values into one.
    /// Same DataType-return validator rule applies.
    pub aggregate_fn: Expression,
    /// Span of the entire `( ~mapFn: … , ~aggregateFn: … )` pair.
    pub source_info: SourceInfo,
}

/// A nested class-mapping inside an
/// [`AggregationAwareClassMappingBody`] (`~mainMapping` or per-view
/// `~aggregateMapping`).
///
/// Carries only the sub-parser name + body — no class FQN, no
/// `[id]`, no `extends` — because the nested mapping inherits the
/// outer class-mapping's target class.
#[derive(Debug, Clone, PartialEq)]
pub struct NestedClassMapping {
    /// `parserName` selecting the body sub-grammar (`Pure`,
    /// `Operation`, …).
    pub parser_name: SmolStr,
    /// The nested body itself.
    pub body: ClassMappingBody,
    /// Span of the entire `~mainMapping : <parserName> { … }` (or
    /// `~aggregateMapping : <parserName> { … }`) clause.
    pub source_info: SourceInfo,
}

/// One source-value form inside an [`EnumValueMapping`].
#[derive(Debug, Clone, PartialEq)]
pub enum EnumSourceValue {
    /// `'literal'` — source value is a string literal.
    String {
        /// The string contents (already stripped of surrounding quotes).
        value: SmolStr,
        /// Span of the literal token.
        source_info: SourceInfo,
    },
    /// `42` — source value is an integer literal.
    Integer {
        /// The integer value.
        value: i64,
        /// Span of the literal token.
        source_info: SourceInfo,
    },
    /// `pkg::OtherEnum.VALUE` — source value is a reference to an
    /// enum value in some other enumeration (typically the source
    /// model's enum that maps onto this target).
    EnumRef {
        /// FQN of the source enumeration.
        enumeration: PackageableElementPtr,
        /// Name of the source enum value.
        value_name: SmolStr,
        /// Span of the entire `pkg::Enum.VAL` reference.
        source_info: SourceInfo,
    },
}
