//! The values a skill author writes: a literal, the run's input, or an
//! earlier step's result.
//!
//! A value is a literal unless it is a string naming a reference:
//!
//! - `$input` or `$input.<path>`: the run's input, or a field of it;
//! - `$steps.<id>` or `$steps.<id>.<path>`: an earlier step's result, or a
//!   field of it;
//! - `$act` or `$act.<field>`: the act the run's invoker states, read only
//!   inside an `act:`. Domain values never read accountability metadata.
//! - `$run.id`: the run's own id, read only by an operation step's
//!   `idempotency_key`, so a write keyed on it happens once per run. Domain
//!   values never read the run's identity.
//!
//! Paths are dotted; a numeric segment indexes a list. A literal string that
//! begins with `$` is written `$$`, everywhere a value is written, including
//! the literal a condition compares with. References may sit at any depth of
//! an object or list. Parsing turns them into typed bindings once, so a bound
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
    /// The invoker's act, or the field at `path`.
    Act { path: Vec<String> },
    /// The run's own id.
    RunId,
}

impl Source {
    /// The step a value is read from, if any.
    #[must_use]
    pub fn step(&self) -> Option<&str> {
        match self {
            Self::Step { step, .. } => Some(step),
            Self::Input { .. } | Self::Act { .. } | Self::RunId => None,
        }
    }

