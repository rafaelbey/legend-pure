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

//! Runtime values — the core `Value` enum for Pure expression evaluation.
//!
//! Every Pure expression evaluates to a `Value`. Primitive types (`Integer`,
//! `Float`, `String`, `Boolean`, `Decimal`) are stored inline with zero
//! allocation. `Decimal` uses `rust_decimal::Decimal` for native arithmetic.
//! Date/time values use `jiff`-backed [`PureDate`] and
//! [`StrictTime`] for native calendar arithmetic.
//!
//! Collections use persistent data structures (`im_rc::Vector`, `im_rc::HashMap`)
//! for structural sharing — this transforms fold+put patterns from
//! O(N²) to O(N log N).
//!
//! Object references use [`ObjectId`] handles into the [`RuntimeHeap`](super::heap::RuntimeHeap),
//! providing identity-preserving semantics for `mutateAdd`.

use std::fmt;

use im_rc::Vector as PVector;
use rust_decimal::Decimal;
use smol_str::SmolStr;

use crate::date::{PureDate, StrictTime};
use crate::heap::ObjectId;

/// A runtime value produced by evaluating a Pure expression.
///
/// Design decisions:
/// - Primitives are unboxed (no heap allocation for `Integer`, `Float`, `Boolean`)
/// - `Decimal` uses `rust_decimal::Decimal` — `Copy`, native arithmetic
/// - `Date` uses `PureDate` — `jiff`-backed, `Copy`, native calendar arithmetic
/// - `StrictTime` uses `jiff::civil::Time` — `Copy`
/// - Strings use `SmolStr` (inline for short strings, shared heap for longer)
/// - Collections use `im_rc` persistent structures for structural sharing
/// - Objects are handles (`ObjectId`) into the `RuntimeHeap`, not direct pointers
#[derive(Debug, Clone)]
pub enum Value {
    /// Pure `Boolean` — stored inline.
    Boolean(bool),

    /// Pure `Integer` — stored inline as `i64`.
    Integer(i64),

    /// Pure `Float` — stored inline as `f64`.
    Float(f64),

    /// Pure `Decimal` — fixed-point decimal.
    ///
    /// Uses `rust_decimal::Decimal` (`Copy`) for native arithmetic.
    /// Supports up to 28-29 significant digits — sufficient for all financial
    /// calculations in Legend Engine.
    Decimal(Decimal),

    /// Pure `String` — inline for short strings via `SmolStr`.
    String(SmolStr),

    /// Pure `Date`, `StrictDate`, `DateTime` — variable-precision temporal value.
    ///
    /// Uses `PureDate` backed by `jiff::civil::DateTime` for native calendar
    /// arithmetic (`add_days`, `add_months`, etc.). All datetime values are
    /// stored as UTC. Timezone conversion is done at format-time only.
    Date(PureDate),

    /// Pure `StrictTime` — time of day without date.
    ///
    /// Uses `jiff::civil::Time` (`Copy`, nanosecond precision).
    StrictTime(StrictTime),

    /// A reference to a runtime object on the [`RuntimeHeap`](super::heap::RuntimeHeap).
    ///
    /// This is a lightweight handle — the actual object data (properties,
    /// classifier) lives in the heap. Multiple `Value::Object` instances
    /// can point to the same `ObjectId`, enabling identity-preserving
    /// `mutateAdd` semantics.
    Object(ObjectId),

    /// An ordered collection of values — backed by an RRB-tree persistent
    /// vector (`im_rc::Vector`).
    ///
    /// Operations like `map`, `filter`, `concatenate` produce new vectors
    /// via structural sharing instead of full copies.
    Collection(PVector<Value>),

    /// A Pure `Map<K, V>` — backed by a HAMT persistent hash map.
    ///
    /// `put` operations produce new maps via structural sharing.
    /// This is the key optimization for fold+put accumulator patterns.
    Map(im_rc::HashMap<ValueKey, Value>),

