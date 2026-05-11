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
//! form — merge form deferred),
//! [`ClassMappingBody::AggregationAware`] (Stage 6 — `Views`,
//! `~modelOperation`, `~mainMapping`; nested mapping bodies recurse
//! into `ClassMappingBody`), and [`ClassMappingBody::XStore`]
//! (Stage 7 — per-association-property cross-store join
//! expressions binding `$this`/`$that`). Remaining variant
//! (`Relation`) arrives in Stage 8; the enum is
//! `#[non_exhaustive]` so adding a variant is non-breaking.
//! Hitting an unknown `parserName` at parse time produces an
//! `UnsupportedSubParser` error pointing at the roadmap.

use std::any::Any;

use legend_pure_parser_ast::SourceInfo;
use legend_pure_parser_ast::annotation::{PackageableElementPtr, SpannedString};
use legend_pure_parser_ast::dsl::DSLElement;
use legend_pure_parser_ast::element::{Annotated, PackageableElement};
use legend_pure_parser_ast::expression::Expression;
use legend_pure_parser_ast::source_info::Spanned;
use legend_pure_parser_ast::type_ref::{Identifier, Multiplicity, Package, TypeReference};
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
/// grammar. Marked `#[non_exhaustive]` so adding variants is
/// non-breaking. The five built-in variants cover the M2 mapping
/// surface; foreign DSLs (Relational, future stores) plug in via the
/// [`Foreign`](Self::Foreign) variant by implementing
/// [`ForeignClassMappingBody`] and registering a
/// [`ClassMappingBodyParser`](crate::parser::ClassMappingBodyParser)
/// — see `tests/foreign_body_parser.rs` for the registration shape.
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
    /// `parserName == "XStore"` — bridges two pure-instance set
    /// implementations across stores via per-association-property
    /// cross expressions binding `$this` (source set-impl class) and
    /// `$that` (target set-impl class). Maps onto
    /// `XStoreAssociationImplementation` in the metamodel — note
    /// the outer class-mapping FQN is reinterpreted as an
    /// `Association` FQN, not a `Class` FQN, by the validator.
    XStore(XStoreClassMappingBody),
    /// `parserName == "Relation"` — ties a Pure function returning
    /// `Relation<…>[1]` to per-property column mappings. Each
    /// property maps to a named column on the function's relation
    /// output, optionally through a `Binding` transformer or as a
    /// `+ local : Type[mult]` declaration. Maps onto
    /// `RelationFunctionInstanceSetImplementation` in the metamodel.
    /// Boxed because the body carries an arbitrarily long
    /// `property_mappings` vector plus the function signature's
    /// parameter type vector — bigger than the other inline variants.
    /// Java parity: protocol class
    /// `RelationFunctionClassMapping` at
    /// `legend-engine-protocol-pure/.../mapping/relationFunction/`;
    /// grammar at
    /// `legend-engine-language-pure-grammar/.../mapping/relationFunctionMapping/RelationFunctionMappingParserGrammar.g4`.
    RelationFunction(Box<RelationFunctionClassMappingBody>),
    /// Body produced by a foreign [`ClassMappingBodyParser`] —
    /// e.g. Relational's `: Relational { ~mainTable [db]schema.tbl … }`
    /// shape — registered with [`MappingSectionParser`] via
    /// [`with_body_parsers`](crate::parser::MappingSectionParser::with_body_parsers).
    /// dsl-mapping never inspects the inner content; the foreign DSL's
    /// own [`CompilerExtension`](legend_pure_parser_pure::extension::CompilerExtension)
    /// downcasts it via
    /// [`ForeignClassMappingBody::as_any`] during validation.
    Foreign(Box<dyn ForeignClassMappingBody>),
}

// ---------------------------------------------------------------------------
// ForeignClassMappingBody — plug-in trait for store-DSL bodies
// ---------------------------------------------------------------------------

