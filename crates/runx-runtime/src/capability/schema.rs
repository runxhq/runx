use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::tools::ToolInput;
use runx_contracts::{JsonObject, JsonValue};

use super::{CapabilityArtifacts, CapabilityDefinition, CapabilityInput, RuntimeError, invalid};

/// A capability's input schema: what decoding its input accepts, never only
/// the one form Rust writes, with its defaults and descriptions.
pub(super) fn effective_schema<I: CapabilityInput>(
    definition: &CapabilityDefinition,
) -> Result<serde_json::Value, RuntimeError> {
    let mut schema = I::accepted_schema();
    let defaults = I::defaults();
    let Some(object) = schema.as_object_mut() else {
        return Ok(schema);
    };
    if let Some(required) = object
        .get_mut("required")
        .and_then(serde_json::Value::as_array_mut)
    {
        required.retain(|name| {
            name.as_str()
                .is_none_or(|name| !defaults.contains_key(name))
        });
    }
    append_property_metadata(object, definition, defaults)?;
    Ok(schema)
}

fn append_property_metadata(
    schema: &mut serde_json::Map<String, serde_json::Value>,
    definition: &CapabilityDefinition,
    defaults: JsonObject,
) -> Result<(), RuntimeError> {
    let Some(properties) = schema
        .get_mut("properties")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return Ok(());
    };
    for (name, value) in defaults {
        if let Some(property) = properties
            .get_mut(&name)
            .and_then(serde_json::Value::as_object_mut)
        {
            property.insert(
                "default".to_owned(),
                serde_json::to_value(value).map_err(|source| {
                    RuntimeError::json(
                        format!("serializing default for capability {}", definition.id),
                        source,
                    )
                })?,
            );
        }
    }
    for field in definition.fields {
        if let Some(property) = properties
            .get_mut(field.name)
            .and_then(serde_json::Value::as_object_mut)
        {
            property.insert(
                "description".to_owned(),
                serde_json::Value::String(field.description.to_owned()),
            );
        }
    }
    Ok(())
}

pub(super) fn normalize_inputs<I: CapabilityInput>(
    definition: &CapabilityDefinition,
    inputs: &JsonObject,
) -> Result<JsonObject, RuntimeError> {
    let normalized = project_inputs::<I>(definition, inputs.clone());
    deserialize_input::<I>(definition, normalized.clone())?;
    Ok(normalized)
}

pub(super) fn decode_inputs<I: CapabilityInput>(
    definition: &CapabilityDefinition,
    inputs: JsonObject,
) -> Result<I, RuntimeError> {
    deserialize_input::<I>(definition, project_inputs::<I>(definition, inputs))
}

fn project_inputs<I: CapabilityInput>(
    definition: &CapabilityDefinition,
    mut inputs: JsonObject,
) -> JsonObject {
    inputs.retain(|name, _| definition.fields.iter().any(|field| field.name == name));
    for (name, value) in I::defaults() {
        inputs.entry(name).or_insert(value);
    }
    inputs
}

fn deserialize_input<I: CapabilityInput>(
    definition: &CapabilityDefinition,
    inputs: JsonObject,
) -> Result<I, RuntimeError> {
    JsonValue::Object(inputs)
        .deserialize_into()
        .map_err(|source| invalid(definition.id, format!("invalid typed input: {source}")))
}

pub(super) fn catalog_inputs(
    definition: &CapabilityDefinition,
    schema: &serde_json::Value,
    defaults: &JsonObject,
) -> Result<BTreeMap<String, ToolInput>, RuntimeError> {
    let properties = schema_properties(definition.id, schema)?;
    let required = required_properties(schema);
    properties
        .iter()
        .map(|(name, property)| catalog_input(definition, defaults, &required, name, property))
        .collect()
}

fn required_properties(schema: &serde_json::Value) -> BTreeSet<&str> {
    schema
        .get("required")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect()
}

