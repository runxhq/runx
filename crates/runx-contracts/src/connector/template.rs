//! Templates: JSON values that bind parts of a call into a target or a
//! provider request.
//!
//! A single-key object is a binding:
//! - `{"$argument": "/pointer"}`: a value from the call input.
//! - `{"$parameter": "/pointer"}`: a value from the connector instance.
//! - `{"$response": "<request id>.<path>"}`: a value from an earlier response
//!   in the same exchange (readbacks reading what a write returned).
//! - `{"$item": "/pointer"}`: a value from the element a request is sent for,
//!   when the request repeats over an input array (`each`), or an effect is
//!   rendered for, when it repeats over a result array.
//! - `{"$result": "/pointer"}`: a value from the call's decoded result, which
//!   only an operation's effect reads.
//! - `{"$runx": "idempotency_key"}`: the key the kernel derived for the call.
//! - `{"$runx": "purchase"}`: the purchase a hosted call runs under.
//!   Only runx Connect supplies it, from the run its gateway session
//!   authenticates; the kernel never does, so anywhere else it is unavailable
//!   and the call fails closed.
//! - `{"$concat": [part, ...]}`: one to eight scalar parts joined into a string.
//! - `{"$minor_units": {"decimal": part, "scale": part}}`: an exact
//!   canonical decimal string converted to integer smallest units, with no
//!   rounding, float arithmetic or currency inference.
//!
//! Everything else is literal. Structure is validated on deserialization, so a
//! malformed binding never reaches a manifest; rendering is bounded in depth
//! and output size.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Deserializer, Serialize, de};
use serde_json::{Value, json};
use thiserror::Error;

use crate::schema::RunxSchema;
use crate::{JsonObject, JsonValue};

const MAX_DEPTH: usize = 32;
const MAX_CONCAT_PARTS: usize = 8;
const MAX_CONCAT_BYTES: usize = 512;

const ARGUMENT: &str = "$argument";
const PARAMETER: &str = "$parameter";
const RESPONSE: &str = "$response";
const ITEM: &str = "$item";
const RESULT: &str = "$result";
const RUNX: &str = "$runx";
const IDEMPOTENCY_KEY: &str = "idempotency_key";
const PURCHASE: &str = "purchase";
const CONCAT: &str = "$concat";
const MINOR_UNITS: &str = "$minor_units";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Template(JsonValue);

// A template is only ever deserialized from JSON or YAML, where no value is
// unequal to itself, so equality is total.
impl Eq for Template {}

/// What a template may bind. Absent sources make their bindings fail.
#[derive(Clone, Copy, Debug, Default)]
pub struct TemplateInputs<'a> {
    pub arguments: Option<&'a JsonValue>,
    pub parameters: Option<&'a JsonObject>,
    pub responses: Option<&'a BTreeMap<String, JsonObject>>,
    /// The element a repeated request is sent for, or an effect rendered
    /// for.
    pub item: Option<&'a JsonValue>,
    /// The call's decoded result, once it has one.
    pub result: Option<&'a JsonValue>,
    pub idempotency_key: Option<&'a str>,
    /// The purchase the call runs under, which only runx Connect
    /// supplies.
    pub purchase_id: Option<&'a str>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("template binding {binding} is malformed: {reason}")]
    Malformed {
        binding: &'static str,
        reason: &'static str,
    },
    #[error("template nests deeper than {MAX_DEPTH} levels")]
    TooDeep,
    #[error("template binding {binding} {reference:?} is unavailable")]
    Unavailable {
        binding: &'static str,
        reference: String,
    },
    #[error("template $concat part must be a string or number")]
    ConcatPart,
    #[error("template $concat exceeds {MAX_CONCAT_BYTES} bytes")]
    ConcatTooLong,
    #[error(
        "template $minor_units needs a canonical decimal, scale 0 to 18, and a portable integer result"
    )]
    MinorUnits,
    #[error("template rendered a {found} where a string was required")]
    NotAString { found: &'static str },
}

impl Template {
    /// The literal object the template declares at `path`, reached through
    /// literal objects alone; none when a binding or any other value stands
    /// on the way, since what it renders is not known until the call.
    #[must_use]
    pub fn literal_object(&self, path: &[String]) -> Option<&JsonObject> {
        fn literal(value: &JsonValue) -> Option<&JsonObject> {
            match value {
                JsonValue::Object(object) if binding(object).is_none() => Some(object),
                _ => None,
            }
        }
        let mut object = literal(&self.0)?;
        for segment in path {
            object = literal(object.get(segment)?)?;
        }
        Some(object)
    }

