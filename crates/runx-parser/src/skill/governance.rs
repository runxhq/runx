use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue};

use crate::ValidationError;

use super::{FIELDS, SkillArtifactContract, SkillInput};

pub fn validate_skill_artifact_contract(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<SkillArtifactContract>, ValidationError> {
    validate_artifact_contract(value, field)
}

pub(super) fn validate_artifact_contract(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<SkillArtifactContract>, ValidationError> {
    let Some(record) = FIELDS.optional_object(value, field)? else {
        return Ok(None);
    };
    FIELDS.reject_unknown_fields(
        &record,
        field,
        &["emits", "named_emits", "packets", "wrap_as", "packet"],
    )?;
    let emits = match record.get("emits") {
        Some(JsonValue::String(value)) => Some(vec![value.clone()]),
        value => FIELDS.optional_string_array(value, &format!("{field}.emits"))?,
    };
    let named_emits =
        validate_named_emits(record.get("named_emits"), &format!("{field}.named_emits"))?;
    let packets = validate_named_emits(record.get("packets"), &format!("{field}.packets"))?;
    if let Some(packet_outputs) = &packets {
        let Some(named_outputs) = &named_emits else {
            return Err(
                FIELDS.validation_error(format!("{field}.packets requires {field}.named_emits"))
            );
        };
        if let Some(output) = packet_outputs
            .keys()
            .find(|output| !named_outputs.contains_key(*output))
        {
            return Err(FIELDS.validation_error(format!(
                "{field}.packets.{output} must name an output declared by {field}.named_emits"
            )));
        }
    }
    let wrap_as =
        FIELDS.optional_non_empty_string(record.get("wrap_as"), &format!("{field}.wrap_as"))?;
    let packet =
        FIELDS.optional_non_empty_string(record.get("packet"), &format!("{field}.packet"))?;
    if packet.is_some() && wrap_as.is_none() {
        return Err(FIELDS.validation_error(format!(
            "{field}.packet requires {field}.wrap_as. Use named_emits for named packet outputs."
        )));
    }
    if emits.is_none() && named_emits.is_none() && packets.is_none() && wrap_as.is_none() {
        return Ok(None);
    }
    Ok(Some(SkillArtifactContract {
        emits,
        named_emits,
        packets,
        wrap_as,
        packet,
    }))
}

pub(crate) fn validate_inputs(
    inputs: JsonObject,
    field_prefix: &str,
) -> Result<BTreeMap<String, SkillInput>, ValidationError> {
    inputs
        .into_iter()
        .map(|(name, value)| {
            let field = format!("{field_prefix}.{name}");
            // The invoker's act travels beside a run's inputs, under this name.
            if name == "act" {
                return Err(FIELDS.validation_error(format!(
                    "{field}: act is the act a run's invoker states, given beside its inputs, never one of them."
                )));
            }
            let input = FIELDS.required_object(Some(&value), &field)?;
            FIELDS.reject_unknown_fields(
                input,
                &field,
                &[
                    "type",
                    "required",
                    "description",
                    "default",
                    "artifact",
                    "packet",
                    "schema",
                ],
            )?;
            let input_type = FIELDS
                .optional_string(input.get("type"), &format!("{field}.type"))?
                .unwrap_or_else(|| "string".to_owned());
            if !matches!(
                input_type.as_str(),
                "array" | "boolean" | "integer" | "json" | "number" | "object" | "string"
            ) {
                return Err(FIELDS.validation_error(format!(
                    "{field}.type must be one of array, boolean, integer, json, number, object, or string"
                )));
            }
            let validated = SkillInput {
                input_type,
                required: FIELDS
                    .optional_bool(input.get("required"), &format!("{field}.required"))?
                    .unwrap_or(false),
                description: FIELDS.optional_string(
                    input.get("description"),
                    &format!("{field}.description"),
                )?,
                default: input.get("default").cloned(),
                artifact: FIELDS
                    .optional_bool(input.get("artifact"), &format!("{field}.artifact"))?,
                packet: FIELDS.optional_non_empty_string(
                    input.get("packet"),
                    &format!("{field}.packet"),
                )?,
                schema: FIELDS
                    .optional_object(input.get("schema"), &format!("{field}.schema"))?,
            };
            validate_input_schema(&validated, &field)?;
            Ok((name.clone(), validated))
        })
        .collect()
}

fn validate_input_schema(input: &SkillInput, field: &str) -> Result<(), ValidationError> {
    if input.packet.is_some() {
        if input.input_type != "json" {
            return Err(
                FIELDS.validation_error(format!("{field}.packet requires {field}.type to be json"))
            );
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

fn validate_named_emits(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<BTreeMap<String, String>>, ValidationError> {
    let Some(record) = FIELDS.optional_object(value, field)? else {
        return Ok(None);
    };
    record
        .into_iter()
        .map(|(key, value)| {
            let JsonValue::String(value) = value else {
                return Err(
                    FIELDS.validation_error(format!("{field}.{key} must be a non-empty string."))
                );
            };
            if value.trim().is_empty() {
                return Err(
                    FIELDS.validation_error(format!("{field}.{key} must be a non-empty string."))
                );
            }
            Ok((key, value))
        })
        .collect::<Result<BTreeMap<_, _>, _>>()
        .map(Some)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use runx_contracts::{JsonObject, JsonValue};

    use super::{validate_artifact_contract, validate_inputs};
    use crate::ValidationError;

    fn packet_input(extra: impl IntoIterator<Item = (&'static str, JsonValue)>) -> JsonObject {
        let mut input = JsonObject::from([
            ("type".to_owned(), JsonValue::String("json".to_owned())),
            ("required".to_owned(), JsonValue::Bool(true)),
            (
                "packet".to_owned(),
                JsonValue::String("runx.test.plan.v1".to_owned()),
            ),
        ]);
        input.extend(
            extra
                .into_iter()
                .map(|(name, value)| (name.to_owned(), value)),
        );
        JsonObject::from([("plan".to_owned(), JsonValue::Object(input))])
    }

    fn object_input(schema: JsonObject) -> JsonObject {
        JsonObject::from([(
            "value".to_owned(),
            JsonValue::Object(JsonObject::from([
                ("type".to_owned(), JsonValue::String("object".to_owned())),
                ("schema".to_owned(), JsonValue::Object(schema)),
            ])),
        )])
    }

    #[test]
    fn packet_input_reference_is_parser_owned() -> Result<(), ValidationError> {
        let inputs = validate_inputs(packet_input([]), "inputs")?;
        assert_eq!(
            inputs.get("plan").and_then(|input| input.packet.as_deref()),
            Some("runx.test.plan.v1")
        );
        Ok(())
    }

    #[test]
    fn packet_input_reference_rejects_parallel_contract_ownership() {
        let inline_schema = packet_input([(
            "schema",
            JsonValue::Object(JsonObject::from([(
                "properties".to_owned(),
                JsonValue::Object(JsonObject::new()),
            )])),
        )]);
        let default_packet = packet_input([("default", JsonValue::Object(JsonObject::new()))]);
        let wrong_type = packet_input([("type", JsonValue::String("object".to_owned()))]);

        assert!(validate_inputs(inline_schema, "inputs").is_err());
        assert!(validate_inputs(default_packet, "inputs").is_err());
        assert!(validate_inputs(wrong_type, "inputs").is_err());
    }

    #[test]
    fn object_inputs_require_recursive_shape_or_explicit_openness() -> Result<(), ValidationError> {
        let implicit_open = object_input(JsonObject::new());
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

        assert!(validate_inputs(implicit_open, "inputs").is_err());
        assert!(validate_inputs(nested_implicit_open, "inputs").is_err());
        validate_inputs(explicit_open, "inputs")?;
        validate_inputs(nested_closed, "inputs")?;
        Ok(())
    }

    #[test]
    fn packet_bindings_must_reference_named_outputs() {
        let artifacts = JsonValue::Object(BTreeMap::from([
            (
                "named_emits".to_owned(),
                JsonValue::Object(BTreeMap::from([(
                    "plan".to_owned(),
                    JsonValue::String("plan".to_owned()),
                )])),
            ),
            (
                "packets".to_owned(),
                JsonValue::Object(BTreeMap::from([(
                    "other".to_owned(),
                    JsonValue::String("runx.plan.v1".to_owned()),
                )])),
            ),
        ]));

        assert!(validate_artifact_contract(Some(&artifacts), "artifacts").is_err());
    }

    #[test]
    fn named_output_and_packet_identity_are_preserved_separately()
    -> Result<(), Box<dyn std::error::Error>> {
        let artifacts = JsonValue::Object(BTreeMap::from([
            (
                "named_emits".to_owned(),
                JsonValue::Object(BTreeMap::from([(
                    "plan".to_owned(),
                    JsonValue::String("plan".to_owned()),
                )])),
            ),
            (
                "packets".to_owned(),
                JsonValue::Object(BTreeMap::from([(
                    "plan".to_owned(),
                    JsonValue::String("runx.plan.v1".to_owned()),
                )])),
            ),
        ]));

        let Some(contract) = validate_artifact_contract(Some(&artifacts), "artifacts")? else {
            return Err("artifact contract is missing".into());
        };

        assert_eq!(
            contract
                .named_emits
                .as_ref()
                .and_then(|outputs| outputs.get("plan"))
                .map(String::as_str),
            Some("plan")
        );
        assert_eq!(
            contract
                .packets
                .as_ref()
                .and_then(|packets| packets.get("plan"))
                .map(String::as_str),
            Some("runx.plan.v1")
        );
        Ok(())
    }
}
