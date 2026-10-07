use std::collections::btree_map;
use std::fmt;
use std::vec;

use serde::de::value::{Error, MapAccessDeserializer, StringDeserializer};
use serde::de::{self, DeserializeOwned, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use super::{JsonNumber, JsonObject, JsonValue, normalized_f64};

pub(super) struct JsonValueVisitor;

impl<'de> Visitor<'de> for JsonValueVisitor {
    type Value = JsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON-compatible value")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(JsonValue::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        JsonValue::deserialize(deserializer)
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(JsonValue::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(JsonValue::Number(JsonNumber::I64(value)))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(JsonValue::Number(JsonNumber::from(value)))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        normalized_f64(value)
            .map(JsonValue::Number)
            .ok_or_else(|| E::custom("non-finite numbers are not valid JSON"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(JsonValue::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(JsonValue::String(value))
    }

    fn visit_seq<A>(self, mut values: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut result = Vec::with_capacity(values.size_hint().unwrap_or_default());
        while let Some(value) = values.next_element()? {
            result.push(value);
        }
        Ok(JsonValue::Array(result))
    }

    fn visit_map<A>(self, mut values: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut result = JsonObject::new();
        while let Some((name, value)) = values.next_entry()? {
            result.insert(name, value);
        }
        Ok(JsonValue::Object(result))
    }
}

pub(super) fn from_json_value<T>(value: JsonValue) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    T::deserialize(ValueDeserializer(value))
}

struct ValueDeserializer(JsonValue);

impl<'de> Deserializer<'de> for ValueDeserializer {
    type Error = Error;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.0 {
            JsonValue::Null => visitor.visit_unit(),
            JsonValue::Bool(value) => visitor.visit_bool(value),
            JsonValue::Number(JsonNumber::I64(value)) => visitor.visit_i64(value),
            JsonValue::Number(JsonNumber::U64(value)) => visitor.visit_u64(value),
            JsonValue::Number(JsonNumber::F64(value)) if value.is_finite() => {
                visitor.visit_f64(value)
            }
            JsonValue::Number(JsonNumber::F64(_)) => {
                Err(de::Error::custom("non-finite numbers are not valid JSON"))
            }
            JsonValue::String(value) => visitor.visit_string(value),
            JsonValue::Array(values) => visitor.visit_seq(ArrayAccess::new(values)),
            JsonValue::Object(values) => visitor.visit_map(ObjectAccess::new(values)),
        }
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.0 {
            JsonValue::Null => visitor.visit_none(),
            value => visitor.visit_some(Self(value)),
        }
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.0 {
            JsonValue::String(value) => visitor.visit_enum(StringDeserializer::<Error>::new(value)),
            JsonValue::Object(values) => {
                visitor.visit_enum(MapAccessDeserializer::new(ObjectAccess::new(values)))
            }
            _ => Err(de::Error::custom(
                "JSON enum must be a string or a single-entry object",
            )),
        }
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        match self.0 {
            JsonValue::Null => visitor.visit_unit(),
            _ => Err(de::Error::custom("expected null")),
        }
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes
        byte_buf seq tuple tuple_struct map struct identifier ignored_any
    }
}

struct ArrayAccess {
    values: vec::IntoIter<JsonValue>,
}

impl ArrayAccess {
    fn new(values: Vec<JsonValue>) -> Self {
        Self {
            values: values.into_iter(),
        }
    }
}

impl<'de> SeqAccess<'de> for ArrayAccess {
    type Error = Error;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: DeserializeSeed<'de>,
    {
        self.values
            .next()
            .map(|value| seed.deserialize(ValueDeserializer(value)))
            .transpose()
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

struct ObjectAccess {
    values: btree_map::IntoIter<String, JsonValue>,
    current_value: Option<JsonValue>,
}

impl ObjectAccess {
    fn new(values: JsonObject) -> Self {
        Self {
            values: values.into_iter(),
            current_value: None,
        }
    }
}

impl<'de> MapAccess<'de> for ObjectAccess {
    type Error = Error;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: DeserializeSeed<'de>,
    {
        let Some((key, value)) = self.values.next() else {
            return Ok(None);
        };
        self.current_value = Some(value);
        seed.deserialize(StringDeserializer::<Error>::new(key))
            .map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: DeserializeSeed<'de>,
    {
        let value = self
            .current_value
            .take()
            .ok_or_else(|| de::Error::custom("JSON object value requested before its key"))?;
        seed.deserialize(ValueDeserializer(value))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.values.len())
    }
}