fn catalog_input(
    definition: &CapabilityDefinition,
    defaults: &JsonObject,
    required: &BTreeSet<&str>,
    name: &String,
    property: &serde_json::Value,
) -> Result<(String, ToolInput), RuntimeError> {
    let description = definition
        .fields
        .iter()
        .find(|field| field.name == name)
        .map(|field| field.description.to_owned());
    let default = defaults.get(name).cloned();
    let property = present_choice(property);
    let input_type = schema_type(property);
    let schema = keywords(definition, property, input_type != "json")?;
    Ok((
        name.clone(),
        ToolInput {
            input_type,
            required: required.contains(name.as_str()),
            description,
            default,
            artifact: None,
            packet: None,
            schema,
        },
    ))
}

/// The schema a property's value takes when present: an optional property's
/// choice other than `null`, or the property's own.
fn present_choice(schema: &serde_json::Value) -> &serde_json::Value {
    schema
        .get("anyOf")
        .and_then(serde_json::Value::as_array)
        .and_then(|variants| {
            variants.iter().find(|variant| {
                variant.get("type").and_then(serde_json::Value::as_str) != Some("null")
            })
        })
        .unwrap_or(schema)
}

/// Every keyword of a property's schema that the input's own fields do not
/// carry (its items, bounds, enums and nesting), so each projection offers the
/// shape the capability decodes. Its `type` stays where the input's type is
/// `json` and so says none.
fn keywords(
    definition: &CapabilityDefinition,
    schema: &serde_json::Value,
    typed: bool,
) -> Result<Option<JsonObject>, RuntimeError> {
    let Some(object) = schema.as_object() else {
        return Ok(None);
    };
    let rest = object
        .iter()
        .filter(|(key, _)| match key.as_str() {
            "description" | "default" => false,
            "type" => !typed,
            _ => true,
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    if rest.is_empty() {
        return Ok(None);
    }
    serde_json::from_value(serde_json::Value::Object(rest))
        .map(Some)
        .map_err(|source| {
            RuntimeError::json(
                format!("projecting an input schema of capability {}", definition.id),
                source,
            )
        })
}

pub(super) fn schema_properties<'a>(
    id: &str,
    schema: &'a serde_json::Value,
) -> Result<&'a serde_json::Map<String, serde_json::Value>, RuntimeError> {
    schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid(id, "typed input schema must be an object with properties"))
}

/// A schema's one JSON type as an input's type, or `json` where it states none.
fn schema_type(schema: &serde_json::Value) -> String {
    schema
        .get("type")
        .and_then(serde_json::Value::as_str)
        .filter(|kind| {
            matches!(
                *kind,
                "string" | "number" | "integer" | "boolean" | "object" | "array"
            )
        })
        .unwrap_or("json")
        .to_owned()
}

pub(super) fn output_schema(artifacts: CapabilityArtifacts) -> serde_json::Value {
    match artifacts {
        CapabilityArtifacts::None => serde_json::json!({}),
        CapabilityArtifacts::Named { output, packet } => serde_json::json!({
            "type": "object",
            "required": [output],
            "properties": {
                (output): { "x-runx-packet": packet }
            }
        }),
        CapabilityArtifacts::Wrapped { packet, .. } => serde_json::json!({
            "type": "object",
            "x-runx-packet": packet
        }),
    }
}

pub(super) fn validate_output(
    definition: &CapabilityDefinition,
    output: &JsonValue,
) -> Result<(), RuntimeError> {
    match definition.artifacts {
        CapabilityArtifacts::None => Ok(()),
        CapabilityArtifacts::Named { output: name, .. } => output
            .as_object()
            .filter(|object| object.contains_key(name))
            .map(|_| ())
            .ok_or_else(|| {
                invalid(
                    definition.id,
                    format!("capability output must contain named packet {name:?}"),
                )
            }),
        CapabilityArtifacts::Wrapped { .. } => output.as_object().map(|_| ()).ok_or_else(|| {
            invalid(
                definition.id,
                "capability output must be an object before packet wrapping",
            )
        }),
    }
}
