//! A serde serializer that writes `runx.stable-json.v1` bytes directly from any
//! `Serialize` value: object keys sorted bytewise, JavaScript-compatible number
//! encoding, and serde_json string escaping.
//!
//! No intermediate value tree is built. Each object buffers only its own
//! encoded entries so they can be sorted; everything else streams.

use std::borrow::Cow;
use std::ops::Range;

use serde::ser::{self, Impossible, Serialize};

use super::CanonicalJsonError;

/// Encode `value` as canonical JSON bytes.
pub fn to_canonical_bytes<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalJsonError> {
    let mut output = Vec::with_capacity(128);
    value.serialize(Canonical {
        output: &mut output,
    })?;
    Ok(output)
}

impl ser::Error for CanonicalJsonError {
    fn custom<T: std::fmt::Display>(message: T) -> Self {
        Self::Serialization {
            message: message.to_string(),
        }
    }
}

fn error(message: &str) -> CanonicalJsonError {
    CanonicalJsonError::Serialization {
        message: message.to_owned(),
    }
}

fn write_str(output: &mut Vec<u8>, value: &str) -> Result<(), CanonicalJsonError> {
    serde_json::to_writer(&mut *output, value).map_err(|source| CanonicalJsonError::Serialization {
        message: source.to_string(),
    })
}

fn write_f64(output: &mut Vec<u8>, value: f64) -> Result<(), CanonicalJsonError> {
    let text =
        ecmascript_number(value).ok_or_else(|| error("non-finite numbers are not valid JSON"))?;
    output.extend_from_slice(text.as_bytes());
    Ok(())
}

/// A float as ECMAScript's `Number.prototype.toString` writes it, the form
/// `JSON.stringify` uses: the shortest round-trip digits, positional from
/// 1e-6 up to 1e21, exponential with an explicit sign outside that. `None`
/// for a non-finite value, which JSON cannot carry.
#[must_use]
pub fn ecmascript_number(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    if value == 0.0 {
        return Some("0".to_owned());
    }
    // Rust's `{:e}` writes the shortest round-trip digits: `d[.ddd]e<exp>`.
    let shortest = format!("{:e}", value.abs());
    let (mantissa, exponent) = shortest.split_once('e')?;
    let digits = mantissa.replace('.', "");
    let length = digits.len();
    // The decimal point sits after `point` digits: value = 0.digits * 10^point.
    let point = exponent.parse::<i32>().ok()? + 1;
    let sign = if value < 0.0 { "-" } else { "" };
    let unsigned = match usize::try_from(point) {
        Ok(whole) if length <= whole && whole <= 21 => {
            format!("{digits}{}", "0".repeat(whole - length))
        }
        Ok(whole) if whole > 0 && whole <= 21 => {
            let (integer, fraction) = digits.split_at(whole);
            format!("{integer}.{fraction}")
        }
        Err(_) | Ok(0) if point > -6 => {
            format!("0.{}{digits}", "0".repeat(point.unsigned_abs() as usize))
        }
        _ => {
            let (first, rest) = digits.split_at(1);
            let fraction = if rest.is_empty() {
                String::new()
            } else {
                format!(".{rest}")
            };
            let exponent_sign = if point > 0 { '+' } else { '-' };
            format!(
                "{first}{fraction}e{exponent_sign}{}",
                (point - 1).unsigned_abs()
            )
        }
    };
    Some(format!("{sign}{unsigned}"))
}

struct Canonical<'a> {
    output: &'a mut Vec<u8>,
}

/// One object's entries: each key with the range of its encoded value in a
/// shared scratch buffer, sorted bytewise by key on close.
struct Object<'a> {
    output: &'a mut Vec<u8>,
    scratch: Vec<u8>,
    entries: Vec<(Cow<'static, str>, Range<usize>)>,
    pending_key: Option<String>,
    /// For enum variants encoded as `{"variant": ...}`.
    variant: Option<&'static str>,
}

