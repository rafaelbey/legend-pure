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

//! Protocol `ValueSpecification` — the expression/value model.
//!
//! Maps to Java `org.finos.legend.engine.protocol.pure.m3.valuespecification.ValueSpecification`
//! and its concrete subclasses.
//!
//! Each variant wraps a named struct to avoid name collisions (e.g., `CString`
//! instead of `String`) and to enable isolated testing per variant.

use serde::{Deserialize, Serialize};

use super::generic_type::GenericType;
use super::multiplicity::Multiplicity;
use super::source_info::SourceInformation;

/// The protocol value specification — discriminated by `_type` in JSON.
///
/// Maps to the Java `ValueSpecification` hierarchy with `@JsonTypeInfo(property = "_type")`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "_type")]
pub enum ValueSpecification {
    /// An integer constant. `_type = "integer"`.
    #[serde(rename = "integer")]
    Integer(CInteger),
    /// A float constant. `_type = "float"`.
    #[serde(rename = "float")]
    Float(CFloat),
    /// A decimal constant (as string). `_type = "decimal"`.
    #[serde(rename = "decimal")]
    Decimal(CDecimal),
    /// A string constant. `_type = "string"`.
    #[serde(rename = "string")]
    String(CString),
    /// A boolean constant. `_type = "boolean"`.
    #[serde(rename = "boolean")]
    Boolean(CBoolean),
    /// A date-time value (ISO string). `_type = "dateTime"`.
    #[serde(rename = "dateTime")]
    DateTime(CDateTime),
    /// A strict (date-only) value. `_type = "strictDate"`.
    #[serde(rename = "strictDate")]
    StrictDate(CStrictDate),
    /// A strict (time-only) value. `_type = "strictTime"`.
    #[serde(rename = "strictTime")]
    StrictTime(CStrictTime),
    /// The `%latest` date sentinel. `_type = "latestDate"`.
    #[serde(rename = "latestDate")]
    LatestDate(CLatestDate),
    /// A function application. `_type = "func"`.
    #[serde(rename = "func")]
    Func(AppliedFunction),
    /// A property access. `_type = "property"`.
    #[serde(rename = "property")]
    Property(AppliedProperty),
    /// A collection literal. `_type = "collection"`.
    #[serde(rename = "collection")]
    Collection(ProtocolCollection),
    /// A variable reference. `_type = "var"`.
    #[serde(rename = "var")]
    Var(Variable),
    /// A lambda expression. `_type = "lambda"`.
    #[serde(rename = "lambda")]
    Lambda(LambdaFunction),
    /// A reference to a packageable element. `_type = "packageableElementPtr"`.
    #[serde(rename = "packageableElementPtr")]
    PackageableElementPtr(ProtocolPackageableElementPtr),
    /// A generic type instance. `_type = "genericTypeInstance"`.
    #[serde(rename = "genericTypeInstance")]
    GenericTypeInstance(ProtocolGenericTypeInstance),
    /// An enum value reference. `_type = "enumValue"`.
    #[serde(rename = "enumValue")]
    EnumValue(ProtocolEnumValue),
    /// A key-expression for `new` syntax. `_type = "keyExpression"`.
    #[serde(rename = "keyExpression")]
    KeyExpression(ProtocolKeyExpression),
    /// An opaque class instance. `_type = "classInstance"`.
    #[serde(rename = "classInstance")]
    ClassInstance(ClassInstance),
}

// ---------------------------------------------------------------------------
// Primitive value structs
// ---------------------------------------------------------------------------

