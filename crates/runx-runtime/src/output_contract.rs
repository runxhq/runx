//! Runner-output and packet-contract verification shared by every execution
//! source. Agent, JavaScript, CLI, native-tool, and nested-skill results must
//! cross the same typed boundary before a successful receipt can be sealed.

use std::collections::BTreeMap;
use std::path::Path;

use runx_contracts::{
    JsonObject, JsonValue, OutputField, output_contract_digest,
    parse_output_contract as parse_contract_outputs, validate_output_value,
};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::adapter::{CONTRACT_VERIFICATION_METADATA, InvocationOutput};

/// Whether a successful producer declares any addressable semantic output.
///
/// This is the admission-time counterpart to [`project_declared_output_claim`].
/// Keep both paths on the same typed contracts so preflight cannot approve a
/// producer that execution would later project as transport-only.
#[must_use]
pub(crate) fn declares_output_contract(
    raw_output: Option<&JsonObject>,
    artifacts: Option<&SkillArtifactContract>,
) -> bool {
    raw_output.is_some_and(|outputs| !outputs.is_empty())
        || artifacts.is_some_and(|artifacts| {
            artifacts.wrap_as.is_some()
                || artifacts
                    .named_emits
                    .as_ref()
                    .is_some_and(|outputs| !outputs.is_empty())
        })
}

/// Project the exact semantic claim a successful producer declared.
///
/// The claim is one shared contract surface for graph state, effect admission,
/// harness replay, and receipt identity. Raw process output, diagnostics, and
/// transport envelopes never enter it. Callers that normalize a provider
/// protocol must pass that already-normalized payload explicitly.
pub(crate) fn project_declared_output_claim(
    producer: &str,
    payload: &JsonValue,
    raw_output: Option<&JsonObject>,
    artifacts: Option<&SkillArtifactContract>,
) -> Result<JsonObject, RuntimeError> {
    let fields = payload.as_object();
    let mut claim = JsonObject::new();
    if let Some(raw_output) = raw_output {
        for (name, field) in parse_output_contract(raw_output)? {
            let Some(value) = fields.and_then(|fields| fields.get(&name)).cloned() else {
                if !field.is_required() {
                    continue;
                }
                return Err(RuntimeError::SkillFailed {
                    skill_name: producer.to_owned(),
                    message: format!("declared run output {name:?} was not returned"),
                });
            };
            claim.insert(name, value);
        }
    }
    claim.extend(project_artifact_outputs(payload, artifacts));
    Ok(claim)
}

/// Fold the artifact envelopes into an object payload, so a step exposes
/// `<step>.<packet>.data` for each declared artifact.
pub(crate) fn apply_artifact_outputs(
    value: &mut JsonValue,
    artifacts: Option<&SkillArtifactContract>,
) {
    let projected = project_artifact_outputs(value, artifacts);
    if let JsonValue::Object(object) = value {
        object.extend(projected);
    }
}

/// Project only the artifact-addressable portion of a producer payload.
///
/// Tool dispatch and step/receipt projection call this same function, so
/// `wrap_as` and `named_emits` cannot drift by entry point.
#[must_use]
pub(crate) fn project_artifact_outputs(
    payload: &JsonValue,
    artifacts: Option<&SkillArtifactContract>,
) -> JsonObject {
    let Some(artifacts) = artifacts else {
        return JsonObject::new();
    };
    let fields = payload.as_object();
    let mut outputs = JsonObject::new();
    if let Some(wrap_as) = artifacts.wrap_as.as_deref() {
        let value = fields
            .and_then(|fields| fields.get(wrap_as))
            .cloned()
            .unwrap_or_else(|| payload.clone());
        outputs.insert(wrap_as.to_owned(), data_envelope(value));
    }
    if let Some(named_emits) = artifacts.named_emits.as_ref() {
        for name in named_emits.keys() {
            let Some(value) = fields.and_then(|fields| fields.get(name)).cloned() else {
                continue;
            };
            outputs.insert(name.clone(), data_envelope(value));
        }
    }
    outputs
}

/// Wrap a value in the canonical `{ "data": ... }` artifact envelope.
///
/// A runtime-created `{ "data": ... }` wrapper and a self-described
/// `{ "schema": ..., "data": ... }` packet are already envelopes. A domain
/// object that merely owns a `data` field is not: preserving the whole object
/// keeps sibling fields at their declared context path.
#[must_use]
fn data_envelope(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(object) if is_data_envelope(&object) => JsonValue::Object(object),
        other => JsonValue::Object(JsonObject::from([("data".to_owned(), other)])),
    }
}