    /// Render against the call. Bindings are replaced; literals pass through.
    pub fn render(&self, inputs: &TemplateInputs<'_>) -> Result<JsonValue, TemplateError> {
        render(&self.0, inputs, 0)
    }

    /// Render and require a string, as targets and URLs do.
    pub fn render_string(&self, inputs: &TemplateInputs<'_>) -> Result<String, TemplateError> {
        match self.render(inputs)? {
            JsonValue::String(value) => Ok(value),
            other => Err(TemplateError::NotAString {
                found: kind(&other),
            }),
        }
    }

    /// The argument pointer when the whole template is one `$argument`
    /// binding, as a repeated request's `each` is.
    #[must_use]
    pub fn argument_pointer(&self) -> Option<&str> {
        match &self.0 {
            JsonValue::Object(object) => match binding(object) {
                Some((ARGUMENT, JsonValue::String(pointer))) => Some(pointer),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether the template binds `$item`, which only a repeated request has.
    #[must_use]
    pub fn binds_item(&self) -> bool {
        binds(&self.0, ITEM)
    }

    /// Whether the template binds `$response` or `$item`, which only the
    /// requests of one exchange have.
    #[must_use]
    pub fn binds_exchange(&self) -> bool {
        binds(&self.0, RESPONSE) || binds(&self.0, ITEM)
    }

    /// Whether the template sends the key the kernel derived for the call.
    #[must_use]
    pub fn binds_idempotency_key(&self) -> bool {
        binds_runx_value(&self.0, IDEMPOTENCY_KEY)
    }

    /// Whether the template sends the purchase the call runs under.
    #[must_use]
    pub fn binds_purchase(&self) -> bool {
        binds_runx_value(&self.0, PURCHASE)
    }

    /// Whether the template binds `$result`, which only an effect reads.
    #[must_use]
    pub fn binds_result(&self) -> bool {
        binds(&self.0, RESULT)
    }

    /// The result pointer when the whole template is one `$result` binding,
    /// as an effect's `each` is.
    #[must_use]
    pub fn result_pointer(&self) -> Option<&str> {
        match &self.0 {
            JsonValue::Object(object) => match binding(object) {
                Some((RESULT, JsonValue::String(pointer))) => Some(pointer),
                _ => None,
            },
            _ => None,
        }
    }

    /// Whether the template binds `$parameter`, a value of the connector
    /// instance.
    #[must_use]
    pub fn binds_parameter(&self) -> bool {
        binds(&self.0, PARAMETER)
    }

    /// Whether the template binds `$argument`, a value of the call input.
    #[must_use]
    pub fn binds_argument(&self) -> bool {
        binds(&self.0, ARGUMENT)
    }

    /// The `$argument` pointers the template reads.
    #[must_use]
    pub fn argument_pointers(&self) -> BTreeSet<&str> {
        let mut pointers = BTreeSet::new();
        argument_pointers(&self.0, &mut pointers);
        pointers
    }

    /// Whether the template binds anything runx fills (`$runx`).
    #[must_use]
    pub fn binds_runx(&self) -> bool {
        binds(&self.0, RUNX)
    }

    /// The `<request id>.<path>` reference when the whole template is one
    /// `$response` binding, as a held field is.
    #[must_use]
    pub fn response_reference(&self) -> Option<&str> {
        match &self.0 {
            JsonValue::Object(object) => match binding(object) {
                Some((RESPONSE, JsonValue::String(reference))) => Some(reference),
                _ => None,
            },
            _ => None,
        }
    }

    /// The request ids its `$response` bindings read from.
    #[must_use]
    pub fn responses(&self) -> BTreeSet<&str> {
        let mut ids = BTreeSet::new();
        response_ids(&self.0, &mut ids);
        ids
    }

    /// Whether the template binds nothing: it renders the same value for
    /// every call.
    #[must_use]
    pub fn is_literal(&self) -> bool {
        !contains_binding(&self.0)
    }

    /// Top-level object fields runx itself fills, with the call's provider
    /// key or its purchase, which no input reaches.
    #[must_use]
    pub fn runx_fields(&self) -> BTreeSet<&str> {
        match &self.0 {
            JsonValue::Object(fields) if binding(fields).is_none() => fields
                .iter()
                .filter(|(_, value)| {
                    matches!(value, JsonValue::Object(object) if binding(object).is_some_and(|(key, _)| key == RUNX))
                })
                .map(|(key, _)| key.as_str())
                .collect(),
            _ => BTreeSet::new(),
        }
    }

    /// Top-level object fields whose values are literals, for the multiplexed
    /// MCP tool rule: a discriminating argument must be literal.
    #[must_use]
    pub fn literal_fields(&self) -> BTreeMap<&str, &JsonValue> {
        match &self.0 {
            JsonValue::Object(fields) if binding(fields).is_none() => fields
                .iter()
                .filter(|(_, value)| !contains_binding(value))
                .map(|(key, value)| (key.as_str(), value))
                .collect(),
            _ => BTreeMap::new(),
        }
    }
}

impl<'de> Deserialize<'de> for Template {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = JsonValue::deserialize(deserializer)?;
        validate(&value, 0).map_err(de::Error::custom)?;
        Ok(Self(value))
    }
}

impl RunxSchema for Template {
    fn json_schema() -> Value {
        json!({})
    }
}

fn binding(object: &JsonObject) -> Option<(&str, &JsonValue)> {
    if object.len() != 1 {
        return None;
    }
    let (key, value) = object.iter().next()?;
    matches!(
        key.as_str(),
        ARGUMENT | PARAMETER | RESPONSE | ITEM | RESULT | RUNX | CONCAT | MINOR_UNITS
    )
    .then_some((key.as_str(), value))
}

fn validate(value: &JsonValue, depth: usize) -> Result<(), TemplateError> {
    if depth > MAX_DEPTH {
        return Err(TemplateError::TooDeep);
    }
    match value {
        JsonValue::Object(object) => {
            if let Some((key, argument)) = binding(object) {
                return validate_binding(key, argument, depth);
            }
            if object.keys().any(|key| key.starts_with('$')) {
                return Err(TemplateError::Malformed {
                    binding: "$",
                    reason: "a $ key must be the only key of a binding object",
                });
            }
            object
                .values()
                .try_for_each(|value| validate(value, depth + 1))
        }
        JsonValue::Array(values) => values
            .iter()
            .try_for_each(|value| validate(value, depth + 1)),
        _ => Ok(()),
    }
}

fn validate_binding(key: &str, argument: &JsonValue, depth: usize) -> Result<(), TemplateError> {
    match key {
        ARGUMENT | PARAMETER | ITEM | RESULT => match argument.as_str() {
            Some(pointer) if pointer.is_empty() || pointer.starts_with('/') => Ok(()),
            _ => Err(TemplateError::Malformed {
                binding: static_key(key),
                reason: "expected a JSON pointer",
            }),
        },
        RESPONSE => match argument.as_str() {
            Some(reference)
                if reference
                    .split_once('.')
                    .is_some_and(|(id, path)| !id.is_empty() && !path.is_empty()) =>
            {
                Ok(())
            }
            _ => Err(TemplateError::Malformed {
                binding: RESPONSE,
                reason: "expected <request id>.<path>",
            }),
        },
        RUNX => match argument.as_str() {
            Some(IDEMPOTENCY_KEY | PURCHASE) => Ok(()),
            _ => Err(TemplateError::Malformed {
                binding: RUNX,
                reason: "the runtime values are idempotency_key and purchase",
            }),
        },
        MINOR_UNITS => {
            let Some(fields) = argument.as_object().filter(|fields| {
                fields.len() == 2 && fields.contains_key("decimal") && fields.contains_key("scale")
            }) else {
                return Err(TemplateError::Malformed {
                    binding: MINOR_UNITS,
                    reason: "expected decimal and scale templates",
                });
            };
            fields
                .values()
                .try_for_each(|value| validate(value, depth + 1))
        }
        _ => match argument.as_array() {
            Some(parts) if (1..=MAX_CONCAT_PARTS).contains(&parts.len()) => {
                parts.iter().try_for_each(|part| validate(part, depth + 1))
            }
            _ => Err(TemplateError::Malformed {
                binding: CONCAT,
                reason: "expected one to eight parts",
            }),
        },
    }
}

fn render(
    value: &JsonValue,
    inputs: &TemplateInputs<'_>,
    depth: usize,
) -> Result<JsonValue, TemplateError> {
    if depth > MAX_DEPTH {
        return Err(TemplateError::TooDeep);
    }
    match value {
        JsonValue::Object(object) => {
            if let Some((key, argument)) = binding(object) {
                return render_binding(key, argument, inputs, depth);
            }
            // A member that is an argument the call does not carry is
            // omitted, as an optional field is; the input schema decides
            // which arguments a call must carry.
            let mut rendered = JsonObject::new();
            for (key, value) in object {
                match render(value, inputs, depth + 1) {
                    Ok(value) => {
                        rendered.insert(key.clone(), value);
                    }
                    Err(TemplateError::Unavailable {
                        binding: ARGUMENT, ..
                    }) if is_argument(value) => {}
                    Err(error) => return Err(error),
                }
            }
            Ok(JsonValue::Object(rendered))
        }
        JsonValue::Array(values) => values
            .iter()
            .map(|value| render(value, inputs, depth + 1))
            .collect::<Result<Vec<_>, _>>()
            .map(JsonValue::Array),
        literal => Ok(literal.clone()),
    }
}

fn is_argument(value: &JsonValue) -> bool {
    matches!(value, JsonValue::Object(object) if binding(object).is_some_and(|(key, _)| key == ARGUMENT))
}

fn render_binding(
    key: &str,
    argument: &JsonValue,
    inputs: &TemplateInputs<'_>,
    depth: usize,
) -> Result<JsonValue, TemplateError> {
    let reference = argument.as_str().unwrap_or_default();
    let unavailable = || TemplateError::Unavailable {
        binding: static_key(key),
        reference: reference.to_owned(),
    };
    match key {
        ARGUMENT => inputs
            .arguments
            .and_then(|arguments| pointer(arguments, reference))
            .cloned()
            .ok_or_else(unavailable),
        PARAMETER => inputs
            .parameters
            .and_then(|parameters| object_pointer(parameters, reference))
            .cloned()
            .ok_or_else(unavailable),
        RESPONSE => inputs
            .responses
            .and_then(|responses| response(responses, reference))
            .cloned()
            .ok_or_else(unavailable),
        ITEM => inputs
            .item
            .and_then(|item| pointer(item, reference))
            .cloned()
            .ok_or_else(unavailable),
        RESULT => inputs
            .result
            .and_then(|result| pointer(result, reference))
            .cloned()
            .ok_or_else(unavailable),
        RUNX => match reference {
            PURCHASE => inputs.purchase_id,
            _ => inputs.idempotency_key,
        }
        .map(|value| JsonValue::String(value.to_owned()))
        .ok_or_else(unavailable),
        MINOR_UNITS => {
            let fields = argument.as_object().ok_or(TemplateError::MinorUnits)?;
            let decimal = render(
                fields.get("decimal").ok_or(TemplateError::MinorUnits)?,
                inputs,
                depth + 1,
            )?;
            let scale = render(
                fields.get("scale").ok_or(TemplateError::MinorUnits)?,
                inputs,
                depth + 1,
            )?;
            let units = decimal
                .as_str()
                .zip(scale.as_u64())
                .and_then(|(value, scale)| decimal_units(value, scale))
                .ok_or(TemplateError::MinorUnits)?;
            Ok(JsonValue::Number(crate::JsonNumber::from(units)))
        }
        _ => {
            let mut joined = String::new();
            for part in argument.as_array().into_iter().flatten() {
                match render(part, inputs, depth + 1)? {
                    JsonValue::String(value) => joined.push_str(&value),
                    JsonValue::Number(value) => joined.push_str(&value.to_string()),
                    _ => return Err(TemplateError::ConcatPart),
                }
                if joined.len() > MAX_CONCAT_BYTES {
                    return Err(TemplateError::ConcatTooLong);
                }
            }
            Ok(JsonValue::String(joined))
        }
    }
}

/// RFC 6901 pointer lookup.
fn pointer<'a>(value: &'a JsonValue, pointer: &str) -> Option<&'a JsonValue> {
    if pointer.is_empty() {
        return Some(value);
    }
    let mut current = value;
    for token in pointer.strip_prefix('/')?.split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        current = match current {
            JsonValue::Object(object) => object.get(&token)?,
            JsonValue::Array(values) => values.get(token.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn object_pointer<'a>(object: &'a JsonObject, reference: &str) -> Option<&'a JsonValue> {
    let (first, rest) = match reference.strip_prefix('/')?.split_once('/') {
        Some((first, rest)) => (first, Some(rest)),
        None => (reference.strip_prefix('/')?, None),
    };
    let value = object.get(&first.replace("~1", "/").replace("~0", "~"))?;
    match rest {
        Some(rest) => pointer(value, &format!("/{rest}")),
        None => Some(value),
    }
}

fn response<'a>(
    responses: &'a BTreeMap<String, JsonObject>,
    reference: &str,
) -> Option<&'a JsonValue> {
    let (request_id, path) = reference.split_once('.')?;
    let mut segments = path.split('.');
    let mut current = responses.get(request_id)?.get(segments.next()?)?;
    for segment in segments {
        current = match current {
            JsonValue::Object(object) => object.get(segment)?,
            JsonValue::Array(values) => values.get(segment.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn binds(value: &JsonValue, key: &str) -> bool {
    binds_matching(value, &|found, _| found == key)
}

fn binds_runx_value(value: &JsonValue, name: &str) -> bool {
    binds_matching(value, &|found, argument| {
        found == RUNX && argument.as_str() == Some(name)
    })
}

fn binds_matching(value: &JsonValue, matches: &dyn Fn(&str, &JsonValue) -> bool) -> bool {
    match value {
        JsonValue::Object(object) => match binding(object) {
            Some((found, argument)) if matches(found, argument) => true,
            Some((CONCAT, parts)) => parts
                .as_array()
                .into_iter()
                .flatten()
                .any(|part| binds_matching(part, matches)),
            Some((MINOR_UNITS, fields)) => fields
                .as_object()
                .is_some_and(|fields| fields.values().any(|part| binds_matching(part, matches))),
            Some(_) => false,
            None => object.values().any(|value| binds_matching(value, matches)),
        },
        JsonValue::Array(values) => values.iter().any(|value| binds_matching(value, matches)),
        _ => false,
    }
}

fn argument_pointers<'a>(value: &'a JsonValue, pointers: &mut BTreeSet<&'a str>) {
    match value {
        JsonValue::Object(object) => match binding(object) {
            Some((ARGUMENT, JsonValue::String(pointer))) => {
                pointers.insert(pointer);
            }
            Some((CONCAT, parts)) => {
                for part in parts.as_array().into_iter().flatten() {
                    argument_pointers(part, pointers);
                }
            }
            Some((MINOR_UNITS, fields)) => {
                for part in fields
                    .as_object()
                    .into_iter()
                    .flat_map(|fields| fields.values())
                {
                    argument_pointers(part, pointers);
                }
            }
            Some(_) => {}
            None => {
                for value in object.values() {
                    argument_pointers(value, pointers);
                }
            }
        },
        JsonValue::Array(values) => {
            for value in values {
                argument_pointers(value, pointers);
            }
        }
        _ => {}
    }
}

fn response_ids<'a>(value: &'a JsonValue, ids: &mut BTreeSet<&'a str>) {
    match value {
        JsonValue::Object(object) => match binding(object) {
            Some((RESPONSE, JsonValue::String(path))) => {
                ids.insert(path.split('.').next().unwrap_or_default());
            }
            Some((CONCAT, parts)) => {
                for part in parts.as_array().into_iter().flatten() {
                    response_ids(part, ids);
                }
            }
            Some((MINOR_UNITS, fields)) => {
                for part in fields
                    .as_object()
                    .into_iter()
                    .flat_map(|fields| fields.values())
                {
                    response_ids(part, ids);
                }
            }
            Some(_) => {}
            None => {
                for value in object.values() {
                    response_ids(value, ids);
                }
            }
        },
        JsonValue::Array(values) => {
            for value in values {
                response_ids(value, ids);
            }
        }
        _ => {}
    }
}

fn contains_binding(value: &JsonValue) -> bool {
    match value {
        JsonValue::Object(object) => {
            binding(object).is_some() || object.values().any(contains_binding)
        }
        JsonValue::Array(values) => values.iter().any(contains_binding),
        _ => false,
    }
}

fn decimal_units(value: &str, scale: u64) -> Option<u64> {
    if scale > 18 || value.len() > 64 {
        return None;
    }
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) if scale > 0 && !fraction.is_empty() => (whole, fraction),
        Some(_) => return None,
        None => (value, ""),
    };
    if whole.is_empty()
        || (whole.len() > 1 && whole.starts_with('0'))
        || fraction.len() as u64 != scale
    {
        return None;
    }
    let units = whole
        .bytes()
        .chain(fraction.bytes())
        .try_fold(0_u64, |value, digit| {
            digit
                .is_ascii_digit()
                .then_some(digit)
                .and_then(|digit| value.checked_mul(10)?.checked_add(u64::from(digit - b'0')))
        })?;
    (units <= crate::MAX_PORTABLE_INTEGER).then_some(units)
}

fn static_key(key: &str) -> &'static str {
    match key {
        ARGUMENT => ARGUMENT,
        PARAMETER => PARAMETER,
        RESPONSE => RESPONSE,
        ITEM => ITEM,
        RESULT => RESULT,
        RUNX => RUNX,
        MINOR_UNITS => MINOR_UNITS,
        _ => CONCAT,
    }
}

fn kind(value: &JsonValue) -> &'static str {
    match value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}