impl<'a> Object<'a> {
    fn new(output: &'a mut Vec<u8>, variant: Option<&'static str>) -> Self {
        Self {
            output,
            scratch: Vec::new(),
            entries: Vec::new(),
            pending_key: None,
            variant,
        }
    }

    fn entry<T: Serialize + ?Sized>(
        &mut self,
        key: Cow<'static, str>,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        let start = self.scratch.len();
        value.serialize(Canonical {
            output: &mut self.scratch,
        })?;
        self.entries.push((key, start..self.scratch.len()));
        Ok(())
    }

    fn finish(mut self) -> Result<(), CanonicalJsonError> {
        self.entries
            .sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if self.entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(error("duplicate object key"));
        }
        if let Some(variant) = self.variant {
            self.output.push(b'{');
            write_str(self.output, variant)?;
            self.output.push(b':');
        }
        self.output.push(b'{');
        for (index, (key, range)) in self.entries.iter().enumerate() {
            if index > 0 {
                self.output.push(b',');
            }
            write_str(self.output, key)?;
            self.output.push(b':');
            self.output.extend_from_slice(&self.scratch[range.clone()]);
        }
        self.output.push(b'}');
        if self.variant.is_some() {
            self.output.push(b'}');
        }
        Ok(())
    }
}

struct Array<'a> {
    output: &'a mut Vec<u8>,
    first: bool,
    variant: Option<&'static str>,
}

impl<'a> Array<'a> {
    fn open(
        output: &'a mut Vec<u8>,
        variant: Option<&'static str>,
    ) -> Result<Self, CanonicalJsonError> {
        if let Some(variant) = variant {
            output.push(b'{');
            write_str(output, variant)?;
            output.push(b':');
        }
        output.push(b'[');
        Ok(Self {
            output,
            first: true,
            variant,
        })
    }

    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), CanonicalJsonError> {
        if !self.first {
            self.output.push(b',');
        }
        self.first = false;
        value.serialize(Canonical {
            output: &mut *self.output,
        })
    }

    fn close(self) -> Result<(), CanonicalJsonError> {
        self.output.push(b']');
        if self.variant.is_some() {
            self.output.push(b'}');
        }
        Ok(())
    }
}

macro_rules! integer {
    ($($method:ident: $ty:ty),+) => {
        $(fn $method(self, value: $ty) -> Result<(), CanonicalJsonError> {
            self.output.extend_from_slice(value.to_string().as_bytes());
            Ok(())
        })+
    };
}

impl<'a> ser::Serializer for Canonical<'a> {
    type Ok = ();
    type Error = CanonicalJsonError;
    type SerializeSeq = Array<'a>;
    type SerializeTuple = Array<'a>;
    type SerializeTupleStruct = Array<'a>;
    type SerializeTupleVariant = Array<'a>;
    type SerializeMap = Object<'a>;
    type SerializeStruct = Object<'a>;
    type SerializeStructVariant = Object<'a>;

    fn serialize_bool(self, value: bool) -> Result<(), CanonicalJsonError> {
        self.output
            .extend_from_slice(if value { b"true" } else { b"false" });
        Ok(())
    }

    integer!(
        serialize_i8: i8, serialize_i16: i16, serialize_i32: i32, serialize_i64: i64,
        serialize_i128: i128, serialize_u8: u8, serialize_u16: u16, serialize_u32: u32,
        serialize_u64: u64, serialize_u128: u128
    );