/// The value an output holds, read through the envelope `data_envelope`
/// wrapped it in. References address values, never envelopes.
#[must_use]
pub(crate) fn enveloped_value(value: &JsonValue) -> &JsonValue {
    match value {
        JsonValue::Object(object) if is_data_envelope(object) => {
            object.get("data").unwrap_or(value)
        }
        other => other,
    }
}

fn is_data_envelope(object: &JsonObject) -> bool {
    object.contains_key("data")
        && (object.len() == 1
            || object
                .get("schema")
                .and_then(JsonValue::as_str)
                .is_some_and(|schema| !schema.trim().is_empty()))
}

pub(crate) fn verified_runner_metadata_with_artifacts(
    skill_name: &str,
    payload: &JsonValue,
    raw_output: Option<&JsonObject>,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<JsonObject, RuntimeError> {
    let output = raw_output.map(parse_output_contract).transpose()?;
    verified_output_metadata_with_artifacts(
        skill_name,
        payload,
        output.as_ref(),
        artifacts,
        skill_directory,
        env,
    )
}

pub(crate) fn verified_output_metadata_with_artifacts(
    skill_name: &str,
    payload: &JsonValue,
    output: Option<&BTreeMap<String, OutputField>>,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
    env: &BTreeMap<String, String>,
) -> Result<JsonObject, RuntimeError> {
    let has_packet_contract = artifact_packet_contracts(artifacts);
    if output.is_none() && !has_packet_contract {
        return Ok(JsonObject::new());
    }

    let mut verification = JsonObject::new();
    if let Some(output) = output {
        validate_output_value(Some(output), payload).map_err(|error| {
            RuntimeError::SkillFailed {
                skill_name: skill_name.to_owned(),
                message: format!("runner output contract violation at {error}"),
            }
        })?;
        let digest = output_contract_digest(Some(output))
            .map_err(|source| RuntimeError::json("hashing runner output contract", source))?;
        verification.insert(
            "output_contract_sha256".to_owned(),
            JsonValue::String(digest),
        );
    }

    let packet_schemas = crate::packet_validation::verify_declared_packets(
        payload,
        artifacts,
        skill_directory,
        env,
    )?;
    if !packet_schemas.is_empty() {
        verification.insert(
            "packet_schemas".to_owned(),
            JsonValue::Object(packet_schemas),
        );
    }

    if verification.is_empty() {
        return Ok(JsonObject::new());
    }
    Ok([(
        CONTRACT_VERIFICATION_METADATA.to_owned(),
        JsonValue::Object(verification),
    )]
    .into_iter()
    .collect())
}

pub(crate) fn attach_verified_metadata(
    output: &mut InvocationOutput,
    mut metadata: JsonObject,
) -> Result<(), RuntimeError> {
    let Some(verification) = metadata.remove(CONTRACT_VERIFICATION_METADATA) else {
        return Ok(());
    };
    if !metadata.is_empty() {
        return Err(RuntimeError::ReceiptInvalid {
            message: "runner contract verification produced undeclared metadata".to_owned(),
        });
    }
    if output
        .metadata
        .insert(CONTRACT_VERIFICATION_METADATA.to_owned(), verification)
        .is_some()
    {
        return Err(RuntimeError::ReceiptInvalid {
            message: "runner output supplied duplicate contract verification metadata".to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn parse_output_contract(
    raw: &JsonObject,
) -> Result<BTreeMap<String, OutputField>, RuntimeError> {
    parse_contract_outputs(raw).map_err(|source| RuntimeError::ReceiptInvalid {
        message: format!("runner output contract is invalid: {source}"),
    })
}

fn artifact_packet_contracts(artifacts: Option<&SkillArtifactContract>) -> bool {
    artifacts.is_some_and(|artifacts| {
        artifacts.packet.is_some()
            || artifacts
                .packets
                .as_ref()
                .is_some_and(|packets| !packets.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifacts(wrap_as: Option<&str>, named_emits: &[(&str, &str)]) -> SkillArtifactContract {
        SkillArtifactContract {
            emits: None,
            named_emits: (!named_emits.is_empty()).then(|| {
                named_emits
                    .iter()
                    .map(|(name, packet)| ((*name).to_owned(), (*packet).to_owned()))
                    .collect()
            }),
            packets: None,
            wrap_as: wrap_as.map(str::to_owned),
            packet: None,
        }
    }

    #[test]
    fn result_admission_matches_addressable_output_declarations() {
        let wrapped = artifacts(Some("result"), &[]);
        let named = artifacts(None, &[("result", "runx.test.result.v1")]);
        let transport_only = SkillArtifactContract {
            emits: Some(vec!["result".to_owned()]),
            named_emits: None,
            packets: None,
            wrap_as: None,
            packet: None,
        };

        assert!(!declares_output_contract(None, None));
        assert!(!declares_output_contract(None, Some(&transport_only)));
        assert!(declares_output_contract(
            Some(&JsonObject::from([(
                "result".to_owned(),
                JsonValue::String("object".to_owned())
            )])),
            None,
        ));
        assert!(declares_output_contract(None, Some(&wrapped)));
        assert!(declares_output_contract(None, Some(&named)));
    }

    #[test]
    fn no_declared_contract_produces_no_receipt_metadata() -> Result<(), RuntimeError> {
        assert!(
            verified_output_metadata_with_artifacts(
                "plain",
                &JsonValue::String("plain output".to_owned()),
                None,
                None,
                Path::new("."),
                &BTreeMap::new(),
            )?
            .is_empty()
        );
        Ok(())
    }

    #[test]
    fn declared_claim_projects_only_contract_fields_and_artifacts() -> Result<(), RuntimeError> {
        let payload = JsonValue::Object(JsonObject::from([
            (
                "result".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "message".to_owned(),
                    JsonValue::String("done".to_owned()),
                )])),
            ),
            (
                "transport_noise".to_owned(),
                JsonValue::String("not sealed".to_owned()),
            ),
        ]));
        let output = JsonObject::from([
            ("result".to_owned(), JsonValue::String("object".to_owned())),
            (
                "optional".to_owned(),
                JsonValue::Object(JsonObject::from([
                    ("type".to_owned(), JsonValue::String("string".to_owned())),
                    ("required".to_owned(), JsonValue::Bool(false)),
                ])),
            ),
        ]);

        let claim = project_declared_output_claim(
            "producer",
            &payload,
            Some(&output),
            Some(&artifacts(None, &[("result", "test.result.v1")])),
        )?;

        assert!(!claim.contains_key("transport_noise"));
        assert!(!claim.contains_key("optional"));
        assert_eq!(
            claim
                .get("result")
                .and_then(JsonValue::as_object)
                .and_then(|packet| packet.get("data"))
                .and_then(JsonValue::as_object)
                .and_then(|result| result.get("message"))
                .and_then(JsonValue::as_str),
            Some("done")
        );
        Ok(())
    }

    #[test]
    fn declared_claim_rejects_a_missing_required_field() {
        let error = project_declared_output_claim(
            "producer",
            &JsonValue::Object(JsonObject::new()),
            Some(&JsonObject::from([(
                "result".to_owned(),
                JsonValue::String("object".to_owned()),
            )])),
            None,
        );

        assert!(matches!(
            error,
            Err(RuntimeError::SkillFailed { skill_name, message })
                if skill_name == "producer"
                    && message.contains("declared run output \"result\" was not returned")
        ));
    }

    #[test]
    fn artifact_projection_distinguishes_packets_from_domain_data_objects() {
        let packet = JsonValue::Object(JsonObject::from([
            (
                "schema".to_owned(),
                JsonValue::String("test.packet.v1".to_owned()),
            ),
            (
                "data".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "message".to_owned(),
                    JsonValue::String("hello".to_owned()),
                )])),
            ),
        ]));
        let packet_projection =
            project_artifact_outputs(&packet, Some(&artifacts(Some("packet"), &[])));
        assert_eq!(packet_projection.get("packet"), Some(&packet));

        let page = JsonValue::Object(JsonObject::from([
            (
                "offset".to_owned(),
                JsonValue::Number(runx_contracts::JsonNumber::U64(0)),
            ),
            (
                "data".to_owned(),
                JsonValue::String("page bytes".to_owned()),
            ),
        ]));
        let page_projection = project_artifact_outputs(&page, Some(&artifacts(Some("page"), &[])));
        assert_eq!(
            page_projection
                .get("page")
                .and_then(JsonValue::as_object)
                .and_then(|packet| packet.get("data")),
            Some(&page)
        );
    }
}