    /// The unit value — result of expressions with no meaningful return.
    /// Equivalent to `[]` with multiplicity `[0..0]`.
    Unit,
}

/// A hashable key for `Map` entries.
///
/// Only value types that are meaningfully comparable can be map keys.
/// Objects are keyed by identity (`ObjectId`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ValueKey {
    /// Boolean key.
    Boolean(bool),
    /// Integer key.
    Integer(i64),
    /// Decimal key.
    Decimal(Decimal),
    /// String key.
    String(SmolStr),
    /// Date key.
    Date(PureDate),
    /// `StrictTime` key.
    StrictTime(StrictTime),
    /// Object identity key.
    Object(ObjectId),
}

// ---------------------------------------------------------------------------
// Value — equality
// ---------------------------------------------------------------------------

impl PartialEq for Value {
    #[allow(clippy::match_same_arms)] // Arms kept separate for clarity — each variant is semantically distinct
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Boolean(a), Self::Boolean(b)) => a == b,
            (Self::Integer(a), Self::Integer(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::Decimal(a), Self::Decimal(b)) => a == b,
            (Self::String(a), Self::String(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::StrictTime(a), Self::StrictTime(b)) => a == b,
            (Self::Object(a), Self::Object(b)) => a == b,
            (Self::Collection(a), Self::Collection(b)) => a == b,
            (Self::Unit, Self::Unit) => true,
            _ => false,
        }
    }
}

impl Eq for Value {}

// ---------------------------------------------------------------------------
// Value — conversion helpers
// ---------------------------------------------------------------------------

impl Value {
    /// Extract a boolean, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `Boolean`.
    pub fn as_boolean(&self) -> Result<bool, crate::error::PureRuntimeError> {
        match self {
            Self::Boolean(b) => Ok(*b),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "Boolean", other,
            )),
        }
    }

    /// Extract an integer, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not an `Integer`.
    pub fn as_integer(&self) -> Result<i64, crate::error::PureRuntimeError> {
        match self {
            Self::Integer(i) => Ok(*i),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "Integer", other,
            )),
        }
    }

    /// Extract a float, or return a type error.
    ///
    /// Integers are auto-promoted to float.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `Float` or `Integer`.
    #[allow(clippy::cast_precision_loss)] // Intentional: Pure semantics require Integer→Float promotion
    pub fn as_float(&self) -> Result<f64, crate::error::PureRuntimeError> {
        match self {
            Self::Float(f) => Ok(*f),
            Self::Integer(i) => Ok(*i as f64),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "Float", other,
            )),
        }
    }

    /// Extract a string reference, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `String`.
    pub fn as_string(&self) -> Result<&SmolStr, crate::error::PureRuntimeError> {
        match self {
            Self::String(s) => Ok(s),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "String", other,
            )),
        }
    }

    /// Extract an object ID, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not an `Object`.
    pub fn as_object(&self) -> Result<ObjectId, crate::error::PureRuntimeError> {
        match self {
            Self::Object(id) => Ok(*id),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "Object", other,
            )),
        }
    }

    /// Extract a collection reference, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `Collection`.
    pub fn as_collection(&self) -> Result<&PVector<Value>, crate::error::PureRuntimeError> {
        match self {
            Self::Collection(v) => Ok(v),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "Collection",
                other,
            )),
        }
    }

    /// Extract a decimal, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `Decimal`.
    pub fn as_decimal(&self) -> Result<Decimal, crate::error::PureRuntimeError> {
        match self {
            Self::Decimal(d) => Ok(*d),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "Decimal", other,
            )),
        }
    }

    /// Extract a date, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `Date`.
    pub fn as_date(&self) -> Result<PureDate, crate::error::PureRuntimeError> {
        match self {
            Self::Date(d) => Ok(*d),
            other => Err(crate::error::PureRuntimeError::type_mismatch("Date", other)),
        }
    }

    /// Extract a strict time, or return a type error.
    ///
    /// # Errors
    /// Returns `TypeMismatch` if this value is not a `StrictTime`.
    pub fn as_strict_time(&self) -> Result<StrictTime, crate::error::PureRuntimeError> {
        match self {
            Self::StrictTime(t) => Ok(*t),
            other => Err(crate::error::PureRuntimeError::type_mismatch(
                "StrictTime",
                other,
            )),
        }
    }

    /// Returns a human-readable type name for error messages.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Boolean(_) => "Boolean",
            Self::Integer(_) => "Integer",
            Self::Float(_) => "Float",
            Self::Decimal(_) => "Decimal",
            Self::String(_) => "String",
            Self::Date(_) => "Date",
            Self::StrictTime(_) => "StrictTime",
            Self::Object(_) => "Object",
            Self::Collection(_) => "Collection",
            Self::Map(_) => "Map",
            Self::Unit => "Unit",
        }
    }
}

