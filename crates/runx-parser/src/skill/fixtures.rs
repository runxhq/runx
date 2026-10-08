use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::ValidationError;
pub use crate::harness_fixture::{
    HarnessCaller, HarnessExpectation, HarnessHttpExchangeFixture, HarnessHttpRequestBodyFixture,
    HarnessHttpResponseFixture, HarnessMcpCallFixture, OperatorJourneyClaim, OperatorJourneyMode,
    ReceiptExpectation,
};

use super::FIELDS;

const RUNTIME_OWNED_HARNESS_ENV: &[&str] = &["RUNX_CWD", "RUNX_RECEIPT_DIR"];
const HARNESS_FIELDS: &[&str] = &["files", "cases"];
const HARNESS_CASE_FIELDS: &[&str] = &[
    "name",
    "runner",
    "inputs",
    "env",
    "caller",
    "expect",
    "operator_journeys",
];
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerHarnessCase {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runner: Option<String>,
    pub inputs: JsonObject,
    pub env: BTreeMap<String, String>,
    pub caller: HarnessCaller,
    pub expect: HarnessExpectation,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operator_journeys: Vec<OperatorJourneyClaim>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerHarnessManifest {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    pub cases: Vec<RunnerHarnessCase>,
}

pub(crate) fn validate_harness_manifest(
    value: Option<JsonObject>,
    field: &str,
) -> Result<Option<RunnerHarnessManifest>, ValidationError> {
    let Some(value) = value else {
        return Ok(None);
    };
    FIELDS.reject_unknown_fields(&value, field, HARNESS_FIELDS)?;
    let files = FIELDS
        .optional_string_array(value.get("files"), &format!("{field}.files"))?
        .unwrap_or_default();
    let cases = FIELDS
        .required_plain_array(value.get("cases"), &format!("{field}.cases"))?
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            validate_harness_case(
                FIELDS.required_object(Some(entry), &format!("{field}.cases[{index}]"))?,
                &format!("{field}.cases[{index}]"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(RunnerHarnessManifest { files, cases }))
}

fn validate_harness_case(
    value: &JsonObject,
    field: &str,
) -> Result<RunnerHarnessCase, ValidationError> {
    FIELDS.reject_unknown_fields(value, field, HARNESS_CASE_FIELDS)?;
    Ok(RunnerHarnessCase {
        name: FIELDS.required_string(value.get("name"), &format!("{field}.name"))?,
        runner: FIELDS
            .optional_non_empty_string(value.get("runner"), &format!("{field}.runner"))?,
        inputs: FIELDS
            .optional_object(value.get("inputs"), &format!("{field}.inputs"))?
            .unwrap_or_default(),
        env: validate_string_object(
            FIELDS
                .optional_object(value.get("env"), &format!("{field}.env"))?
                .unwrap_or_default(),
            &format!("{field}.env"),
        )?,
        caller: validate_harness_caller(
            FIELDS
                .optional_object(value.get("caller"), &format!("{field}.caller"))?
                .unwrap_or_default(),
            &format!("{field}.caller"),
        )?,
        expect: validate_harness_expectation(
            FIELDS.required_object(value.get("expect"), &format!("{field}.expect"))?,
            &format!("{field}.expect"),
        )?,
        operator_journeys: validate_operator_journeys(
            value.get("operator_journeys"),
            &format!("{field}.operator_journeys"),
        )?,
    })
}

fn validate_operator_journeys(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Vec<OperatorJourneyClaim>, ValidationError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    FIELDS
        .required_plain_array(Some(value), field)?
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let claim_field = format!("{field}[{index}]");
            let object = FIELDS.required_object(Some(value), &claim_field)?.clone();
            crate::harness_fixture::parse_operator_journey(object)
                .map_err(|error| FIELDS.validation_error(format!("{claim_field}: {error}")))
        })
        .collect()
}

fn validate_string_object(
    value: JsonObject,
    field: &str,
) -> Result<BTreeMap<String, String>, ValidationError> {
    value
        .into_iter()
        .map(|(key, value)| {
            if RUNTIME_OWNED_HARNESS_ENV.contains(&key.as_str()) {
                return Err(FIELDS.validation_error(format!(
                    "{field}.{key} is owned by the isolated harness runtime and cannot be overridden."
                )));
            }
            match value {
                JsonValue::String(value) => Ok((key, value)),
                _ => Err(FIELDS.validation_error(format!("{field}.{key} must be a string."))),
            }
        })
        .collect()
}

fn validate_harness_caller(
    value: JsonObject,
    field: &str,
) -> Result<HarnessCaller, ValidationError> {
    crate::harness_fixture::parse_harness_caller(value)
        .map_err(|error| FIELDS.validation_error(format!("{field}: {error}")))
}