/// Integer constant value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CInteger {
    /// The integer value.
    pub value: i64,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Float constant value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CFloat {
    /// The float value.
    pub value: f64,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Decimal constant value (serialized as a JSON number).
///
/// Java's `BigDecimal` is serialized as a JSON number by Jackson.
/// We use `f64` here which matches what JSON can natively represent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CDecimal {
    /// The decimal value (e.g., `3.14159`).
    pub value: f64,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// String constant value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CString {
    /// The string value.
    pub value: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Boolean constant value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CBoolean {
    /// The boolean value.
    pub value: bool,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Date-time constant value (ISO 8601 string).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CDateTime {
    /// The date-time value as an ISO string.
    pub value: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Strict date (date-only) constant value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CStrictDate {
    /// The date value as a string (e.g., `"2024-01-15"`).
    pub value: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// Strict time (time-only) constant value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CStrictTime {
    /// The time value as a string (e.g., `"10:30:00"`).
    pub value: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// The `%latest` date sentinel — a marker type with no value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CLatestDate {
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

// ---------------------------------------------------------------------------
// Complex value specification structs
// ---------------------------------------------------------------------------

/// A function application (e.g., `plus(1, 2)` or `filter(x|$x > 5)`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedFunction {
    /// The function name (e.g., `"plus"`, `"filter"`, `"map"`).
    pub function: String,
    /// Function control hint (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f_control: Option<String>,
    /// Function parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ValueSpecification>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A property access (e.g., `$person.name`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppliedProperty {
    /// The owning class (optional).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    /// The property name.
    pub property: String,
    /// Parameters (for qualified access).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<ValueSpecification>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A collection literal (e.g., `[1, 2, 3]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolCollection {
    /// The collection's multiplicity.
    pub multiplicity: Multiplicity,
    /// The collection's values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<ValueSpecification>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A variable reference (e.g., `$this`, `$name`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Variable {
    /// The variable name (without `$` prefix).
    pub name: String,
    /// The variable's generic type (when typed, e.g., in a parameter).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generic_type: Option<GenericType>,
    /// The variable's multiplicity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multiplicity: Option<Multiplicity>,
    /// Whether this variable supports streaming.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_stream: Option<bool>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A lambda expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LambdaFunction {
    /// The lambda body.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body: Vec<ValueSpecification>,
    /// Lambda parameters (serialized as value specifications).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Variable>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A reference to a packageable element by fully qualified path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolPackageableElementPtr {
    /// Fully qualified path (e.g., `"model::Person"`).
    pub full_path: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A generic type instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolGenericTypeInstance {
    /// The generic type.
    pub generic_type: GenericType,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// An enum value reference (e.g., `Color.RED`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolEnumValue {
    /// The fully qualified path of the enumeration.
    pub full_path: String,
    /// The enum value name.
    pub value: String,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// A key expression for `new` syntax (e.g., `name = 'John'`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolKeyExpression {
    /// Whether this is an add operation.
    #[serde(default)]
    pub add: bool,
    /// The key (property name as value specification).
    pub key: Box<ValueSpecification>,
    /// The expression (value).
    pub expression: Box<ValueSpecification>,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

/// An opaque class instance — for extensible types.
///
/// Maps to Java `ClassInstance` with `type` + `value`. The `value` is
/// stored as opaque `serde_json::Value`; typed deserialization comes
/// in later phases via a registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassInstance {
    /// The type name (e.g., `"path"`, `"rootGraphFetchTree"`, `"colSpec"`).
    #[serde(rename = "type")]
    pub type_name: String,
    /// The opaque value.
    pub value: serde_json::Value,
    /// Source location.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_information: Option<SourceInformation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integer_discriminator() {
        let vs = ValueSpecification::Integer(CInteger {
            value: 42,
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "integer");
        assert_eq!(json["value"], 42);
    }

    #[test]
    fn test_string_discriminator() {
        let vs = ValueSpecification::String(CString {
            value: "hello".into(),
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "string");
        assert_eq!(json["value"], "hello");
    }

    #[test]
    fn test_boolean_discriminator() {
        let vs = ValueSpecification::Boolean(CBoolean {
            value: true,
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "boolean");
        assert_eq!(json["value"], true);
    }

    #[test]
    fn test_latest_date_no_value_field() {
        let vs = ValueSpecification::LatestDate(CLatestDate {
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "latestDate");
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 1, "latestDate should only have _type");
    }

    #[test]
    fn test_func_serialization() {
        let vs = ValueSpecification::Func(AppliedFunction {
            function: "plus".into(),
            f_control: None,
            parameters: vec![
                ValueSpecification::Integer(CInteger {
                    value: 1,
                    source_information: None,
                }),
                ValueSpecification::Integer(CInteger {
                    value: 2,
                    source_information: None,
                }),
            ],
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "func");
        assert_eq!(json["function"], "plus");
        assert_eq!(json["parameters"].as_array().unwrap().len(), 2);
        assert!(json.get("fControl").is_none());
    }

    #[test]
    fn test_var_serialization() {
        let vs = ValueSpecification::Var(Variable {
            name: "this".into(),
            generic_type: None,
            multiplicity: None,
            supports_stream: None,
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "var");
        assert_eq!(json["name"], "this");
    }

    #[test]
    fn test_lambda_serialization() {
        let vs = ValueSpecification::Lambda(LambdaFunction {
            body: vec![ValueSpecification::Var(Variable {
                name: "x".into(),
                generic_type: None,
                multiplicity: None,
                supports_stream: None,
                source_information: None,
            })],
            parameters: vec![],
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "lambda");
        assert_eq!(json["body"][0]["_type"], "var");
    }

    #[test]
    fn test_class_instance_serialization() {
        let vs = ValueSpecification::ClassInstance(ClassInstance {
            type_name: "path".into(),
            value: serde_json::json!({"name": "test"}),
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "classInstance");
        assert_eq!(json["type"], "path");
        assert_eq!(json["value"]["name"], "test");
    }

    #[test]
    fn test_enum_value_serialization() {
        let vs = ValueSpecification::EnumValue(ProtocolEnumValue {
            full_path: "model::Color".into(),
            value: "RED".into(),
            source_information: None,
        });
        let json = serde_json::to_value(&vs).unwrap();
        assert_eq!(json["_type"], "enumValue");
        assert_eq!(json["fullPath"], "model::Color");
        assert_eq!(json["value"], "RED");
    }

    #[test]
    fn test_roundtrip_nested() {
        let vs = ValueSpecification::Func(AppliedFunction {
            function: "filter".into(),
            f_control: None,
            parameters: vec![
                ValueSpecification::Var(Variable {
                    name: "x".into(),
                    generic_type: None,
                    multiplicity: None,
                    supports_stream: None,
                    source_information: None,
                }),
                ValueSpecification::Lambda(LambdaFunction {
                    body: vec![ValueSpecification::Boolean(CBoolean {
                        value: true,
                        source_information: None,
                    })],
                    parameters: vec![],
                    source_information: None,
                }),
            ],
            source_information: None,
        });
        let json_str = serde_json::to_string(&vs).unwrap();
        let back: ValueSpecification = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back, vs);
    }
}
