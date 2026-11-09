use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue, PACKET_ID_EXPECTED, PacketId};

use crate::ValidationError;

use super::{FIELDS, SkillInput};

/// The input types a field may name. A field with no `type` takes any value.
const FIELD_TYPES: &[&str] = &["array", "boolean", "integer", "number", "object", "string"];

/// A method's inputs as `run.yaml` writes them: each field is its JSON
/// Schema, required unless it says `optional: true`, and an input that takes
/// an artifact names its `packet`.
pub(crate) fn validate_input_fields(
    inputs: &JsonObject,
    field_prefix: &str,
) -> Result<BTreeMap<String, SkillInput>, ValidationError> {
    inputs
        .iter()
        .map(|(name, value)| {
            let field = format!("{field_prefix}.{name}");
            // The invoker's act travels beside a run's inputs, under this name.
            if name == "act" {
                return Err(FIELDS.validation_error(format!(
                    "{field}: act is the act a run's invoker states, given beside its inputs, never one of them."
                )));
            }
            let mut schema = FIELDS.required_object(Some(value), &field)?.clone();
            let input_type = match schema.remove("type") {
                None => "json".to_owned(),
                Some(JsonValue::String(input_type))
                    if FIELD_TYPES.contains(&input_type.as_str()) =>
                {
                    input_type
                }
                Some(_) => {
                    return Err(FIELDS.validation_error(format!(
                        "{field}.type must be one of {}, or left out to take any value.",
                        FIELD_TYPES.join(", ")
                    )));
                }
            };
            let optional = match schema.remove("optional") {
                None => false,
                Some(JsonValue::Bool(optional)) => optional,
                Some(_) => {
                    return Err(FIELDS.validation_error(format!(
                        "{field}.optional must be true or false."
                    )));
                }
            };
            let description = schema
                .remove("description")
                .map(|description| {
                    FIELDS.required_string(Some(&description), &format!("{field}.description"))
                })
                .transpose()?;
            let packet = schema
                .remove("packet")
                .map(|packet| {
                    FIELDS
                        .required_string(Some(&packet), &format!("{field}.packet"))
                        .and_then(|packet| packet_id(packet, &format!("{field}.packet")))
                })
                .transpose()?;
            let validated = SkillInput {
                input_type,
                required: !optional,
                description,
                default: schema.remove("default"),
                packet,
                schema: (!schema.is_empty()).then_some(schema),
            };
            validate_input_schema(&validated, &field)?;
            Ok((name.clone(), validated))
        })
        .collect()
}

fn validate_input_schema(input: &SkillInput, field: &str) -> Result<(), ValidationError> {
    if input.packet.is_some() {
        if input.input_type != "json" {
            return Err(FIELDS.validation_error(format!(
                "{field}.packet takes the packet's own shape, so {field} names no type"
            )));
        }
        if input.schema.is_some() {
            return Err(FIELDS.validation_error(format!(
                "{field}.packet and {field}.schema are mutually exclusive; the packet catalog owns the schema"
            )));
        }
        if input.default.is_some() {
            return Err(FIELDS.validation_error(format!(
                "{field}.packet cannot declare a default packet value"
            )));
        }
        return Ok(());
    }
    let Some(fragment) = &input.schema else {
        if let Some(default) = &input.default
            && !input.accepts_value(default)
        {
            return Err(FIELDS.validation_error(format!(
                "{field}.default must match declared type {}",
                input.input_type
            )));
        }
        return Ok(());
    };
    let duplicate = ["type", "description", "default"]
        .into_iter()
        .find(|name| fragment.contains_key(*name));
    if let Some(duplicate) = duplicate {
        return Err(FIELDS.validation_error(format!(
            "{field}.schema.{duplicate} duplicates {field}.{duplicate}"
        )));
    }
    if let Some(examples) = fragment.get("examples")
        && !matches!(examples, JsonValue::Array(_))
    {
        return Err(FIELDS.validation_error(format!("{field}.schema.examples must be an array")));
    }
    let schema = serde_json::to_value(input.effective_schema()).map_err(|error| {
        FIELDS.validation_error(format!("{field}.schema could not be serialized: {error}"))
    })?;
    jsonschema::draft202012::meta::validate(&schema)
        .map_err(|error| FIELDS.validation_error(format!("{field}.schema is invalid: {error}")))?;
    validate_explicit_object_contracts(&schema, &format!("{field}.schema"))?;
    let values = input
        .default
        .iter()
        .map(|value| ("default".to_owned(), value))
        .chain(
            input
                .schema
                .as_ref()
                .and_then(|fragment| fragment.get("examples"))
                .and_then(JsonValue::as_array)
                .into_iter()
                .flatten()
                .enumerate()
                .map(|(index, value)| (format!("schema.examples[{index}]"), value)),
        )
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Ok(());
    }
    let validator = jsonschema::draft202012::options()
        .build(&schema)
        .map_err(|error| FIELDS.validation_error(format!("{field}.schema is invalid: {error}")))?;
    for (label, value) in values {
        let instance = serde_json::to_value(value).map_err(|error| {
            FIELDS.validation_error(format!("{field}.{label} could not be serialized: {error}"))
        })?;
        if !validator.is_valid(&instance) {
            return Err(FIELDS.validation_error(format!(
                "{field}.{label} must match the declared input schema"
            )));
        }
    }
    Ok(())
}

