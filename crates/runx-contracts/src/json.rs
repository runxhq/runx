//! Boundary JSON model: deterministic value, object, and number types for cross-language contracts.
use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Value, json};

use crate::canonical_json::ecmascript_number;
use crate::schema::RunxSchema;

mod deserializer;
mod serializer;

pub use serializer::Error as JsonValueError;

pub type JsonObject = BTreeMap<String, JsonValue>;

/// Largest integer that survives every supported JSON boundary without loss.
pub const MAX_PORTABLE_INTEGER: u64 = 9_007_199_254_740_991;

impl RunxSchema for JsonValue {
    fn json_schema() -> Value {
        // An arbitrary JSON value: the committed schemas express this as an
        // empty subschema (`{}`), which accepts anything.
        json!({})
    }
}

impl RunxSchema for JsonNumber {
    fn json_schema() -> Value {
        json!({ "type": "number" })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(JsonObject),
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(deserializer::JsonValueVisitor)
    }
}

impl JsonValue {
    /// Deserialize directly from the canonical Runx JSON tree.
    ///
    /// This avoids constructing a parallel `serde_json::Value` tree at typed
    /// runtime boundaries while preserving serde's normal validation rules.
    pub fn deserialize_into<T>(self) -> Result<T, de::value::Error>
    where
        T: serde::de::DeserializeOwned,
    {
        deserializer::from_json_value(self)
    }

    /// Serialize a typed value directly into the canonical Runx JSON tree.
    pub fn from_serialize<T: Serialize + ?Sized>(value: &T) -> Result<Self, JsonValueError> {
        serializer::to_json_value(value)
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&JsonObject> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&Vec<JsonValue>> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }

    /// A whole number of zero or more. An integral float within the portable
    /// range is already held as an integer.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Number(JsonNumber::I64(value)) => u64::try_from(*value).ok(),
            Self::Number(JsonNumber::U64(value)) => Some(*value),
            _ => None,
        }
    }
}

#[must_use]
pub fn json_object(value: Option<&JsonValue>) -> Option<&JsonObject> {
    value.and_then(JsonValue::as_object)
}

#[must_use]
pub fn json_object_field<'a>(object: &'a JsonObject, field: &str) -> Option<&'a JsonObject> {
    object.get(field).and_then(JsonValue::as_object)
}

#[must_use]
pub fn json_string_field<'a>(object: &'a JsonObject, field: &str) -> Option<&'a str> {
    object.get(field).and_then(JsonValue::as_str)
}

#[must_use]
pub fn json_bool_field(object: &JsonObject, field: &str) -> Option<bool> {
    object.get(field).and_then(JsonValue::as_bool)
}

/// Strict JSON number representation for public serde boundaries.
///
/// Public serialization rejects non-finite floats. Act assignment idempotency
/// hashing deliberately uses a separate JSON.stringify-compatible writer that
/// hashes non-finite floats as `null`.
#[derive(Clone, Debug)]
pub enum JsonNumber {
    I64(i64),
    U64(u64),
    F64(f64),
}

/// Numbers are equal when they serialize, and so digest, as the same JSON
/// number, whichever variant holds them.
impl PartialEq for JsonNumber {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::I64(left), Self::I64(right)) => left == right,
            (Self::U64(left), Self::U64(right)) => left == right,
            (Self::I64(signed), Self::U64(unsigned)) | (Self::U64(unsigned), Self::I64(signed)) => {
                u64::try_from(*signed).is_ok_and(|signed| signed == *unsigned)
            }
            (Self::F64(float), Self::I64(integer)) | (Self::I64(integer), Self::F64(float)) => {
                float.fract() == 0.0 && *float == *integer as f64 && *float as i64 == *integer
            }
            (Self::F64(float), Self::U64(integer)) | (Self::U64(integer), Self::F64(float)) => {
                float.fract() == 0.0 && *float == *integer as f64 && *float as u64 == *integer
            }
            (Self::F64(left), Self::F64(right)) => left == right,
        }
    }
}

/// An unsigned integer in canonical form: `I64` whenever it fits, so equal
/// numbers compare equal whatever produced them.
impl From<u64> for JsonNumber {
    fn from(value: u64) -> Self {
        i64::try_from(value).map_or(Self::U64(value), Self::I64)
    }
}

impl JsonNumber {
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::I64(value) => Some(*value as f64),
            Self::U64(value) => Some(*value as f64),
            Self::F64(value) if value.is_finite() => Some(*value),
            Self::F64(_) => None,
        }
    }
}

pub(super) fn normalized_f64(value: f64) -> Option<JsonNumber> {
    if !value.is_finite() {
        return None;
    }
    if value.fract() == 0.0 && value.abs() <= MAX_PORTABLE_INTEGER as f64 {
        return Some(JsonNumber::I64(value as i64));
    }
    Some(JsonNumber::F64(value))
}

