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

//! Top-level packageable elements: Class, Enum, Function, Profile, Association, Measure.
//!
//! Every element is [`Spanned`], [`Annotated`], and [`PackageableElement`].
//! The [`Element`] enum wraps all element types for uniform handling.

use crate::annotation::{Parameter, SpannedString, StereotypePtr, TaggedValue};
use crate::expression::Expression;
use crate::source_info::{SourceInfo, Spanned};
use crate::type_ref::{Identifier, Multiplicity, Package, TypeReference};

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// A top-level element that lives in a package and has a name.
///
/// Named `PackageableElement` to match the Java convention, easing transition
/// for developers moving between the Java and Rust implementations.
pub trait PackageableElement: Spanned + Annotated {
    /// Returns the package this element belongs to.
    fn package(&self) -> Option<&Package>;
    /// Returns the name of this element.
    fn name(&self) -> &Identifier;
}

/// An element that can carry stereotypes and tagged values.
pub trait Annotated {
    /// Returns the stereotypes applied to this element.
    fn stereotypes(&self) -> &[StereotypePtr];
    /// Returns the tagged values applied to this element.
    fn tagged_values(&self) -> &[TaggedValue];
}

// ---------------------------------------------------------------------------
// Element enum
// ---------------------------------------------------------------------------

/// A top-level packageable element in the Pure grammar.
///
/// This is the root type the parser produces — a source file parses into
/// a `Vec<Element>`.
#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    /// A class definition.
    Class(ClassDef),
    /// An enumeration definition.
    Enumeration(EnumDef),
    /// A function definition.
    Function(FunctionDef),
    /// A profile definition.
    Profile(ProfileDef),
    /// An association definition.
    Association(AssociationDef),
    /// A measure definition (with units).
    Measure(MeasureDef),
}

impl Spanned for Element {
    fn source_info(&self) -> &SourceInfo {
        match self {
            Self::Class(e) => e.source_info(),
            Self::Enumeration(e) => e.source_info(),
            Self::Function(e) => e.source_info(),
            Self::Profile(e) => e.source_info(),
            Self::Association(e) => e.source_info(),
            Self::Measure(e) => e.source_info(),
        }
    }
}

impl PackageableElement for Element {
    fn package(&self) -> Option<&Package> {
        match self {
            Self::Class(e) => e.package(),
            Self::Enumeration(e) => e.package(),
            Self::Function(e) => e.package(),
            Self::Profile(e) => e.package(),
            Self::Association(e) => e.package(),
            Self::Measure(e) => e.package(),
        }
    }

    fn name(&self) -> &Identifier {
        match self {
            Self::Class(e) => e.name(),
            Self::Enumeration(e) => e.name(),
            Self::Function(e) => e.name(),
            Self::Profile(e) => e.name(),
            Self::Association(e) => e.name(),
            Self::Measure(e) => e.name(),
        }
    }
}

impl Annotated for Element {
    fn stereotypes(&self) -> &[StereotypePtr] {
        match self {
            Self::Class(e) => e.stereotypes(),
            Self::Enumeration(e) => e.stereotypes(),
            Self::Function(e) => e.stereotypes(),
            Self::Profile(e) => e.stereotypes(),
            Self::Association(e) => e.stereotypes(),
            Self::Measure(e) => e.stereotypes(),
        }
    }

    fn tagged_values(&self) -> &[TaggedValue] {
        match self {
            Self::Class(e) => e.tagged_values(),
            Self::Enumeration(e) => e.tagged_values(),
            Self::Function(e) => e.tagged_values(),
            Self::Profile(e) => e.tagged_values(),
            Self::Association(e) => e.tagged_values(),
            Self::Measure(e) => e.tagged_values(),
        }
    }
}


// ---------------------------------------------------------------------------
// ProfileDef
// ---------------------------------------------------------------------------