fn validate_explicit_object_contracts(
    schema: &serde_json::Value,
    field: &str,
) -> Result<(), ValidationError> {
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    let declares_object = match object.get("type") {
        Some(serde_json::Value::String(value)) => value == "object",
        Some(serde_json::Value::Array(values)) => values.iter().any(|value| value == "object"),
        _ => false,
    };
    let object_contract_is_explicit = [
        "$ref",
        "additionalProperties",
        "allOf",
        "anyOf",
        "const",
        "enum",
        "if",
        "maxProperties",
        "minProperties",
        "not",
        "oneOf",
        "patternProperties",
        "properties",
        "propertyNames",
        "unevaluatedProperties",
    ]
    .iter()
    .any(|keyword| object.contains_key(*keyword));
    if declares_object && !object_contract_is_explicit {
        return Err(FIELDS.validation_error(format!(
            "{field} declares an object without a shape; define its properties or set additionalProperties explicitly for an intentional open object"
        )));
    }

    for keyword in [
        "additionalProperties",
        "contains",
        "else",
        "if",
        "items",
        "not",
        "propertyNames",
        "then",
        "unevaluatedProperties",
    ] {
        if let Some(child) = object.get(keyword)
            && child.is_object()
        {
            validate_explicit_object_contracts(child, &format!("{field}.{keyword}"))?;
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = object.get(keyword).and_then(serde_json::Value::as_array) {
            for (index, child) in children.iter().enumerate() {
                validate_explicit_object_contracts(child, &format!("{field}.{keyword}[{index}]"))?;
            }
        }
    }
    for keyword in [
        "$defs",
        "definitions",
        "dependentSchemas",
        "patternProperties",
        "properties",
    ] {
        if let Some(children) = object.get(keyword).and_then(serde_json::Value::as_object) {
            for (name, child) in children {
                validate_explicit_object_contracts(child, &format!("{field}.{keyword}.{name}"))?;
            }
        }
    }
    Ok(())
}

fn packet_id(value: String, field: &str) -> Result<PacketId, ValidationError> {
    PacketId::new(value.clone()).ok_or_else(|| {
        FIELDS.validation_error(format!(
            "{field} '{value}' is not a packet id; {PACKET_ID_EXPECTED}."
        ))
    })
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue, PacketId};

    use super::validate_input_fields;
    use crate::ValidationError;

    fn packet_input(extra: impl IntoIterator<Item = (&'static str, JsonValue)>) -> JsonObject {
        let mut input = JsonObject::from([(
            "packet".to_owned(),
            JsonValue::String("runx.test.plan.v1".to_owned()),
        )]);
        input.extend(
            extra
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value)),
        );
        JsonObject::from([("plan".to_owned(), JsonValue::Object(input))])
    }

    fn object_input(schema: JsonObject) -> JsonObject {
        let mut input =
            JsonObject::from([("type".to_owned(), JsonValue::String("object".to_owned()))]);
        input.extend(schema);
        JsonObject::from([("value".to_owned(), JsonValue::Object(input))])
    }

    #[test]
    fn packet_input_reference_is_parser_owned() -> Result<(), ValidationError> {
        let inputs = validate_input_fields(&packet_input([]), "inputs")?;
        assert_eq!(
            inputs
                .get("plan")
                .and_then(|input| input.packet.as_ref())
                .map(PacketId::as_str),
            Some("runx.test.plan.v1")
        );
        Ok(())
    }

    #[test]
    fn packet_reference_outside_the_id_grammar_is_refused() {
        let mut inputs = packet_input([]);
        if let Some(JsonValue::Object(plan)) = inputs.get_mut("plan") {
            plan.insert(
                "packet".to_owned(),
                JsonValue::String("test.plan.v1".to_owned()),
            );
        }
        let error = validate_input_fields(&inputs, "inputs").err();
        assert!(
            error
                .as_ref()
                .is_some_and(|error| error.to_string().contains("is not a packet id")),
            "{error:?}"
        );
    }

    #[test]
    fn packet_input_reference_rejects_parallel_contract_ownership() {
        let inline_schema = packet_input([("properties", JsonValue::Object(JsonObject::new()))]);
        let default_packet = packet_input([("default", JsonValue::Object(JsonObject::new()))]);
        let wrong_type = packet_input([("type", JsonValue::String("object".to_owned()))]);

        assert!(validate_input_fields(&inline_schema, "inputs").is_err());
        assert!(validate_input_fields(&default_packet, "inputs").is_err());
        assert!(validate_input_fields(&wrong_type, "inputs").is_err());
    }

    #[test]
    fn object_inputs_require_recursive_shape_or_explicit_openness() -> Result<(), ValidationError> {
        let explicit_open = object_input(JsonObject::from([(
            "additionalProperties".to_owned(),
            JsonValue::Bool(true),
        )]));
        let nested_implicit_open = object_input(JsonObject::from([(
            "properties".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "payload".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "type".to_owned(),
                    JsonValue::String("object".to_owned()),
                )])),
            )])),
        )]));
        let nested_closed = object_input(JsonObject::from([(
            "properties".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "payload".to_owned(),
                JsonValue::Object(JsonObject::from([
                    ("type".to_owned(), JsonValue::String("object".to_owned())),
                    ("additionalProperties".to_owned(), JsonValue::Bool(false)),
                ])),
            )])),
        )]));

        assert!(validate_input_fields(&nested_implicit_open, "inputs").is_err());
        validate_input_fields(&explicit_open, "inputs")?;
        validate_input_fields(&nested_closed, "inputs")?;
        Ok(())
    }
}