impl Serialize for JsonNumber {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *self {
            Self::I64(value) => serializer.serialize_i64(value),
            Self::U64(value) => serializer.serialize_u64(value),
            // A whole float in the portable range is that integer exactly;
            // past it, the canonical writer gives the float's own form.
            Self::F64(value)
                if value.fract() == 0.0 && value.abs() <= MAX_PORTABLE_INTEGER as f64 =>
            {
                serializer.serialize_i64(value as i64)
            }
            Self::F64(value) if value.is_finite() => serializer.serialize_f64(value),
            Self::F64(_) => Err(serde::ser::Error::custom(
                "non-finite numbers are not valid JSON",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for JsonNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(JsonNumberVisitor)
    }
}

struct JsonNumberVisitor;

impl Visitor<'_> for JsonNumberVisitor {
    type Value = JsonNumber;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a finite JSON number")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonNumber::I64(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(JsonNumber::from(value))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        normalized_f64(value).ok_or_else(|| E::custom("non-finite numbers are not valid JSON"))
    }
}

impl fmt::Display for JsonNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::I64(value) => write!(formatter, "{value}"),
            Self::U64(value) => write!(formatter, "{value}"),
            Self::F64(value) => match ecmascript_number(value) {
                Some(text) => formatter.write_str(&text),
                None => write!(formatter, "{value}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use serde::Deserialize;

    use super::{JsonNumber, JsonValue};

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    struct TypedFixture {
        name: String,
        flags: Vec<bool>,
    }

    #[test]
    fn numbers_are_equal_when_they_serialize_alike() {
        assert_eq!(JsonNumber::U64(200), JsonNumber::I64(200));
        assert_eq!(JsonNumber::from(200), JsonNumber::I64(200));
        assert_eq!(JsonNumber::F64(3.0), JsonNumber::U64(3));
        assert_ne!(JsonNumber::I64(-1), JsonNumber::U64(u64::MAX));
        assert_ne!(JsonNumber::F64(3.5), JsonNumber::I64(3));
        assert_eq!(JsonNumber::from(u64::MAX), JsonNumber::U64(u64::MAX));
    }

    #[test]
    fn json_value_deserializes_directly_into_typed_inputs() {
        let value = JsonValue::Object(
            [
                ("name".to_owned(), JsonValue::String("runx".to_owned())),
                (
                    "flags".to_owned(),
                    JsonValue::Array(vec![JsonValue::Bool(true), JsonValue::Bool(false)]),
                ),
            ]
            .into_iter()
            .collect(),
        );

        let decoded = value
            .deserialize_into::<TypedFixture>()
            .expect("canonical JSON should deserialize without a second tree");

        assert_eq!(
            decoded,
            TypedFixture {
                name: "runx".to_owned(),
                flags: vec![true, false],
            }
        );
    }

    #[test]
    fn json_value_round_trips_objects_with_sorted_keys() -> Result<(), serde_json::Error> {
        let value = JsonValue::Object(
            [
                ("z".to_owned(), JsonValue::String("last".to_owned())),
                ("a".to_owned(), JsonValue::Number(JsonNumber::I64(1))),
            ]
            .into_iter()
            .collect(),
        );

        let json = serde_json::to_string(&value)?;
        let decoded: JsonValue = serde_json::from_str(&json)?;

        assert_eq!(json, r#"{"a":1,"z":"last"}"#);
        assert_eq!(decoded, value);
        Ok(())
    }

    #[test]
    fn json_value_preserves_fractional_numbers() -> Result<(), serde_json::Error> {
        let value = JsonValue::Number(JsonNumber::F64(0.91));

        let json = serde_json::to_string(&value)?;
        let decoded: JsonValue = serde_json::from_str(&json)?;

        assert_eq!(json, "0.91");
        assert_eq!(decoded, value);
        Ok(())
    }

    #[test]
    fn json_number_serializes_whole_floats_as_json_integers() -> Result<(), serde_json::Error> {
        let value = JsonValue::Number(JsonNumber::F64(1.0));

        let json = serde_json::to_string(&value)?;

        assert_eq!(json, "1");
        Ok(())
    }

    #[test]
    fn json_value_normalizes_whole_floats_for_typed_integer_inputs()
    -> Result<(), Box<dyn std::error::Error>> {
        let value: JsonValue = serde_json::from_str("2.0")?;

        assert_eq!(value, JsonValue::Number(JsonNumber::I64(2)));
        assert_eq!(value.deserialize_into::<u64>()?, 2);
        Ok(())
    }

    #[test]
    fn json_number_rejects_non_finite_float_serialization() {
        let value = JsonValue::Number(JsonNumber::F64(f64::NAN));

        let result = serde_json::to_string(&value);

        assert!(result.is_err());
    }
}