/// A profile definition: `Profile meta::pure::profiles::doc { stereotypes: [...]; tags: [...]; }`.
#[derive(Debug, Clone, PartialEq, crate::PackageableElement)]
pub struct ProfileDef {
    /// The package this profile belongs to.
    pub package: Option<Package>,
    /// The profile name.
    pub name: Identifier,
    /// Stereotype names declared by this profile.
    pub stereotypes: Vec<SpannedString>,
    /// Tag names declared by this profile.
    pub tags: Vec<SpannedString>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// EnumDef
// ---------------------------------------------------------------------------

/// An enumeration definition: `Enum <<stereo>> {tag='val'} MyEnum { A, B, C }`.
#[derive(Debug, Clone, PartialEq, crate::PackageableElement)]
pub struct EnumDef {
    /// The package.
    pub package: Option<Package>,
    /// The enum name.
    pub name: Identifier,
    /// Enum values (members).
    pub values: Vec<EnumValue>,
    /// Stereotypes on the enum.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values on the enum.
    pub tagged_values: Vec<TaggedValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A single value (member) in an enumeration: `<<stereo>> {tag='val'} ValueName`.
#[derive(Debug, Clone, PartialEq, crate::Annotated)]
pub struct EnumValue {
    /// The value name.
    pub name: Identifier,
    /// Stereotypes on this enum value.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values on this enum value.
    pub tagged_values: Vec<TaggedValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// ClassDef & related
// ---------------------------------------------------------------------------

/// Aggregation kind for properties.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregationKind {
    /// No aggregation — `(none)`.
    None,
    /// Shared aggregation — `(shared)`.
    Shared,
    /// Composite aggregation — `(composite)`.
    Composite,
}

/// A class property: `<<stereo>> {tag='val'} name: Type[mult]`.
#[derive(Debug, Clone, PartialEq, crate::Annotated)]
pub struct Property {
    /// Property name.
    pub name: Identifier,
    /// Property type.
    pub type_ref: TypeReference,
    /// Multiplicity.
    pub multiplicity: Multiplicity,
    /// Aggregation kind (if specified).
    pub aggregation: Option<AggregationKind>,
    /// Default value expression (if specified).
    pub default_value: Option<Expression>,
    /// Stereotypes on this property.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values on this property.
    pub tagged_values: Vec<TaggedValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A qualified (derived) property: `name(params) { body }: ReturnType[mult]`.
#[derive(Debug, Clone, PartialEq, crate::Annotated)]
pub struct QualifiedProperty {
    /// Property name.
    pub name: Identifier,
    /// Parameters.
    pub parameters: Vec<Parameter>,
    /// Return type.
    pub return_type: TypeReference,
    /// Return multiplicity.
    pub return_multiplicity: Multiplicity,
    /// Body expressions.
    pub body: Vec<Expression>,
    /// Stereotypes on this qualified property.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values on this qualified property.
    pub tagged_values: Vec<TaggedValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A class constraint.
///
/// Constraints can be unnamed (just an expression) or named with optional
/// enforcement level, external ID, and message function.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct Constraint {
    /// Constraint name (optional — unnamed constraints are allowed).
    pub name: Option<Identifier>,
    /// The constraint function/expression.
    pub function_definition: Expression,
    /// Enforcement level, e.g., `Warn`, `Error`.
    pub enforcement_level: Option<Identifier>,
    /// External identifier.
    pub external_id: Option<String>,
    /// Message function (evaluated when constraint fails).
    pub message: Option<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A class definition with properties, constraints, stereotypes, tagged values,
/// qualified properties, and type parameters.
///
/// ```text
/// Class <<temporal.businesstemporal>> {doc.description = 'A person'}
///   model::domain::Person extends model::domain::LegalEntity
/// [
///   $this.name->isNotEmpty(),
///   ageConstraint: $this.age >= 0
/// ]
/// {
///   name: String[1];
///   age: Integer[0..1];
///   fullName(sep: String[1]) { $this.firstName + $sep + $this.lastName }: String[1];
/// }
/// ```
#[derive(Debug, Clone, PartialEq, crate::PackageableElement)]
pub struct ClassDef {
    /// The package.
    pub package: Option<Package>,
    /// The class name.
    pub name: Identifier,
    /// Type parameters (e.g., `<T, U>` — supported in Rust parser, unlike Java).
    pub type_parameters: Vec<Identifier>,
    /// Super types (`extends`).
    pub super_types: Vec<TypeReference>,
    /// Regular properties.
    pub properties: Vec<Property>,
    /// Qualified (derived) properties.
    pub qualified_properties: Vec<QualifiedProperty>,
    /// Constraints.
    pub constraints: Vec<Constraint>,
    /// Stereotypes.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values.
    pub tagged_values: Vec<TaggedValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// AssociationDef
// ---------------------------------------------------------------------------

/// An association definition linking two classes.
///
/// ```text
/// Association <<stereo>> model::Person_Firm {
///   employee: model::Person[*];
///   employer: model::Firm[*];
/// }
/// ```
#[derive(Debug, Clone, PartialEq, crate::PackageableElement)]
pub struct AssociationDef {
    /// The package.
    pub package: Option<Package>,
    /// The association name.
    pub name: Identifier,
    /// Properties (typically exactly two).
    pub properties: Vec<Property>,
    /// Qualified properties.
    pub qualified_properties: Vec<QualifiedProperty>,
    /// Stereotypes.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values.
    pub tagged_values: Vec<TaggedValue>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// MeasureDef
// ---------------------------------------------------------------------------

/// A unit definition within a measure.
///
/// Can be canonical (the `*` unit) or non-canonical with a conversion function.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct UnitDef {
    /// Unit name.
    pub name: Identifier,
    /// Conversion parameter name (e.g., `x` in `x -> $x * 1000`).
    pub conversion_param: Option<Identifier>,
    /// Conversion body expression.
    pub conversion_body: Option<Expression>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A measure definition with a canonical unit and non-canonical units.
///
/// ```text
/// Measure pkg::NewMeasure {
///   *UnitOne: x -> $x;
///   UnitTwo: x -> $x * 1000;
/// }
/// ```
#[derive(Debug, Clone, PartialEq, crate::PackageableElement)]
pub struct MeasureDef {
    /// The package.
    pub package: Option<Package>,
    /// The measure name.
    pub name: Identifier,
    /// The canonical unit (marked with `*`).
    pub canonical_unit: Option<UnitDef>,
    /// Non-canonical units.
    pub non_canonical_units: Vec<UnitDef>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// FunctionDef
// ---------------------------------------------------------------------------

/// Test data for a function test — either inline or reference.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct FunctionTestData {
    /// The store reference (e.g., `ModelStore`, `store::MyStore`).
    pub store: Package,
    /// The data format (e.g., `JSON`, `XML`), if inline.
    pub format: Option<Identifier>,
    /// The inline data string, or reference path.
    pub data: FunctionTestDataValue,
    /// Source location.
    pub source_info: SourceInfo,
}

/// The value of function test data — either inline content or a reference.
#[derive(Debug, Clone, PartialEq)]
pub enum FunctionTestDataValue {
    /// Inline string data.
    Inline(String),
    /// Reference to external data.
    Reference(Package),
}

/// An assertion in a function test: `testName | funcName(args) => expectedResult`.
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct FunctionTestAssertion {
    /// Test assertion name.
    pub name: Identifier,
    /// The function invocation expression.
    pub invocation: Expression,
    /// Expected result format (e.g., `JSON`, `XML`), if specified.
    pub expected_format: Option<Identifier>,
    /// Expected result value.
    pub expected: Expression,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A function test block (named test suite).
#[derive(Debug, Clone, PartialEq, crate::Spanned)]
pub struct FunctionTest {
    /// Test suite name (optional — unnamed suites are allowed).
    pub name: Option<Identifier>,
    /// Test data (store bindings).
    pub data: Vec<FunctionTestData>,
    /// Test assertions.
    pub assertions: Vec<FunctionTestAssertion>,
    /// Source location.
    pub source_info: SourceInfo,
}

/// A function definition.
///
/// ```text
/// function <<stereo>> {tag='val'}
///   model::hello(name: String[1]): String[1]
/// {
///   'Hello ' + $name
/// }
/// {
///   myTest | hello('World') => 'Hello World';
/// }
/// ```
#[derive(Debug, Clone, PartialEq, crate::PackageableElement)]
pub struct FunctionDef {
    /// The package.
    pub package: Option<Package>,
    /// The function name.
    pub name: Identifier,
    /// Parameters.
    pub parameters: Vec<Parameter>,
    /// Return type.
    pub return_type: TypeReference,
    /// Return multiplicity.
    pub return_multiplicity: Multiplicity,
    /// Body expressions.
    pub body: Vec<Expression>,
    /// Stereotypes.
    pub stereotypes: Vec<StereotypePtr>,
    /// Tagged values.
    pub tagged_values: Vec<TaggedValue>,
    /// Function tests.
    pub tests: Vec<FunctionTest>,
    /// Source location.
    pub source_info: SourceInfo,
}

// ---------------------------------------------------------------------------
// Visitor
// ---------------------------------------------------------------------------

/// Visitor pattern for walking top-level elements.
///
/// Implement this for compiler passes, linters, protocol converters, etc.
pub trait ElementVisitor {
    /// Visit a class definition.
    fn visit_class(&mut self, class: &ClassDef);
    /// Visit an enumeration definition.
    fn visit_enum(&mut self, enum_def: &EnumDef);
    /// Visit a function definition.
    fn visit_function(&mut self, func: &FunctionDef);
    /// Visit a profile definition.
    fn visit_profile(&mut self, profile: &ProfileDef);
    /// Visit an association definition.
    fn visit_association(&mut self, assoc: &AssociationDef);
    /// Visit a measure definition.
    fn visit_measure(&mut self, measure: &MeasureDef);
}

impl Element {
    /// Accepts a visitor and dispatches to the appropriate `visit_*` method.
    pub fn accept(&self, visitor: &mut dyn ElementVisitor) {
        match self {
            Self::Class(e) => visitor.visit_class(e),
            Self::Enumeration(e) => visitor.visit_enum(e),
            Self::Function(e) => visitor.visit_function(e),
            Self::Profile(e) => visitor.visit_profile(e),
            Self::Association(e) => visitor.visit_association(e),
            Self::Measure(e) => visitor.visit_measure(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::annotation::PackageableElementPtr;
    use smol_str::SmolStr;

    use crate::test_utils::src;

    #[test]
    fn test_profile_def() {
        let profile = ProfileDef {
            package: Some(Package::root(SmolStr::new("meta"), src())),
            name: SmolStr::new("doc"),
            stereotypes: vec![SpannedString {
                value: SmolStr::new("deprecated"),
                source_info: src(),
            }],
            tags: vec![SpannedString {
                value: SmolStr::new("description"),
                source_info: src(),
            }],
            source_info: src(),
        };

        assert_eq!(profile.name(), "doc");
        assert_eq!(profile.package().unwrap().name(), "meta");
        assert_eq!(profile.stereotypes.len(), 1);
        assert_eq!(profile.tags.len(), 1);
    }

    #[test]
    fn test_enum_def() {
        let enum_def = EnumDef {
            package: Some(Package::root(SmolStr::new("model"), src())),
            name: SmolStr::new("Color"),
            values: vec![
                EnumValue {
                    name: SmolStr::new("RED"),
                    stereotypes: vec![],
                    tagged_values: vec![],
                    source_info: src(),
                },
                EnumValue {
                    name: SmolStr::new("GREEN"),
                    stereotypes: vec![],
                    tagged_values: vec![],
                    source_info: src(),
                },
            ],
            stereotypes: vec![],
            tagged_values: vec![],
            source_info: src(),
        };

        assert_eq!(enum_def.name(), "Color");
        assert_eq!(enum_def.values.len(), 2);
        assert!(enum_def.stereotypes().is_empty());
    }

    #[test]
    fn test_class_def_with_stereotype() {
        let class = ClassDef {
            package: Some(Package::root(SmolStr::new("model"), src())),
            name: SmolStr::new("Person"),
            type_parameters: vec![],
            super_types: vec![],
            properties: vec![Property {
                name: SmolStr::new("name"),
                type_ref: TypeReference {
                    path: Package::root(SmolStr::new("String"), src()),
                    type_arguments: vec![],
                    type_variable_values: vec![],
                    source_info: src(),
                },
                multiplicity: Multiplicity::one(),
                aggregation: None,
                default_value: None,
                stereotypes: vec![],
                tagged_values: vec![],
                source_info: src(),
            }],
            qualified_properties: vec![],
            constraints: vec![],
            stereotypes: vec![StereotypePtr {
                profile: PackageableElementPtr {
                    package: None,
                    name: SmolStr::new("temporal"),
                    source_info: src(),
                },
                value: SmolStr::new("businesstemporal"),
                source_info: src(),
            }],
            tagged_values: vec![],
            source_info: src(),
        };

        assert_eq!(class.name(), "Person");
        assert_eq!(class.properties.len(), 1);
        assert_eq!(class.stereotypes().len(), 1);
        assert_eq!(class.stereotypes()[0].value, "businesstemporal");
    }

    #[test]
    fn test_element_enum_dispatch() {
        let profile = Element::Profile(ProfileDef {
            package: None,
            name: SmolStr::new("doc"),
            stereotypes: vec![],
            tags: vec![],
            source_info: src(),
        });

        assert_eq!(profile.source_info().start_line, 1);
        assert_eq!(profile.name(), "doc");
        assert!(profile.package().is_none());
    }

    #[test]
    fn test_visitor_dispatch() {
        struct Counter {
            classes: u32,
            profiles: u32,
        }

        impl ElementVisitor for Counter {
            fn visit_class(&mut self, _: &ClassDef) {
                self.classes += 1;
            }
            fn visit_profile(&mut self, _: &ProfileDef) {
                self.profiles += 1;
            }
            fn visit_enum(&mut self, _: &EnumDef) {}
            fn visit_function(&mut self, _: &FunctionDef) {}
            fn visit_association(&mut self, _: &AssociationDef) {}
            fn visit_measure(&mut self, _: &MeasureDef) {}
        }

        let elements = vec![
            Element::Profile(ProfileDef {
                package: None,
                name: SmolStr::new("doc"),
                stereotypes: vec![],
                tags: vec![],
                source_info: src(),
            }),
            Element::Class(ClassDef {
                package: None,
                name: SmolStr::new("Person"),
                type_parameters: vec![],
                super_types: vec![],
                properties: vec![],
                qualified_properties: vec![],
                constraints: vec![],
                stereotypes: vec![],
                tagged_values: vec![],
                source_info: src(),
            }),
        ];

        let mut counter = Counter {
            classes: 0,
            profiles: 0,
        };

        for element in &elements {
            element.accept(&mut counter);
        }

        assert_eq!(counter.classes, 1);
        assert_eq!(counter.profiles, 1);
    }
}