/// Plug-in trait for class-mapping body sub-grammars contributed by
/// store DSLs (e.g. Relational's `: Relational { … }`,
/// future BigQuery / Delta / etc.).
///
/// Mirrors the existing [`DSLElement`] trait shape used for
/// section-level plug-ins. Built-in body sub-grammars (Pure,
/// Enumeration, Operation, AggregationAware, XStore) live as enum
/// variants on [`ClassMappingBody`]; foreign bodies travel through
/// the [`ClassMappingBody::Foreign`] variant.
///
/// Implementations should be lightweight (parsed AST only — no
/// validators). Validation belongs in the foreign DSL's
/// [`CompilerExtension`](legend_pure_parser_pure::extension::CompilerExtension)
/// implementation, which downcasts the `Foreign` variant back to its
/// concrete type via [`as_any`](Self::as_any).
pub trait ForeignClassMappingBody: std::fmt::Debug + Send + Sync {
    /// The `parserName` token that produced this body (e.g. `"Relational"`).
    /// Must match the [`kind()`](crate::parser::ClassMappingBodyParser::kind)
    /// of the registered body parser.
    fn kind(&self) -> &str;

    /// Downcast support. Foreign DSL validators use this to recover
    /// the concrete body type from a [`ClassMappingBody::Foreign`].
    fn as_any(&self) -> &dyn std::any::Any;

    /// Clone this body into a fresh `Box<dyn …>`. Required because
    /// [`ClassMappingBody`] derives `Clone` and trait objects don't
    /// implement `Clone` directly.
    fn clone_box(&self) -> Box<dyn ForeignClassMappingBody>;

    /// Structural equality against another body. Required because
    /// [`ClassMappingBody`] derives `PartialEq` and trait objects
    /// don't implement it directly. Implementations should downcast
    /// `other` to their own concrete type and compare field-wise.
    fn eq_content(&self, other: &dyn ForeignClassMappingBody) -> bool;

    /// Append this body's grammar text — including the surrounding
    /// `{ … }` braces — to `out`. Used by the composer's
    /// round-trip path. The composer prints `: <kind> ` immediately
    /// before this call, so the body itself owns only the body block.
    fn compose(&self, out: &mut String);

    /// Stores referenced by this body's grammar. Default returns
    /// empty. Implementations override when their grammar references
    /// stores — relational class-mapping bodies reference Databases
    /// via `~mainTable [db]`, scope's `[db]`, per-line `[db]`
    /// qualifiers, and join-sequence `[db]` qualifiers.
    ///
    /// Used by Phase E3's `validate_store_substitution_existence` to
    /// answer "what stores does mapping IM accept?". Java parity:
    /// `InstanceSetImplementation._stores()` on the relational
    /// metamodel.
    fn referenced_stores(&self) -> Vec<PackageableElementPtr> {
        Vec::new()
    }
}

impl Clone for Box<dyn ForeignClassMappingBody> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl PartialEq for Box<dyn ForeignClassMappingBody> {
    fn eq(&self, other: &Self) -> bool {
        self.eq_content(other.as_ref())
    }
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
    /// Optional inline `EnumerationMapping <name>` transformer prefix
    /// — e.g. `state : EnumerationMapping StateMapping : $src.state`.
    /// The name is the *local* `mapping_name` (not an FQN) of an
    /// EnumerationMapping declared elsewhere in this Mapping (or a
    /// transitively included one). Mirrors Java's M3 grammar
    /// `(ENUMERATION_MAPPING identifier COLON)?` between the
    /// property colon and the transform expression.
    pub transformer: Option<SmolStr>,
    /// `+` modifier marking the transform as exploding into multiple
    /// values. Reserved; Stage 2 always parses as `false`.
    pub explode: bool,
    /// Optional inline local-property declaration. When present, the
    /// `+` prefix form was used: `+name : Type[mult] : transform`,
    /// declaring a *local* property (only visible from this mapping)
    /// with the given type/multiplicity along with the transform.
    /// Mirrors the M3 grammar's
    /// `(PLUS qualifiedName COLON type multiplicity)` alternative
    /// for `mappingLine`.
    pub local_property: Option<LocalPropertyDecl>,
    /// Span of the entire `propertyName : transform` entry.
    pub source_info: SourceInfo,
}