fn validate_harness_expectation(
    value: &JsonObject,
    field: &str,
) -> Result<HarnessExpectation, ValidationError> {
    crate::harness_fixture::parse_harness_expectation(value.clone())
        .map_err(|error| FIELDS.validation_error(format!("{field}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::OperatorJourneyMode;
    use crate::{parse_runner_manifest_yaml, validate_runner_manifest};

    #[test]
    fn inline_harness_cannot_override_runtime_owned_isolation()
    -> Result<(), Box<dyn std::error::Error>> {
        for key in ["RUNX_CWD", "RUNX_RECEIPT_DIR"] {
            let raw = parse_runner_manifest_yaml(&format!(
                r#"
skill: fixture
harness:
  cases:
    - name: escape
      env:
        {key}: "."
      inputs: {{}}
      expect:
        status: sealed
runners:
  fixture:
    default: true
    result_from:
      - digest
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: fixture
"#
            ))?;
            let Err(error) = validate_runner_manifest(raw) else {
                return Err("runtime-owned harness environment was accepted".into());
            };

            assert!(error.to_string().contains("isolated harness runtime"));
        }
        Ok(())
    }

    #[test]
    fn inline_harness_projects_semantic_operator_journey_claims()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = parse_runner_manifest_yaml(
            r#"
skill: fixture
harness:
  cases:
    - name: reuses-prior-evidence
      inputs: {}
      operator_journeys:
        - mode: composed
          request: Continue from this prior evidence packet.
          expected_outcome: Return the bounded result without rediscovery.
          prior_evidence:
            - runx.fixture.evidence.v1
          must_not_repeat:
            - Do not fetch the source again.
      expect:
        status: sealed
runners:
  fixture:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: fixture
"#,
        )?;
        let manifest = validate_runner_manifest(raw)?;
        let claim = manifest
            .harness
            .as_ref()
            .and_then(|harness| harness.cases.first())
            .and_then(|case| case.operator_journeys.first())
            .ok_or("operator journey claim was not retained")?;
        assert_eq!(claim.mode, OperatorJourneyMode::Composed);
        assert_eq!(claim.prior_evidence, ["runx.fixture.evidence.v1"]);
        assert_eq!(claim.must_not_repeat, ["Do not fetch the source again."]);
        Ok(())
    }

    #[test]
    fn composed_operator_journey_requires_reuse_and_non_repetition_claims()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = parse_runner_manifest_yaml(
            r#"
skill: fixture
harness:
  cases:
    - name: empty-composition-claim
      inputs: {}
      operator_journeys:
        - mode: composed
          request: Continue this work.
          expected_outcome: Return a result.
      expect:
        status: sealed
runners:
  fixture:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: fixture
"#,
        )?;
        let Err(error) = validate_runner_manifest(raw) else {
            return Err("empty composed claim must fail".into());
        };
        assert!(
            error
                .to_string()
                .contains("prior_evidence and must_not_repeat")
        );
        Ok(())
    }

    #[test]
    fn inline_harness_accepts_request_sensitive_http_exchanges()
    -> Result<(), Box<dyn std::error::Error>> {
        let raw = parse_runner_manifest_yaml(
            r#"
skill: fixture
harness:
  cases:
    - name: exact-mcp
      inputs: {}
      caller:
        http_exchanges:
          - request:
              method: POST
              url: https://fixture.runx.invalid/mcp
              body:
                json: { operation: status }
            response:
              status: 200
              body: '{"ok":true}'
      expect:
        status: sealed
runners:
  fixture:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: fixture
"#,
        )?;
        let manifest = validate_runner_manifest(raw)?;
        let exchanges = &manifest
            .harness
            .as_ref()
            .and_then(|harness| harness.cases.first())
            .ok_or("inline case missing")?
            .caller
            .http_exchanges;
        assert_eq!(exchanges.len(), 1);
        assert_eq!(exchanges[0].request.method, "POST");
        Ok(())
    }

    #[test]
    fn inline_harness_http_exchanges_accept_get_and_delete_json_bodies()
    -> Result<(), Box<dyn std::error::Error>> {
        for method in ["GET", "DELETE"] {
            let yaml = format!(
                r#"
skill: fixture
harness:
  cases:
    - name: exact-method-body
      inputs: {{}}
      caller:
        http_exchanges:
          - request:
              method: {method}
              url: https://fixture.runx.invalid/source
              body:
                json: null
            response:
              status: 200
              body: matched
      expect:
        status: sealed
runners:
  fixture:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: fixture
"#,
            );
            let raw = parse_runner_manifest_yaml(&yaml)?;
            let manifest = validate_runner_manifest(raw)?;
            let exchanges = &manifest
                .harness
                .as_ref()
                .and_then(|harness| harness.cases.first())
                .ok_or("inline case missing")?
                .caller
                .http_exchanges;
            assert_eq!(exchanges[0].request.method, method);
        }
        Ok(())
    }

    #[test]
    fn inline_harness_http_exchanges_reject_credentials_and_fragments()
    -> Result<(), Box<dyn std::error::Error>> {
        for url in [
            "https://user:pass@fixture.runx.invalid/source",
            "https://fixture.runx.invalid/source#fragment",
        ] {
            let yaml = format!(
                r#"
skill: fixture
harness:
  cases:
    - name: unreachable-url
      inputs: {{}}
      caller:
        http_exchanges:
          - request:
              method: POST
              url: "{url}"
              body: none
            response:
              status: 200
              body: unreachable
      expect:
        status: sealed
runners:
  fixture:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: fixture
"#,
            );
            let raw = parse_runner_manifest_yaml(&yaml)?;
            let Err(error) = validate_runner_manifest(raw) else {
                return Err("inline credential and fragment URLs must fail validation".into());
            };
            assert!(error.to_string().contains("exact absolute HTTP(S) URL"));
        }
        Ok(())
    }
}
