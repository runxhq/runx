//! Pure, bounded decoding of JSON-compatible data, without source I/O.

use std::fmt;
use std::io::{self, Write};

use serde::Serialize;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use thiserror::Error;

const MAX_DEPTH: usize = 128;
mod yaml;

/// Deliberately contains no parser diagnostic or source fragment.
#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum DataDecodeError {
    #[error("unsupported data format; use json, yaml or toml")]
    UnsupportedFormat,
    #[error("invalid or unsupported JSON-compatible data")]
    InvalidData,
    #[error("decoded data exceeds its byte or nesting budget")]
    ResourceLimit,
}

/// Decode one complete document. The budget counts serialized JSON bytes while
/// constructing the value, including repeated YAML aliases. TOML dates become
/// strings; tags, duplicate/non-string keys and non-finite numbers are refused.
pub fn decode_data(source: &str, format: &str, max_bytes: usize) -> Result<Value, DataDecodeError> {
    let mut budget = Budget {
        remaining: max_bytes,
        exceeded: false,
    };
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let result = match format {
        "json" => {
            let mut deserializer = serde_json::Deserializer::from_str(source);
            Seed {
                budget: &mut budget,
                depth: 0,
            }
            .deserialize(&mut deserializer)
            .and_then(|value| deserializer.end().map(|()| value))
            .map_err(|_| DataDecodeError::InvalidData)
        }
        "yaml" => yaml::decode(source, &mut budget, max_bytes),
        "toml" => {
            let document = source
                .parse::<toml_edit::DocumentMut>()
                .map_err(|_| DataDecodeError::InvalidData)?;
            toml_item(document.as_item(), &mut budget, 0)
        }
        _ => return Err(DataDecodeError::UnsupportedFormat),
    };
    if budget.exceeded {
        Err(DataDecodeError::ResourceLimit)
    } else {
        result
    }
}

struct Budget {
    remaining: usize,
    exceeded: bool,
}

impl Budget {
    fn charge(&mut self, bytes: usize) -> Result<(), DataDecodeError> {
        if let Some(remaining) = self.remaining.checked_sub(bytes) {
            self.remaining = remaining;
            Ok(())
        } else {
            self.exceeded = true;
            Err(DataDecodeError::ResourceLimit)
        }
    }

    fn value(&mut self, value: &impl Serialize) -> Result<(), DataDecodeError> {
        serde_json::to_writer(self, value).map_err(|_| DataDecodeError::ResourceLimit)
    }

    fn nesting(&mut self, depth: usize) -> Result<(), DataDecodeError> {
        if depth > MAX_DEPTH {
            self.exceeded = true;
            return Err(DataDecodeError::ResourceLimit);
        }
        Ok(())
    }
}

impl Write for Budget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.charge(bytes.len()).map_err(io::Error::other)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Seed<'a> {
    budget: &'a mut Budget,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Value;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        self.budget.nesting(self.depth).map_err(de::Error::custom)?;
        // RawValue preserves the numeric lexeme before serde_json can fall
        // back from an overflowing integer to an imprecise f64. It does not
        // change ordinary Value/Number representation across the workspace.
        let raw = <&serde_json::value::RawValue as serde::Deserialize>::deserialize(deserializer)?;
        literal_integer(raw.get())
            .transpose()
            .map_err(de::Error::custom)?;
        let mut decoder = serde_json::Deserializer::from_str(raw.get());
        serde::Deserializer::deserialize_any(&mut decoder, self).map_err(de::Error::custom)
    }
}

/// Recognize an integer literal, preserving its exact sign/radix and refusing
/// overflow. Non-integers are left to the format's established scalar decoder.
fn literal_integer(raw: &str) -> Option<Result<Number, DataDecodeError>> {
    let (negative, unsigned) = if let Some(value) = raw.strip_prefix('-') {
        (true, value)
    } else {
        (false, raw.strip_prefix('+').unwrap_or(raw))
    };
    let (radix, digits) = if let Some(value) = unsigned.strip_prefix("0x") {
        (16, value)
    } else if let Some(value) = unsigned.strip_prefix("0o") {
        (8, value)
    } else {
        (10, unsigned)
    };
    // YAML 1.2 core permits signs on decimal integers only. Signed radix
    // spellings are strings and must reach the YAML scalar resolver intact.
    if radix != 10 && raw.starts_with(['+', '-']) {
        return None;
    }
    if digits.is_empty() || !digits.chars().all(|value| value.is_digit(radix)) {
        return None;
    }
    Some((|| {
        let value = u64::from_str_radix(digits, radix).map_err(|_| DataDecodeError::InvalidData)?;
        if !negative {
            return Ok(value.into());
        }
        if value == 1_u64 << 63 {
            return Ok(i64::MIN.into());
        }
        i64::try_from(value)
            .map_err(|_| DataDecodeError::InvalidData)
            .map(|value| (-value).into())
    })())
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Value;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("bounded JSON-compatible data")
    }
    fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
        self.budget.charge(4).map_err(E::custom)?;
        Ok(Value::Null)
    }
    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
        self.budget.value(&value).map_err(E::custom)?;
        Ok(Value::Bool(value))
    }
    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
        self.budget.value(&value).map_err(E::custom)?;
        Ok(Value::Number(value.into()))
    }
    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
        self.budget.value(&value).map_err(E::custom)?;
        Ok(Value::Number(value.into()))
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
        let number = Number::from_f64(value).ok_or_else(|| E::custom("non-finite number"))?;
        self.budget.value(&number).map_err(E::custom)?;
        Ok(Value::Number(number))
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
        self.budget.value(&value).map_err(E::custom)?;
        Ok(Value::String(value.to_owned()))
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
        self.budget.value(&value).map_err(E::custom)?;
        Ok(Value::String(value))
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
        self.budget.charge(2).map_err(de::Error::custom)?;
        let mut output = Vec::new();
        while let Some(value) = sequence.next_element_seed(Seed {
            budget: self.budget,
            depth: self.depth + 1,
        })? {
            if !output.is_empty() {
                self.budget.charge(1).map_err(de::Error::custom)?;
            }
            output.push(value);
        }
        Ok(Value::Array(output))
    }
    fn visit_map<A: MapAccess<'de>>(self, mut mapping: A) -> Result<Value, A::Error> {
        self.budget.charge(2).map_err(de::Error::custom)?;
        let mut output = Map::new();
        while let Some(key) = mapping.next_key_seed(StringKey)? {
            if output.contains_key(&key) {
                return Err(de::Error::custom("duplicate key"));
            }
            self.budget.value(&key).map_err(de::Error::custom)?;
            self.budget
                .charge(if output.is_empty() { 1 } else { 2 })
                .map_err(de::Error::custom)?;
            let value = mapping.next_value_seed(Seed {
                budget: self.budget,
                depth: self.depth + 1,
            })?;
            output.insert(key, value);
        }
        Ok(Value::Object(output))
    }
}