/// Declared type + multiplicity for an inline local-property
/// declaration (`+name : Type[mult]`). Java's TestModelMapping
/// covers the type-validity rule
/// (`testLocalPropertyWithInvalidType`) and the type-mismatch rule
/// (`testLocalPropertyTypeError`).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalPropertyDecl {
    /// Declared type reference (e.g. `String`, `pkg::SomeClass`).
    pub type_ref: TypeReference,
    /// Declared multiplicity (e.g. `[1]`, `[*]`, `[0..1]`).
    pub multiplicity: Multiplicity,
    /// Span covering the whole `Type[mult]` declaration.
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
/// Two Java-grammar forms — both produce the same struct, with
/// `validation_function` discriminating:
///
/// 1. **Simple `parameters` form** (most operations):
///    ```text
///    {
///      pkg::operations::union_OperationSetImplementation_1__SetImplementation_MANY_(rel1, rel2)
///    }
///    ```
///    `validation_function: None`.
///
/// 2. **`mergeParameters` form** (`MergeOperationSetImplementation`):
///    ```text
///    {
///      pkg::operations::merge_OperationSetImplementation_1__SetImplementation_MANY_(
///        [rel1, rel2],
///        {row | $row->validate(...)}
///      )
///    }
///    ```
///    Parameters arrive bracketed, followed by a validation lambda
///    that's invoked on the merged rows. `validation_function:
///    Some(<lambda>)`. Java parity:
///    `OperationClassMappingParseTreeWalker.visitMergeOperationClassMapping`
///    + `mergeParameters` rule at
///    `OperationClassMappingParserGrammar.g4:31`.
///
/// The function path is typically a mangled FQN (Java's exact-FQN
/// dispatch convention) of a function whose signature is
/// `OperationSetImplementation[1] -> SetImplementation[*]`. Each
/// parameter is a sibling class-mapping ID (whether explicit `[id]`
/// or implicit class-name default).
#[derive(Debug, Clone, PartialEq)]
pub struct OperationClassMappingBody {
    /// FQN of the operation function.
    pub operation: PackageableElementPtr,
    /// Set-implementation IDs combined by the operation, in source
    /// order. Empty allowed (matches Java grammar's `()` form).
    pub parameters: Vec<OperationParameter>,
    /// Validation lambda paired with the merge form. `None` for the
    /// simple form. Round-trips verbatim through the composer.
    pub validation_function: Option<Expression>,
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
// RelationFunctionClassMappingBody — Stage 8
// ---------------------------------------------------------------------------

/// Body of a `: Relation { … }` class mapping.
///
/// Shape (Java grammar
/// `RelationFunctionMappingParserGrammar.g4`):
///
/// ```text
/// {
///   ~func pkg::myRelationFn():Relation<Any>[1]
///   firstName : name_col,
///   lastName : surname_col,
///   + computed : String[1] : composite_col
/// }
/// ```
///
/// Or with a binding transformer:
///
/// ```text
/// {
///   ~func pkg::myRelFn():Relation<Any>[1]
///   data : Binding pkg::TextBinding : text_col
/// }
/// ```
///
/// Each property mapping pairs a property name (regular or
/// `+`-prefixed local) with a column name on the function's relation
/// output, optionally going through a `Binding` transformer.
#[derive(Debug, Clone, PartialEq)]
pub struct RelationFunctionClassMappingBody {
    /// Function FQN. Java captures the full
    /// `qualifiedName(P1,…):Return[m]` text as a single string; we
    /// store the qualified name in this `PackageableElementPtr` and
    /// the signature suffix in [`Self::function_signature_suffix`].
    pub relation_function: PackageableElementPtr,
    /// Original textual form of the `():Return[m]` suffix following
    /// the function's qualified name (e.g.
    /// `"():meta::pure::metamodel::relation::Relation<Any>[1]"`).
    /// Preserved verbatim for round-trip composer fidelity; the
    /// validator parses it on demand. `None` when the grammar omits
    /// the suffix (mangled-FQN form).
    pub function_signature_suffix: Option<SmolStr>,
    /// Per-property column mappings, in source order.
    pub property_mappings: Vec<RelationFunctionPropertyMapping>,
    /// Span of the entire body.
    pub source_info: SourceInfo,
}

/// One property → column mapping inside a
/// [`RelationFunctionClassMappingBody`].
///
/// Three shapes (Java grammar
/// `singlePropertyMapping`):
///
/// 1. **Non-local**: `propertyName : column_name`
/// 2. **With binding transformer**:
///    `propertyName : Binding pkg::SomeBinding : column_name`
/// 3. **Local declaration**:
///    `+ propertyName : Type[mult] : column_name`
///    Optionally combined with a binding transformer:
///    `+ propertyName : Type[mult] : Binding pkg::SomeBinding : column_name`
#[derive(Debug, Clone, PartialEq)]
pub struct RelationFunctionPropertyMapping {
    /// Target property name on the outer class.
    pub property_name: SmolStr,
    /// `+ name : Type[mult]` declaration; `None` for non-local refs.
    pub local_mapping_property: Option<LocalPropertyDecl>,
    /// Optional `Binding pkg::SomeBinding :` transformer.
    pub binding_transformer: Option<BindingTransformer>,
    /// Column name on the function's relation output.
    pub column: SmolStr,
    /// Span of the entire entry.
    pub source_info: SourceInfo,
}

/// A `Binding pkg::SomeBinding :` transformer prefix on a relation
/// property mapping. Mirrors Java's `BindingTransformer` protocol
/// class (lives in `legend-engine-protocol-pure-external-format`
/// today; we model it inline since the DSL is the only consumer).
#[derive(Debug, Clone, PartialEq)]
pub struct BindingTransformer {
    /// FQN of the binding referenced by the transformer.
    pub binding: PackageableElementPtr,
    /// Span covering the `Binding pkg::SomeBinding` prefix.
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

// ---------------------------------------------------------------------------
// XStoreClassMappingBody — Stage 7
// ---------------------------------------------------------------------------

/// Body of an `XStore` association mapping.
///
/// Shape:
/// ```text
/// {
///   firm[employee_set, firm_set]    : $this.firmId == $that.id,
///   employees[firm_set, employee_set] : $this.id == $that.firmId
/// }
/// ```
///
/// The outer class-mapping FQN (`Firm_Person` in tests) is
/// reinterpreted as an [`Association`](legend_pure_parser_pure::nodes::association::Association)
/// FQN by the validator. Each entry binds one association property
/// to a cross-store join expression where `$this` resolves to the
/// source set-implementation's class and `$that` to the target's.
#[derive(Debug, Clone, PartialEq)]
pub struct XStoreClassMappingBody {
    /// Per-association-property cross mappings, in source order.
    pub property_mappings: Vec<XStorePropertyMapping>,
}

/// One `propName[srcId, tgtId]? : crossExpression` entry inside an
/// [`XStoreClassMappingBody`].
#[derive(Debug, Clone, PartialEq)]
pub struct XStorePropertyMapping {
    /// Target association property name.
    pub property_name: SmolStr,
    /// Optional source set-implementation ID
    /// (the `[srcId, ...]` form). When absent the cross-expression
    /// has no `$this` binding — Java treats this as an error during
    /// processing, but parsing accepts the shape.
    pub source_set_impl_id: Option<SmolStr>,
    /// Optional target set-implementation ID
    /// (the `[..., tgtId]` form). Same parse-vs-validate split as
    /// the source ID.
    pub target_set_impl_id: Option<SmolStr>,
    /// `$this`/`$that` cross-store join expression. Stored raw;
    /// downstream lowering wraps it as a `Lambda<{this, that}>`.
    pub cross_expression: Expression,
    /// Span of the entire entry.
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