    fn serialize_f32(self, value: f32) -> Result<(), CanonicalJsonError> {
        write_f64(self.output, f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<(), CanonicalJsonError> {
        write_f64(self.output, value)
    }

    fn serialize_char(self, value: char) -> Result<(), CanonicalJsonError> {
        write_str(self.output, value.encode_utf8(&mut [0; 4]))
    }

    fn serialize_str(self, value: &str) -> Result<(), CanonicalJsonError> {
        write_str(self.output, value)
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<(), CanonicalJsonError> {
        let mut array = Array::open(self.output, None)?;
        for byte in value {
            array.element(byte)?;
        }
        array.close()
    }

    fn serialize_none(self) -> Result<(), CanonicalJsonError> {
        self.output.extend_from_slice(b"null");
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), CanonicalJsonError> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<(), CanonicalJsonError> {
        self.serialize_none()
    }

    fn serialize_unit_struct(self, _: &'static str) -> Result<(), CanonicalJsonError> {
        self.serialize_none()
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<(), CanonicalJsonError> {
        write_str(self.output, variant)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.output.push(b'{');
        write_str(self.output, variant)?;
        self.output.push(b':');
        value.serialize(Canonical {
            output: &mut *self.output,
        })?;
        self.output.push(b'}');
        Ok(())
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Array<'a>, CanonicalJsonError> {
        Array::open(self.output, None)
    }

    fn serialize_tuple(self, _: usize) -> Result<Array<'a>, CanonicalJsonError> {
        Array::open(self.output, None)
    }

    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Array<'a>, CanonicalJsonError> {
        Array::open(self.output, None)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Array<'a>, CanonicalJsonError> {
        Array::open(self.output, Some(variant))
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Object<'a>, CanonicalJsonError> {
        Ok(Object::new(self.output, None))
    }

    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Object<'a>, CanonicalJsonError> {
        Ok(Object::new(self.output, None))
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Object<'a>, CanonicalJsonError> {
        Ok(Object::new(self.output, Some(variant)))
    }
}

impl ser::SerializeSeq for Array<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_element<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.element(value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.close()
    }
}

impl ser::SerializeTuple for Array<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_element<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.element(value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.close()
    }
}

impl ser::SerializeTupleStruct for Array<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.element(value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.close()
    }
}

impl ser::SerializeTupleVariant for Array<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.element(value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.close()
    }
}

impl ser::SerializeMap for Object<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), CanonicalJsonError> {
        self.pending_key = Some(key.serialize(KeyName)?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| error("map value without a key"))?;
        self.entry(Cow::Owned(key), value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.finish()
    }
}

impl ser::SerializeStruct for Object<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.entry(Cow::Borrowed(key), value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for Object<'_> {
    type Ok = ();
    type Error = CanonicalJsonError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), CanonicalJsonError> {
        self.entry(Cow::Borrowed(key), value)
    }

    fn end(self) -> Result<(), CanonicalJsonError> {
        self.finish()
    }
}

/// Object keys must be strings; string-like newtypes pass through.
struct KeyName;

impl ser::Serializer for KeyName {
    type Ok = String;
    type Error = CanonicalJsonError;
    type SerializeSeq = Impossible<String, CanonicalJsonError>;
    type SerializeTuple = Impossible<String, CanonicalJsonError>;
    type SerializeTupleStruct = Impossible<String, CanonicalJsonError>;
    type SerializeTupleVariant = Impossible<String, CanonicalJsonError>;
    type SerializeMap = Impossible<String, CanonicalJsonError>;
    type SerializeStruct = Impossible<String, CanonicalJsonError>;
    type SerializeStructVariant = Impossible<String, CanonicalJsonError>;

    fn serialize_str(self, value: &str) -> Result<String, CanonicalJsonError> {
        Ok(value.to_owned())
    }

    fn serialize_char(self, value: char) -> Result<String, CanonicalJsonError> {
        Ok(value.to_string())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<String, CanonicalJsonError> {
        value.serialize(self)
    }

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<String, CanonicalJsonError> {
        Ok(variant.to_owned())
    }

    fn serialize_bool(self, _: bool) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_i8(self, _: i8) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_i16(self, _: i16) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_i32(self, _: i32) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_i64(self, _: i64) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_u8(self, _: u8) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_u16(self, _: u16) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_u32(self, _: u32) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_u64(self, _: u64) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_f32(self, _: f32) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_f64(self, _: f64) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_none(self) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_unit(self) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<String, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, CanonicalJsonError> {
        Err(error("object keys must be strings"))
    }
}
