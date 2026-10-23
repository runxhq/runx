//! Runner-output and packet-contract verification shared by every execution
//! source. Agent, JavaScript, CLI, native-tool, and nested-skill results must
//! cross the same typed boundary before a successful receipt can be sealed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use runx_contracts::{
    JsonObject, JsonValue, OutputField, output_contract_digest,
    parse_output_contract as parse_contract_outputs, validate_output_value,
};
use runx_parser::SkillArtifactContract;

use crate::RuntimeError;
use crate::observations::ContractChecks;

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
    // `wrap_as` names the whole result as one artifact; `named_emits`
    // selects fields of it. Neither looks at the result to decide.
    if let Some(wrap_as) = artifacts.wrap_as.as_deref() {
        outputs.insert(wrap_as.to_owned(), data_envelope(payload.clone()));
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

/// Wrap an artifact's value in its `{ "data": ... }` envelope, once: which
/// outputs are artifacts is declared, so a value is never inspected to guess
/// whether it is wrapped already.
#[must_use]
fn data_envelope(value: JsonValue) -> JsonValue {
    JsonValue::Object(JsonObject::from([("data".to_owned(), value)]))
}

/// The outputs among `outputs` that `artifacts` declares: each is held in
/// its packet envelope.
#[must_use]
pub(crate) fn artifact_outputs(
    outputs: &JsonObject,
    artifacts: Option<&SkillArtifactContract>,
) -> BTreeSet<String> {
    let Some(artifacts) = artifacts else {
        return BTreeSet::new();
    };
    artifacts
        .wrap_as
        .iter()
        .chain(artifacts.named_emits.iter().flat_map(|emits| emits.keys()))
        .filter(|name| outputs.contains_key(name.as_str()))
        .cloned()
        .collect()
}

/// The value a step's output holds: an artifact's packet data, or any other
/// output as it is. Which outputs are artifacts is declared, never inferred
/// from a value's shape. References address values, never envelopes.
#[must_use]
pub(crate) fn output_value<'a>(
    outputs: &'a JsonObject,
    artifact_outputs: &BTreeSet<String>,
    name: &str,
) -> Option<&'a JsonValue> {
    let value = outputs.get(name)?;
    if artifact_outputs.contains(name) {
        artifact_data(value)
    } else {
        Some(value)
    }
}

/// The data an artifact's packet envelope carries.
#[must_use]
pub(crate) fn artifact_data(envelope: &JsonValue) -> Option<&JsonValue> {
    envelope.as_object()?.get("data")
}

/// The checks of a runner's result against its declared output contract and
/// packets, the contract parsed from the runner's raw declaration.
pub(crate) fn runner_contract_checks(
    skill_name: &str,
    payload: &JsonValue,
    raw_output: Option<&JsonObject>,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
) -> Result<ContractChecks, RuntimeError> {
    let output = raw_output.map(parse_output_contract).transpose()?;
    output_contract_checks(
        skill_name,
        payload,
        output.as_ref(),
        artifacts,
        skill_directory,
    )
}

/// Check a result against its declared output contract and packets. A
/// result that violates either fails; a step that declares neither is not
/// checked.
pub(crate) fn output_contract_checks(
    skill_name: &str,
    payload: &JsonValue,
    output: Option<&BTreeMap<String, OutputField>>,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
) -> Result<ContractChecks, RuntimeError> {
    let mut checks = ContractChecks::default();
    if let Some(output) = output {
        validate_output_value(Some(output), payload).map_err(|error| {
            RuntimeError::SkillFailed {
                skill_name: skill_name.to_owned(),
                message: format!("runner output contract violation at {error}"),
            }
        })?;
        checks.output_contract_digest = Some(
            output_contract_digest(Some(output))
                .map_err(|source| RuntimeError::json("hashing runner output contract", source))?,
        );
    }
    checks.packet_schemas = crate::packet_validation::verify_declared_packets(
        &project_artifact_outputs(payload, artifacts),
        artifacts,
        skill_directory,
    )?;
    Ok(checks)
}

/// The check of the artifacts a step holds, each against the packet its
/// contract declares: for a step whose artifacts are already projected, as a
/// nested run's are when its parent adopts them.
pub(crate) fn artifact_contract_checks(
    held: &JsonObject,
    artifacts: Option<&SkillArtifactContract>,
    skill_directory: &Path,
) -> Result<ContractChecks, RuntimeError> {
    Ok(ContractChecks {
        packet_schemas: crate::packet_validation::verify_declared_packets(
            held,
            artifacts,
            skill_directory,
        )?,
        ..ContractChecks::default()
    })
}

pub(crate) fn parse_output_contract(
    raw: &JsonObject,
) -> Result<BTreeMap<String, OutputField>, RuntimeError> {
    parse_contract_outputs(raw).map_err(|source| RuntimeError::ReceiptInvalid {
        message: format!("runner output contract is invalid: {source}"),
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
    fn no_declared_contract_runs_no_check() -> Result<(), RuntimeError> {
        assert_eq!(
            output_contract_checks(
                "plain",
                &JsonValue::String("plain output".to_owned()),
                None,
                None,
                Path::new("."),
            )?,
            ContractChecks::default()
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
    fn an_artifact_is_wrapped_once_whatever_its_value_looks_like() {
        // A value shaped like an envelope is still the artifact's value: the
        // declaration says what is an artifact, never the value's shape.
        for value in [
            serde_json::json!({"schema": "domain.v1", "data": {"id": 7}, "source": "crm"}),
            serde_json::json!({"data": "page bytes"}),
            serde_json::json!({"offset": 0, "data": "page bytes"}),
        ] {
            let value = JsonValue::from_serialize(&value).unwrap_or(JsonValue::Null);
            let projection =
                project_artifact_outputs(&value, Some(&artifacts(Some("report"), &[])));
            assert_eq!(
                projection
                    .get("report")
                    .and_then(JsonValue::as_object)
                    .and_then(|envelope| envelope.get("data")),
                Some(&value)
            );
        }
        // An output named by both wrap_as and named_emits is one artifact,
        // wrapped once.
        let value = JsonValue::from_serialize(&serde_json::json!({"events": "present"}))
            .unwrap_or(JsonValue::Null);
        let both = artifacts(
            Some("result"),
            &[("result", "runx.data.operation_result.v1")],
        );
        assert_eq!(
            project_artifact_outputs(&value, Some(&both))
                .get("result")
                .and_then(JsonValue::as_object)
                .and_then(|envelope| envelope.get("data")),
            Some(&value)
        );
    }
}