// ---------------------------------------------------------------------------
// Value — multiplicity coercion
// ---------------------------------------------------------------------------

impl Value {
    /// Coerce a value to a scalar (for functions expecting `[1]`).
    ///
    /// - A scalar value returns itself.
    /// - A 1-element collection returns the inner value.
    /// - Unit or multi-element collections produce a multiplicity error.
    ///
    /// # Errors
    /// Returns `MultiplicityViolation` if the value is `Unit` (empty) or
    /// a collection with length != 1.
    pub fn to_one(&self) -> Result<&Value, crate::error::PureRuntimeError> {
        match self {
            Self::Collection(v) if v.len() == 1 => Ok(&v[0]),
            Self::Collection(v) => Err(crate::error::PureRuntimeError::MultiplicityViolation {
                expected: "[1]".into(),
                actual: v.len(),
            }),
            Self::Unit => Err(crate::error::PureRuntimeError::MultiplicityViolation {
                expected: "[1]".into(),
                actual: 0,
            }),
            other => Ok(other), // already scalar
        }
    }

    /// Coerce a value to an optional (for functions expecting `[0..1]`).
    ///
    /// - `Unit` or empty collection → `Ok(None)`
    /// - Scalar or 1-element collection → `Ok(Some(&inner))`
    /// - Multi-element collection → multiplicity error
    ///
    /// # Errors
    /// Returns `MultiplicityViolation` if the value is a collection with
    /// more than one element.
    pub fn to_zero_one(&self) -> Result<Option<&Value>, crate::error::PureRuntimeError> {
        match self {
            Self::Unit => Ok(None),
            Self::Collection(v) if v.is_empty() => Ok(None),
            Self::Collection(v) if v.len() == 1 => Ok(Some(&v[0])),
            Self::Collection(v) => Err(crate::error::PureRuntimeError::MultiplicityViolation {
                expected: "[0..1]".into(),
                actual: v.len(),
            }),
            other => Ok(Some(other)),
        }
    }

    /// Coerce a value to a collection (for functions expecting `[*]`).
    ///
    /// - A `Collection` returns a clone (O(1) via structural sharing).
    /// - `Unit` returns an empty `PVector`.
    /// - Any scalar becomes a 1-element `PVector`.
    #[must_use]
    pub fn to_collection(&self) -> PVector<Value> {
        match self {
            Self::Collection(v) => v.clone(), // O(1) persistent clone
            Self::Unit => PVector::new(),
            other => {
                let mut v = PVector::new();
                v.push_back(other.clone());
                v
            }
        }
    }