    /// The reference as an author writes it.
    #[must_use]
    pub fn reference(&self) -> String {
        let (head, path) = match self {
            Self::Input { path } => ("$input".to_owned(), path),
            Self::Step { step, path } => (format!("$steps.{step}"), path),
            Self::Act { path } => ("$act".to_owned(), path),
            Self::RunId => return "$run.id".to_owned(),
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

/// Parse an operation step's call fields: the literal template, with each
/// reference left as written, and the bindings that fill it. Only its
/// `idempotency_key` may read `$run.id`.
pub(crate) fn parse_call(
    call: &JsonObject,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
    object(call, previous_steps, field, |key| {
        if key == "idempotency_key" {
            Reading::Key
        } else {
            Reading::Values
        }
    })
}

/// Parse a whole value that may itself be one reference, such as
/// `inputs: $input`.
pub(crate) fn parse_whole(
    value: &JsonValue,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
    Reading::Values.whole(value, previous_steps, field)
}

/// Parse an `act:`: an object of act fields, or a reference to a whole act
/// such as `act: $act`. Only here may a value read `$act`.
pub(crate) fn parse_act(
    value: &JsonValue,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
    Reading::Act.whole(value, previous_steps, field)
}

/// Parse the value a condition or a guard reads: always a reference, since a
/// literal would decide the same way on every run.
pub(crate) fn parse_operand(
    value: &JsonValue,
    previous_steps: &BTreeSet<String>,
    field: &str,
) -> Result<Operand, ValidationError> {
    let source = match value {
        JsonValue::String(text) => Reading::Values.reference(text, previous_steps, field)?,
        _ => None,
    };
    let field_of = |source: &Source| match source {
        Source::Input { path } | Source::Act { path } | Source::Step { path, .. } => {
            !path.is_empty()
        }
        Source::RunId => false,
    };
    source
        .filter(field_of)
        .map(Operand::Reference)
        .ok_or_else(|| {
            validation_error(format!(
                "{field} reads one field of the run's input or an earlier step's result: write $input.<path> or $steps.<id>.<path>; a literal decides nothing."
            ))
        })
}

/// Parse the literal a condition or a guard compares with. It reads nothing,
/// so a string in it that begins with `$` is written `$$`.
pub(crate) fn parse_literal(value: &JsonValue, field: &str) -> Result<JsonValue, ValidationError> {
    Ok(match value {
        JsonValue::String(text) if text.starts_with('$') && !text.starts_with("$$") => {
            return Err(validation_error(format!(
                "{field} is a literal compared with the value read; write a literal that begins with $ as $$."
            )));
        }
        JsonValue::String(text) => JsonValue::String(unescape(text).to_owned()),
        JsonValue::Object(object) => JsonValue::Object(
            object
                .iter()
                .map(|(key, value)| {
                    Ok((
                        key.clone(),
                        parse_literal(value, &format!("{field}.{key}"))?,
                    ))
                })
                .collect::<Result<_, ValidationError>>()?,
        ),
        JsonValue::Array(items) => JsonValue::Array(
            items
                .iter()
                .enumerate()
                .map(|(index, item)| parse_literal(item, &format!("{field}.{index}")))
                .collect::<Result<_, _>>()?,
        ),
        literal => literal.clone(),
    })
}

/// What a value may read: the run's input and earlier steps everywhere, the
/// invoker's act only inside an `act:`, and the run's id only in an
/// operation's `idempotency_key`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reading {
    Values,
    Act,
    Key,
}

/// Parse an object whose members each read what `reading` allows for their
/// key.
fn object(
    values: &JsonObject,
    previous_steps: &BTreeSet<String>,
    field: &str,
    reading: impl Fn(&str) -> Reading,
) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
    let mut bindings = Vec::new();
    let mut template = JsonObject::new();
    for (key, value) in values {
        let mut at = vec![key.clone()];
        let literal = reading(key).value(
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

impl Reading {
    fn object(
        self,
        values: &JsonObject,
        previous_steps: &BTreeSet<String>,
        field: &str,
    ) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
        object(values, previous_steps, field, |_| self)
    }

    fn whole(
        self,
        value: &JsonValue,
        previous_steps: &BTreeSet<String>,
        field: &str,
    ) -> Result<(JsonObject, Vec<Binding>), ValidationError> {
        let refused = || {
            validation_error(format!(
                "{field} must be an object of values or a reference to one."
            ))
        };
        match value {
            JsonValue::Object(values) => self.object(values, previous_steps, field),
            JsonValue::String(text) => match self.reference(text, previous_steps, field)? {
                Some(source) => Ok((
                    JsonObject::new(),
                    vec![Binding {
                        at: Vec::new(),
                        source,
                    }],
                )),
                None => Err(refused()),
            },
            _ => Err(refused()),
        }
    }

    fn value(
        self,
        value: &JsonValue,
        at: &mut Vec<String>,
        previous_steps: &BTreeSet<String>,
        field: &str,
        bindings: &mut Vec<Binding>,
    ) -> Result<JsonValue, ValidationError> {
        Ok(match value {
            JsonValue::String(text) => match self.reference(text, previous_steps, field)? {
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
                        self.value(
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
                    parsed.push(self.value(
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

    /// The reference `text` names, if it names one this value may read.
    fn reference(
        self,
        text: &str,
        previous_steps: &BTreeSet<String>,
        field: &str,
    ) -> Result<Option<Source>, ValidationError> {
        match reference(text, previous_steps, field)? {
            Some(Source::Act { .. }) if self != Self::Act => Err(validation_error(format!(
                "{field} reads $act, the invoker's act, which only an act: may read."
            ))),
            Some(Source::RunId) if self != Self::Key => Err(validation_error(format!(
                "{field} reads $run.id, the run's own id, which only an operation's idempotency_key may read."
            ))),
            source => Ok(source),
        }
    }
}

/// The reference `text` names, if it names one, wherever it is written.
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
        "$act" => Ok(Some(Source::Act { path: rest })),
        "$run" => match rest.as_slice() {
            [id] if id == "id" => Ok(Some(Source::RunId)),
            _ => Err(validation_error(format!(
                "{field} reference {text:?} reads the run's id: write $run.id."
            ))),
        },
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
            "{field} {text:?} is not a reference: write $input.<path>, $steps.<id>.<path>, $act.<field> in an act:, or $run.id in an idempotency_key, and a literal that begins with $ as $$."
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
