//! Decode YAML events before tags, scalar spellings or merge syntax are lost.
//! The established parser/scalar resolver owns YAML syntax; this module owns
//! only the bounded, JSON-compatible projection.

use std::collections::BTreeMap;

use saphyr::Scalar;
use saphyr_parser::{Event, Parser, ScalarStyle, StrInput};
use serde_json::{Map, Number, Value};

use super::{Budget, DataDecodeError, literal_integer};

struct Node {
    value: Value,
    height: usize,
}

struct Decoder<'input, 'budget> {
    parser: Parser<'input, StrInput<'input>>,
    budget: &'budget mut Budget,
    anchors: BTreeMap<usize, Node>,
    // Cached anchors may overlap. Bound their retained copies separately by
    // the same invocation byte budget, before cloning any anchored subtree.
    anchor_budget: Budget,
}

pub(super) fn decode(
    source: &str,
    budget: &mut Budget,
    max_bytes: usize,
) -> Result<Value, DataDecodeError> {
    let mut decoder = Decoder {
        parser: Parser::new_from_str(source),
        budget,
        anchors: BTreeMap::new(),
        anchor_budget: Budget {
            remaining: max_bytes,
            exceeded: false,
        },
    };
    if decoder.next()? != Event::StreamStart || !matches!(decoder.next()?, Event::DocumentStart(_))
    {
        return Err(DataDecodeError::InvalidData);
    }
    let event = decoder.next()?;
    let node = decoder.node(event, 0)?;
    if decoder.next()? != Event::DocumentEnd || decoder.next()? != Event::StreamEnd {
        return Err(DataDecodeError::InvalidData);
    }
    Ok(node.value)
}

impl<'input> Decoder<'input, '_> {
    fn next(&mut self) -> Result<Event<'input>, DataDecodeError> {
        self.parser
            .next_event()
            .ok_or(DataDecodeError::InvalidData)?
            .map(|(event, _)| event)
            .map_err(|_| DataDecodeError::InvalidData)
    }

    fn node(&mut self, event: Event<'input>, depth: usize) -> Result<Node, DataDecodeError> {
        self.budget.nesting(depth)?;
        let (node, anchor) = match event {
            Event::Scalar(value, style, anchor, tag) => {
                if tag.is_some() {
                    return Err(DataDecodeError::InvalidData);
                }
                let value = scalar(value, style)?;
                self.budget.value(&value)?;
                (Node { value, height: 0 }, anchor)
            }
            Event::SequenceStart(anchor, tag) => {
                if tag.is_some() {
                    return Err(DataDecodeError::InvalidData);
                }
                self.budget.charge(2)?;
                let mut values = Vec::new();
                let mut height = 0;
                loop {
                    let event = self.next()?;
                    if event == Event::SequenceEnd {
                        break;
                    }
                    if !values.is_empty() {
                        self.budget.charge(1)?;
                    }
                    let child = self.node(event, depth + 1)?;
                    height = height.max(child.height + 1);
                    values.push(child.value);
                }
                (
                    Node {
                        value: Value::Array(values),
                        height,
                    },
                    anchor,
                )
            }
            Event::MappingStart(anchor, tag) => {
                if tag.is_some() {
                    return Err(DataDecodeError::InvalidData);
                }
                self.budget.charge(2)?;
                let mut fields = Map::new();
                let mut height = 0;
                loop {
                    let event = self.next()?;
                    if event == Event::MappingEnd {
                        break;
                    }
                    if matches!(&event, Event::Scalar(value, ScalarStyle::Plain, _, _) if value == "<<")
                    {
                        return Err(DataDecodeError::InvalidData);
                    }
                    // Key scalars are charged by node; punctuation is charged
                    // here, so aliases and escaped keys count exact JSON bytes.
                    let key = self.node(event, depth + 1)?;
                    let Value::String(key) = key.value else {
                        return Err(DataDecodeError::InvalidData);
                    };
                    if fields.contains_key(&key) {
                        return Err(DataDecodeError::InvalidData);
                    }
                    self.budget.charge(if fields.is_empty() { 1 } else { 2 })?;
                    let event = self.next()?;
                    let child = self.node(event, depth + 1)?;
                    height = height.max(child.height + 1);
                    fields.insert(key, child.value);
                }
                (
                    Node {
                        value: Value::Object(fields),
                        height,
                    },
                    anchor,
                )
            }
            Event::Alias(anchor) => {
                // Forward and cyclic references cannot become JSON values.
                let source = self
                    .anchors
                    .get(&anchor)
                    .ok_or(DataDecodeError::InvalidData)?;
                self.budget.nesting(depth + source.height)?;
                self.budget.value(&source.value)?;
                return Ok(Node {
                    value: source.value.clone(),
                    height: source.height,
                });
            }
            _ => return Err(DataDecodeError::InvalidData),
        };
        if anchor != 0 {
            self.anchor_budget.value(&node.value)?;
            self.anchors.insert(
                anchor,
                Node {
                    value: node.value.clone(),
                    height: node.height,
                },
            );
        }
        Ok(node)
    }
}

fn scalar(value: std::borrow::Cow<'_, str>, style: ScalarStyle) -> Result<Value, DataDecodeError> {
    if style == ScalarStyle::Plain {
        if let Some(integer) = literal_integer(&value) {
            return integer.map(Value::Number);
        }
        // The resolver may classify an overflowing float as a string. Its raw
        // spelling is still available here, so range failure cannot change type.
        if let Ok(number) = value.parse::<f64>() {
            let numeric_start = value
                .trim_start_matches(['+', '-'])
                .starts_with(|c: char| c.is_ascii_digit() || c == '.');
            if numeric_start && !number.is_finite() {
                return Err(DataDecodeError::InvalidData);
            }
        }
    }
    let resolved = Scalar::parse_from_cow_and_metadata(value.clone(), style, None)
        .ok_or(DataDecodeError::InvalidData)?;
    match resolved {
        Scalar::Null => Ok(Value::Null),
        Scalar::Boolean(value) => Ok(Value::Bool(value)),
        // Valid YAML integer spellings returned above. The resolver accepts
        // Rust-only forms such as 0x-10 and ++12; those are YAML strings.
        Scalar::Integer(_) => Ok(Value::String(value.into_owned())),
        Scalar::FloatingPoint(value) => Number::from_f64(value.0)
            .map(Value::Number)
            .ok_or(DataDecodeError::InvalidData),
        Scalar::String(value) => Ok(Value::String(value.into_owned())),
    }
}