struct StringKey;
impl<'de> DeserializeSeed<'de> for StringKey {
    type Value = String;
    fn deserialize<D: de::Deserializer<'de>>(self, deserializer: D) -> Result<String, D::Error> {
        deserializer.deserialize_any(self)
    }
}
impl Visitor<'_> for StringKey {
    type Value = String;
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string key")
    }
    fn visit_str<E: de::Error>(self, value: &str) -> Result<String, E> {
        Ok(value.to_owned())
    }
    fn visit_string<E: de::Error>(self, value: String) -> Result<String, E> {
        Ok(value)
    }
}

fn toml_item(
    item: &toml_edit::Item,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value, DataDecodeError> {
    budget.nesting(depth)?;
    match item {
        toml_edit::Item::Value(value) => toml_value(value, budget, depth),
        toml_edit::Item::Table(table) => toml_map(table.iter(), budget, depth),
        toml_edit::Item::ArrayOfTables(tables) => {
            budget.charge(2)?;
            let mut output = Vec::new();
            for table in tables.iter() {
                if !output.is_empty() {
                    budget.charge(1)?;
                }
                budget.nesting(depth + 1)?;
                output.push(toml_map(table.iter(), budget, depth + 1)?);
            }
            Ok(Value::Array(output))
        }
        toml_edit::Item::None => Err(DataDecodeError::InvalidData),
    }
}

fn toml_map<'a>(
    entries: impl Iterator<Item = (&'a str, &'a toml_edit::Item)>,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value, DataDecodeError> {
    budget.charge(2)?;
    let mut output = Map::new();
    for (key, value) in entries {
        budget.value(&key)?;
        budget.charge(if output.is_empty() { 1 } else { 2 })?;
        output.insert(key.to_owned(), toml_item(value, budget, depth + 1)?);
    }
    Ok(Value::Object(output))
}

fn toml_value(
    value: &toml_edit::Value,
    budget: &mut Budget,
    depth: usize,
) -> Result<Value, DataDecodeError> {
    budget.nesting(depth)?;
    match value {
        toml_edit::Value::String(value) => {
            budget.value(value.value())?;
            Ok(Value::String(value.value().clone()))
        }
        toml_edit::Value::Integer(value) => {
            budget.value(value.value())?;
            Ok(Value::Number((*value.value()).into()))
        }
        toml_edit::Value::Float(value) => {
            let number = Number::from_f64(*value.value()).ok_or(DataDecodeError::InvalidData)?;
            budget.value(&number)?;
            Ok(Value::Number(number))
        }
        toml_edit::Value::Boolean(value) => {
            budget.value(value.value())?;
            Ok(Value::Bool(*value.value()))
        }
        toml_edit::Value::Datetime(value) => {
            let text = value.value().to_string();
            budget.value(&text)?;
            Ok(Value::String(text))
        }
        toml_edit::Value::Array(values) => {
            budget.charge(2)?;
            let mut output = Vec::new();
            for value in values.iter() {
                if !output.is_empty() {
                    budget.charge(1)?;
                }
                output.push(toml_value(value, budget, depth + 1)?);
            }
            Ok(Value::Array(output))
        }
        toml_edit::Value::InlineTable(table) => {
            budget.charge(2)?;
            let mut output = Map::new();
            for (key, value) in table.iter() {
                budget.value(&key)?;
                budget.charge(if output.is_empty() { 1 } else { 2 })?;
                output.insert(key.to_owned(), toml_value(value, budget, depth + 1)?);
            }
            Ok(Value::Object(output))
        }
    }
}

#[cfg(test)]
mod tests;