    /// Whether this value is "empty" — `Unit` or an empty collection.
    ///
    /// Used for multiplicity checks and short-circuit evaluation.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Unit => true,
            Self::Collection(v) => v.is_empty(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Value — Display
// ---------------------------------------------------------------------------

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Boolean(b) => write!(f, "{b}"),
            Self::Integer(i) => write!(f, "{i}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Decimal(d) => write!(f, "{d}"),
            Self::String(s) => write!(f, "'{s}'"),
            Self::Date(d) => write!(f, "%{d}"),
            Self::StrictTime(t) => write!(f, "%{t}"),
            Self::Object(id) => write!(f, "<Object@{id}>"),
            Self::Collection(v) => {
                write!(f, "[")?;
                for (i, item) in v.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{item}")?;
                }
                write!(f, "]")
            }
            Self::Map(m) => write!(f, "<Map size={}>", m.len()),
            Self::Unit => write!(f, "[]"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn value_integer_roundtrip() {
        let v = Value::Integer(42);
        assert_eq!(v.as_integer().unwrap(), 42);
        assert!(v.as_string().is_err());
    }

    #[test]
    fn value_float_auto_promote() {
        let v = Value::Integer(42);
        assert_eq!(v.as_float().unwrap(), 42.0);
    }

    #[test]
    fn value_string_smolstr() {
        let v = Value::String(SmolStr::new("hello"));
        assert_eq!(v.as_string().unwrap().as_str(), "hello");
    }

    #[test]
    fn value_decimal_arithmetic() {
        let a = Decimal::from_str("10.50").unwrap();
        let b = Decimal::from_str("3.25").unwrap();
        let v = Value::Decimal(a + b);
        assert_eq!(v.as_decimal().unwrap(), Decimal::from_str("13.75").unwrap());
        assert_eq!(v.to_string(), "13.75");
    }

    #[test]
    fn value_decimal_precision() {
        // No floating-point surprises
        let a = Decimal::from_str("0.1").unwrap();
        let b = Decimal::from_str("0.2").unwrap();
        let result = a + b;
        assert_eq!(result, Decimal::from_str("0.3").unwrap());
    }

    #[test]
    fn value_date_strict() {
        let d = PureDate::strict_date(2024, 3, 15).unwrap();
        let v = Value::Date(d);
        assert_eq!(v.to_string(), "%2024-03-15");
        assert_eq!(v.as_date().unwrap().get_year(), 2024);
    }

    #[test]
    fn value_date_arithmetic() {
        let d = PureDate::strict_date(2024, 3, 15).unwrap();
        let d2 = d.add_days(10).unwrap();
        assert_eq!(Value::Date(d2).to_string(), "%2024-03-25");
    }

    #[test]
    fn value_strict_time() {
        let t = StrictTime::new(10, 30, 45, 0).unwrap();
        let v = Value::StrictTime(t);
        assert_eq!(v.to_string(), "%10:30:45");
    }

    #[test]
    fn value_collection_persistent() {
        let v1 = PVector::<Value>::new();
        let v2 = v1.clone() + PVector::unit(Value::Integer(1));
        let v3 = v2.clone() + PVector::unit(Value::Integer(2));

        // v1 is still empty — structural sharing, not mutation
        assert_eq!(v1.len(), 0);
        assert_eq!(v2.len(), 1);
        assert_eq!(v3.len(), 2);
    }

    #[test]
    fn value_map_persistent() {
        let m1 = im_rc::HashMap::<ValueKey, Value>::new();
        let m2 = m1.update(ValueKey::String("a".into()), Value::Integer(1));
        let m3 = m2.update(ValueKey::String("b".into()), Value::Integer(2));

        // m1 is still empty
        assert_eq!(m1.len(), 0);
        assert_eq!(m2.len(), 1);
        assert_eq!(m3.len(), 2);
    }

    #[test]
    fn value_display() {
        assert_eq!(Value::Integer(42).to_string(), "42");
        assert_eq!(Value::String("hi".into()).to_string(), "'hi'");
        assert_eq!(Value::Unit.to_string(), "[]");
        assert_eq!(Value::Boolean(true).to_string(), "true");
    }

    #[test]
    fn value_equality() {
        assert_eq!(Value::Integer(1), Value::Integer(1));
        assert_ne!(Value::Integer(1), Value::Integer(2));
        assert_ne!(Value::Integer(1), Value::String("1".into()));
    }

    #[test]
    fn value_decimal_equality() {
        let a = Value::Decimal(Decimal::from_str("42.00").unwrap());
        let b = Value::Decimal(Decimal::from_str("42.00").unwrap());
        assert_eq!(a, b);
    }

    #[test]
    fn value_date_equality() {
        let a = Value::Date(PureDate::strict_date(2024, 3, 15).unwrap());
        let b = Value::Date(PureDate::strict_date(2024, 3, 15).unwrap());
        assert_eq!(a, b);
    }

    // -----------------------------------------------------------------------
    // Multiplicity coercion tests
    // -----------------------------------------------------------------------

    #[test]
    fn to_one_scalar() {
        let v = Value::Integer(42);
        assert_eq!(*v.to_one().unwrap(), Value::Integer(42));
    }

    #[test]
    fn to_one_single_collection() {
        let mut pv = PVector::new();
        pv.push_back(Value::Integer(42));
        let v = Value::Collection(pv);
        assert_eq!(*v.to_one().unwrap(), Value::Integer(42));
    }

    #[test]
    fn to_one_empty_collection_errors() {
        let v = Value::Collection(PVector::new());
        assert!(v.to_one().is_err());
    }

    #[test]
    fn to_one_multi_collection_errors() {
        let mut pv = PVector::new();
        pv.push_back(Value::Integer(1));
        pv.push_back(Value::Integer(2));
        let v = Value::Collection(pv);
        assert!(v.to_one().is_err());
    }

    #[test]
    fn to_one_unit_errors() {
        assert!(Value::Unit.to_one().is_err());
    }

    #[test]
    fn to_zero_one_scalar() {
        let v = Value::Integer(42);
        assert_eq!(*v.to_zero_one().unwrap().unwrap(), Value::Integer(42));
    }

    #[test]
    fn to_zero_one_unit_is_none() {
        assert!(Value::Unit.to_zero_one().unwrap().is_none());
    }

    #[test]
    fn to_zero_one_empty_collection_is_none() {
        let v = Value::Collection(PVector::new());
        assert!(v.to_zero_one().unwrap().is_none());
    }

    #[test]
    fn to_zero_one_single_collection() {
        let mut pv = PVector::new();
        pv.push_back(Value::String("hi".into()));
        let v = Value::Collection(pv);
        assert_eq!(
            *v.to_zero_one().unwrap().unwrap(),
            Value::String("hi".into())
        );
    }

    #[test]
    fn to_zero_one_multi_collection_errors() {
        let mut pv = PVector::new();
        pv.push_back(Value::Integer(1));
        pv.push_back(Value::Integer(2));
        let v = Value::Collection(pv);
        assert!(v.to_zero_one().is_err());
    }

    #[test]
    fn to_collection_from_scalar() {
        let v = Value::Integer(42);
        let c = v.to_collection();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0], Value::Integer(42));
    }

    #[test]
    fn to_collection_from_unit() {
        let c = Value::Unit.to_collection();
        assert!(c.is_empty());
    }

    #[test]
    fn to_collection_from_collection() {
        let mut pv = PVector::new();
        pv.push_back(Value::Integer(1));
        pv.push_back(Value::Integer(2));
        let v = Value::Collection(pv.clone());
        let c = v.to_collection();
        assert_eq!(c, pv);
    }

    #[test]
    fn is_empty_unit() {
        assert!(Value::Unit.is_empty());
    }

    #[test]
    fn is_empty_empty_collection() {
        assert!(Value::Collection(PVector::new()).is_empty());
    }

    #[test]
    fn is_empty_scalar_is_false() {
        assert!(!Value::Integer(42).is_empty());
    }

    #[test]
    fn is_empty_nonempty_collection_is_false() {
        let mut pv = PVector::new();
        pv.push_back(Value::Integer(1));
        assert!(!Value::Collection(pv).is_empty());
    }
}
