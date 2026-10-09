//! The values a skill author writes: a literal, the run's input, or an
//! earlier step's result.
//!
//! A value is a literal unless it is a string naming a reference:
//!
//! - `$input` or `$input.<path>`: the run's input, or a field of it;
//! - `$steps.<id>` or `$steps.<id>.<path>`: an earlier step's result, or a
//!   field of it.
//!
//! Paths are dotted; a numeric segment indexes a list. A literal string that
//! begins with `$` is written `$$`. References may sit at any depth of an
//! object or list. Parsing turns them into typed bindings once, so a bound
//! value is never read again as a reference.

use std::collections::BTreeSet;

use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use super::helpers::validation_error;
use crate::ValidationError;

/// Where a bound value comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case", deny_unknown_fields)]
pub enum Source {
    /// The run's input, or the field at `path`.
    Input { path: Vec<String> },
    /// An earlier step's result, or the field at `path`.
    Step { step: String, path: Vec<String> },
}

impl Source {
    /// The step a value is read from, if any.
    #[must_use]
    pub fn step(&self) -> Option<&str> {
        match self {
            Self::Step { step, .. } => Some(step),
            Self::Input { .. } => None,
        }
    }

    /// The reference as an author writes it.
    #[must_use]
    pub fn reference(&self) -> String {
        let (head, path) = match self {
            Self::Input { path } => ("$input".to_owned(), path),
            Self::Step { step, path } => (format!("$steps.{step}"), path),
        };
        std::iter::once(head)
            .chain(path.iter().cloned())
            .collect::<Vec<_>>()
            .join(".")
    }
}

/// One reference inside a step's values: where it sits, and what it reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// The path to the value inside the step's values: object keys, and list
    /// indexes as decimal strings. Empty for a reference that is the whole.
    pub at: Vec<String>,
    pub source: Source,
}

/// A value read once, by a condition or a guard.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Operand {
    Literal(JsonValue),
    Reference(Source),
}

impl Operand {
    #[must_use]
    pub fn step(&self) -> Option<&str> {
        match self {
            Self::Reference(source) => source.step(),
            Self::Literal(_) => None,
        }
    }
}

/// Parse an object of values: the literal template, with each reference
/// left as written, and the bindings that fill it.
pub(crate) fn parse_values(
    values: &JsonObject,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
    let mut bindings = Vec::new();
    let mut template = JsonObject::new();
    for (key, value) in values {
        let mut at = vec![key.clone()];
        let literal = parse_value(
            value,
            &mut at,
            previous_steps,
            &format!("{field}.{key}"),
            &mut bindings,
        )?;
        template.insert(key.clone(), literal);
    }
    Ok((template, bindings))
}

/// Parse a whole value that may itself be one reference, such as
/// `inputs: $input`.
pub(crate) fn parse_whole(
    value: &JsonValue,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
    match value {
        JsonValue::Object(values) => parse_values(values, previous_steps, field),
        JsonValue::String(text) => match reference(text, previous_steps, field)? {
            Some(source) => Ok((
                JsonObject::new(),
                vec![Binding {
                    at: Vec::new(),
                    source,
                }],
            )),
            None => Err(validation_error(format!(
                "{field} must be an object of values or a reference to one."
            ))),
        },
        _ => Err(validation_error(format!(
            "{field} must be an object of values or a reference to one."
        ))),
    }
}

/// Parse one value read by a condition or a guard.
pub(crate) fn parse_operand(
    value: &JsonValue,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<Operand, ValidationError> {
    match value {
        JsonValue::String(text) => Ok(match reference(text, previous_steps, field)? {
            Some(source) => Operand::Reference(source),
            None => Operand::Literal(JsonValue::String(unescape(text).to_owned())),
        }),
        literal => Ok(Operand::Literal(literal.clone())),
    }
}

fn parse_value(
    value: &JsonValue,
    at: &mut Vec<String>,
    previous_steps: &BTreeSet<String>,
    field: &str,
    bindings: &mut Vec<Binding>,
) -> Result<JsonValue, ValidationError> {
    Ok(match value {
        JsonValue::String(text) => match reference(text, previous_steps, field)? {
            Some(source) => {
                bindings.push(Binding {
                    at: at.clone(),
                    source,
                });
                JsonValue::String(text.clone())
            }
            None => JsonValue::String(unescape(text).to_owned()),
        },
        JsonValue::Object(object) => {
            let mut parsed = JsonObject::new();
            for (key, value) in object {
                at.push(key.clone());
                parsed.insert(
                    key.clone(),
                    parse_value(
                        value,
                        at,
                        previous_steps,
                        &format!("{field}.{key}"),
                        bindings,
                    )?,
                );
                at.pop();
            }
            JsonValue::Object(parsed)
        }
        JsonValue::Array(items) => {
            let mut parsed = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                at.push(index.to_string());
                parsed.push(parse_value(
                    item,
                    at,
                    previous_steps,
                    &format!("{field}.{index}"),
                    bindings,
                )?);
                at.pop();
            }
            JsonValue::Array(parsed)
        }
        literal => literal.clone(),
    })
}

/// The reference `text` names, if it names one.
fn reference(
    text: &str,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<Option<Source>, ValidationError> {
    if !text.starts_with('$') || text.starts_with("$$") {
        return Ok(None);
    }
    let mut segments = text.split('.');
    let head = segments.next().unwrap_or_default();
    let rest = segments.map(str::to_owned).collect::<Vec<_>>();
    if rest.iter().any(String::is_empty) {
        return Err(validation_error(format!(
            "{field} reference {text:?} has an empty path segment."
        )));
    }
    match head {
        "$input" => Ok(Some(Source::Input { path: rest })),
        "$steps" => {
            let Some((step, path)) = rest.split_first() else {
                return Err(validation_error(format!(
                    "{field} reference {text:?} must name a step: $steps.<id>."
                )));
            };
            if !previous_steps.contains(step) {
                return Err(validation_error(format!(
                    "{field} references unknown or later step '{step}'."
                )));
            }
            Ok(Some(Source::Step {
                step: step.clone(),
                path: path.to_vec(),
            }))
        }
        _ => Err(validation_error(format!(
            "{field} {text:?} is not a reference: write $input.<path> or $steps.<id>.<path>, and a literal that begins with $ as $$."
        ))),
    }
}

fn unescape(text: &str) -> &str {
    text.strip_prefix('$')
        .filter(|rest| rest.starts_with('$'))
        .unwrap_or(text)
}

#[cfg(test)]
mod tests;
