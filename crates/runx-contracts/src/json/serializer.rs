//! Serialize a typed value straight into a [`JsonValue`]: the inverse of
//! [`JsonValue::deserialize_into`], with no `serde_json::Value` in between.

use std::fmt;

use serde::ser::{self, Serialize};

use super::{JsonNumber, JsonObject, JsonValue, normalized_f64};

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

pub(super) fn to_json_value<T: Serialize + ?Sized>(value: &T) -> Result<JsonValue, Error> {
    value.serialize(ValueSerializer)
}

struct ValueSerializer;

pub(super) struct Seq(Vec<JsonValue>);

pub(super) struct Map {
    object: JsonObject,
    key: Option<String>,
}

pub(super) struct Variant {
    name: &'static str,
    inner: Seq,
}

pub(super) struct StructVariant {
    name: &'static str,
    inner: Map,
}

fn number(value: f64) -> Result<JsonValue, Error> {
    normalized_f64(value)
        .map(JsonValue::Number)
        .ok_or_else(|| Error("non-finite numbers are not JSON".to_owned()))
}

fn tagged(name: &'static str, value: JsonValue) -> JsonValue {
    JsonValue::Object(JsonObject::from([(name.to_owned(), value)]))
}

impl ser::Serializer for ValueSerializer {
    type Ok = JsonValue;
    type Error = Error;
    type SerializeSeq = Seq;
    type SerializeTuple = Seq;
    type SerializeTupleStruct = Seq;
    type SerializeTupleVariant = Variant;
    type SerializeMap = Map;
    type SerializeStruct = Map;
    type SerializeStructVariant = StructVariant;

    fn serialize_bool(self, value: bool) -> Result<JsonValue, Error> {
        Ok(JsonValue::Bool(value))
    }
    fn serialize_i8(self, value: i8) -> Result<JsonValue, Error> {
        self.serialize_i64(i64::from(value))
    }
    fn serialize_i16(self, value: i16) -> Result<JsonValue, Error> {
        self.serialize_i64(i64::from(value))
    }
    fn serialize_i32(self, value: i32) -> Result<JsonValue, Error> {
        self.serialize_i64(i64::from(value))
    }
    fn serialize_i64(self, value: i64) -> Result<JsonValue, Error> {
        Ok(JsonValue::Number(JsonNumber::I64(value)))
    }
    fn serialize_u8(self, value: u8) -> Result<JsonValue, Error> {
        self.serialize_u64(u64::from(value))
    }
    fn serialize_u16(self, value: u16) -> Result<JsonValue, Error> {
        self.serialize_u64(u64::from(value))
    }
    fn serialize_u32(self, value: u32) -> Result<JsonValue, Error> {
        self.serialize_u64(u64::from(value))
    }
    fn serialize_u64(self, value: u64) -> Result<JsonValue, Error> {
        Ok(JsonValue::Number(JsonNumber::U64(value)))
    }
    fn serialize_f32(self, value: f32) -> Result<JsonValue, Error> {
        number(f64::from(value))
    }
    fn serialize_f64(self, value: f64) -> Result<JsonValue, Error> {
        number(value)
    }
    fn serialize_char(self, value: char) -> Result<JsonValue, Error> {
        Ok(JsonValue::String(value.to_string()))
    }
    fn serialize_str(self, value: &str) -> Result<JsonValue, Error> {
        Ok(JsonValue::String(value.to_owned()))
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<JsonValue, Error> {
        Ok(JsonValue::Array(
            value
                .iter()
                .map(|byte| JsonValue::Number(JsonNumber::U64(u64::from(*byte))))
                .collect(),
        ))
    }
    fn serialize_none(self) -> Result<JsonValue, Error> {
        Ok(JsonValue::Null)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<JsonValue, Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<JsonValue, Error> {
        Ok(JsonValue::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<JsonValue, Error> {
        Ok(JsonValue::Null)
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<JsonValue, Error> {
        Ok(JsonValue::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<JsonValue, Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<JsonValue, Error> {
        Ok(tagged(variant, value.serialize(self)?))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<Seq, Error> {
        Ok(Seq(Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> Result<Seq, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _name: &'static str, len: usize) -> Result<Seq, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<Variant, Error> {
        Ok(Variant {
            name: variant,
            inner: Seq(Vec::with_capacity(len)),
        })
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Map, Error> {
        Ok(Map {
            object: JsonObject::new(),
            key: None,
        })
    }
    fn serialize_struct(self, _name: &'static str, len: usize) -> Result<Map, Error> {
        self.serialize_map(Some(len))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<StructVariant, Error> {
        Ok(StructVariant {
            name: variant,
            inner: self.serialize_map(Some(len))?,
        })
    }
}

impl ser::SerializeSeq for Seq {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.0.push(value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<JsonValue, Error> {
        Ok(JsonValue::Array(self.0))
    }
}

impl ser::SerializeTuple for Seq {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<JsonValue, Error> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for Seq {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, value)
    }
    fn end(self) -> Result<JsonValue, Error> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleVariant for Variant {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(&mut self.inner, value)
    }
    fn end(self) -> Result<JsonValue, Error> {
        Ok(tagged(self.name, ser::SerializeSeq::end(self.inner)?))
    }
}

impl ser::SerializeMap for Map {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        match key.serialize(ValueSerializer)? {
            JsonValue::String(key) => {
                self.key = Some(key);
                Ok(())
            }
            _ => Err(Error("JSON object keys must be strings".to_owned())),
        }
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .key
            .take()
            .ok_or_else(|| Error("a map value arrived without its key".to_owned()))?;
        self.object.insert(key, value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<JsonValue, Error> {
        Ok(JsonValue::Object(self.object))
    }
}

impl ser::SerializeStruct for Map {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.object
            .insert(key.to_owned(), value.serialize(ValueSerializer)?);
        Ok(())
    }
    fn end(self) -> Result<JsonValue, Error> {
        Ok(JsonValue::Object(self.object))
    }
}

impl ser::SerializeStructVariant for StructVariant {
    type Ok = JsonValue;
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, value)
    }
    fn end(self) -> Result<JsonValue, Error> {
        Ok(tagged(self.name, ser::SerializeStruct::end(self.inner)?))
    }
}
