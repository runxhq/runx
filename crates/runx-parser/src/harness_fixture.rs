//! Pure parsing and validation for conventional package harness fixtures.

use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{ClosureDisposition, JsonObject, JsonValue, ReceiptSchema};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ParseError, parse_yaml_document};

const RETIRED_RECEIPT_FIELDS: &[&str] =
    &["kind", "skill_name", "source_type", "graph_name", "owner"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessFixtureKind {
    Skill,
    Graph,
    Agent,
    #[serde(rename = "agent_task")]
    AgentStep,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessExpectedStatus {
    Sealed,
    Failure,
    NeedsAgent,
    PolicyDenied,
    Escalated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatorJourneyMode {
    Standalone,
    Composed,
    Refusal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperatorJourneyClaim {
    pub mode: OperatorJourneyMode,
    pub request: String,
    pub expected_outcome: String,
    /// For a complex graph harness, name the public runner the executable
    /// journey invokes. Root skill fixtures derive this from `runner` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exercises_runner: Option<String>,
    /// Nearby public skill identities a cold agent must distinguish from this
    /// journey. Selection proof is meaningless without plausible alternatives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub confusors: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prior_evidence: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub must_not_repeat: Vec<String>,
}

/// One exact HTTP response available only while the native harness executes.
/// Production skill inputs and environment never carry this contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessHttpResponseFixture {
    pub status: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

/// One exact HTTP request admitted only by deterministic harness execution.
/// Matching the method, URL, and JSON body lets a fixture distinguish multiple
/// MCP operations that share one endpoint without weakening live transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessHttpRequestFixture {
    pub method: String,
    pub url: String,
    pub body: HarnessHttpRequestBodyFixture,
}

/// Exact body identity for a request-sensitive harness exchange. `none` is an
/// absent body; `{ json: ... }` is one structural JSON value, including null.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum HarnessHttpRequestBodyFixture {
    None(String),
    Json { json: JsonValue },
}

/// One tool call the harness answers in place of an MCP server: the exact
/// tool, arguments and session headers a call renders, and the result the
/// server gives (`content`, `structuredContent`, `isError`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessMcpCallFixture {
    pub tool: String,
    #[serde(default)]
    pub arguments: JsonObject,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub result: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessHttpExchangeFixture {
    pub request: HarnessHttpRequestFixture,
    pub response: HarnessHttpResponseFixture,
}

/// One hosted-provider grant exposed only to deterministic harness execution.
/// This is authority-shaped test evidence, not a production credential or a
/// public runner input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessProviderGrantFixture {
    pub grant_id: String,
    pub provider: String,
    pub scopes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessProviderAccess {
    Read,
    Mutate,
}

/// One exact provider operation and readback result admitted by the harness.
/// The runtime still performs its normal admission, approval, custody,
/// finality, and receipt transitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessProviderOperationFixture {
    pub operation: String,
    pub target: String,
    pub access: HarnessProviderAccess,
    pub result: JsonValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessProviderResponsesFixture {
    pub principal_id: String,
    /// The grant the harness injects as the run's hosted instance.
    pub grant: HarnessProviderGrantFixture,
    pub operations: Vec<HarnessProviderOperationFixture>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarnessFixture {
    pub name: String,
    pub kind: HarnessFixtureKind,
    pub target: String,
    pub runner: Option<String>,
    pub setup: HarnessSetup,
    pub inputs: JsonObject,
    pub env: BTreeMap<String, String>,
    pub caller: JsonObject,
    pub expect: HarnessExpectation,
    pub operator_journeys: Vec<OperatorJourneyClaim>,
    pub metadata: JsonObject,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessSetup {
    #[serde(default)]
    pub receipts: Vec<String>,
    /// Connectors whose local session the case confirms and selects before
    /// it runs, the way `runx connector use <connector> --session` would.
    #[serde(default)]
    pub sessions: Vec<runx_contracts::connector::ConnectorId>,
    /// Connectors whose credential the case stores from its environment and
    /// selects, the way `runx credential set` would: the one declared
    /// credential whose variable the case sets.
    #[serde(default)]
    pub credentials: Vec<runx_contracts::connector::ConnectorId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HarnessExpectation {
    pub status: Option<HarnessExpectedStatus>,
    pub receipt: Option<ReceiptExpectation>,
    pub steps: Vec<String>,
    pub output: Option<HarnessJsonExpectation>,
    pub step_outputs: BTreeMap<String, HarnessJsonExpectation>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HarnessJsonExpectation {
    pub exact: Option<JsonValue>,
    pub subset: Option<JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReceiptExpectation {
    pub schema: ReceiptSchema,
    pub body_digest: Option<String>,
    pub receipt_id: Option<String>,
    pub receipt_digest: Option<String>,
    pub harness_id: Option<String>,
    pub state: Option<String>,
    pub disposition: Option<ClosureDisposition>,
    pub reason_code: Option<String>,
    pub act_ids: Vec<String>,
    pub decision_ids: Vec<String>,
    pub child_receipt_refs: Vec<String>,
    pub child_receipt_count: Option<usize>,
    pub verification_refs: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHarnessFixture {
    name: String,
    kind: HarnessFixtureKind,
    #[serde(default)]
    target: Option<String>,
    runner: Option<String>,
    #[serde(default)]
    setup: HarnessSetup,
    #[serde(default)]
    inputs: JsonObject,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    caller: JsonObject,
    #[serde(default)]
    expect: RawHarnessExpectation,
    #[serde(default)]
    operator_journeys: Vec<OperatorJourneyClaim>,
    #[serde(default)]
    metadata: JsonObject,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHarnessExpectation {
    status: Option<HarnessExpectedStatus>,
    receipt: Option<RawReceiptExpectation>,
    #[serde(default)]
    steps: Vec<String>,
    output: Option<HarnessJsonExpectation>,
    #[serde(default)]
    step_outputs: BTreeMap<String, HarnessJsonExpectation>,
}

#[derive(Debug, Deserialize)]
struct RawReceiptExpectation {
    #[serde(default = "default_receipt_schema")]
    schema: ReceiptSchema,
    body_digest: Option<String>,
    receipt_id: Option<String>,
    receipt_digest: Option<String>,
    harness_id: Option<String>,
    state: Option<String>,
    disposition: Option<ClosureDisposition>,
    reason_code: Option<String>,
    #[serde(default)]
    act_ids: Vec<String>,
    #[serde(default)]
    decision_ids: Vec<String>,
    #[serde(default)]
    child_receipt_refs: Vec<String>,
    child_receipt_count: Option<usize>,
    #[serde(default)]
    verification_refs: Vec<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(Debug, Error)]
pub enum HarnessFixtureError {
    #[error("failed to parse harness fixture YAML: {0}")]
    Parse(#[from] ParseError),
    #[error("harness fixture {field} is required")]
    Required { field: String },
    #[error("harness fixture {field} must not be empty")]
    Empty { field: &'static str },
    #[error("invalid harness fixture {field}: {message}")]
    Invalid { field: String, message: String },
    #[error("retired receipt expectation field {field_path}")]
    RetiredReceiptField { field_path: String },
    #[error("unknown receipt expectation field {field_path}")]
    UnknownReceiptField { field_path: String },
}

pub fn parse_harness_fixture(contents: &str) -> Result<HarnessFixture, HarnessFixtureError> {
    validate_fixture(parse_yaml_document::<RawHarnessFixture>(contents)?)
}

pub(crate) fn parse_operator_journey(
    value: JsonObject,
) -> Result<OperatorJourneyClaim, HarnessFixtureError> {
    let value = serde_json::to_value(value).map_err(|error| HarnessFixtureError::Invalid {
        field: "operator_journey".to_owned(),
        message: error.to_string(),
    })?;
    let claim = serde_json::from_value(value).map_err(|error| HarnessFixtureError::Invalid {
        field: "operator_journey".to_owned(),
        message: error.to_string(),
    })?;
    validate_operator_journey(claim)
}

/// Validate an inline runner-harness expectation through the same contract as
/// a conventional fixture. Inline `X.yaml` cases and standalone fixture files
/// must not acquire parallel expectation vocabularies.
pub fn parse_harness_expectation(
    value: JsonObject,
) -> Result<HarnessExpectation, HarnessFixtureError> {
    let value = serde_json::to_value(value).map_err(|error| HarnessFixtureError::Invalid {
        field: "expect".to_owned(),
        message: error.to_string(),
    })?;
    let expectation = serde_json::from_value::<RawHarnessExpectation>(value).map_err(|error| {
        HarnessFixtureError::Invalid {
            field: "expect".to_owned(),
            message: error.to_string(),
        }
    })?;
    validate_expectation(expectation)
}

fn validate_fixture(fixture: RawHarnessFixture) -> Result<HarnessFixture, HarnessFixtureError> {
    require_non_empty(&fixture.name, "name")?;
    let target = fixture.target.unwrap_or_default();
    if !matches!(fixture.kind, HarnessFixtureKind::AgentStep) {
        require_non_empty(&target, "target")?;
    }
    if let Some(runner) = &fixture.runner {
        require_non_empty(runner, "runner")?;
    }
    if fixture.caller.contains_key("web_responses") {
        return Err(HarnessFixtureError::Invalid {
            field: "caller.web_responses".to_owned(),
            message: "was renamed to caller.http_responses".to_owned(),
        });
    }
    parse_harness_http_responses(
        fixture.caller.get("http_responses"),
        "caller.http_responses",
    )?;
    parse_harness_http_exchanges(
        fixture.caller.get("http_exchanges"),
        "caller.http_exchanges",
    )?;
    parse_harness_provider_responses(
        fixture.caller.get("provider_responses"),
        "caller.provider_responses",
    )?;
    parse_harness_mcp_calls(fixture.caller.get("mcp_calls"), "caller.mcp_calls")?;
    Ok(HarnessFixture {
        name: fixture.name,
        kind: fixture.kind,
        target,
        runner: fixture.runner,
        setup: validate_setup(fixture.setup)?,
        inputs: fixture.inputs,
        env: fixture.env,
        caller: fixture.caller,
        expect: validate_expectation(fixture.expect)?,
        operator_journeys: fixture
            .operator_journeys
            .into_iter()
            .map(validate_operator_journey)
            .collect::<Result<_, _>>()?,
        metadata: fixture.metadata,
    })
}

/// Parse the deterministic hosted-provider lane used by conventional harness
/// fixtures. The compact fixture describes authority and readback; the runtime
/// owns the wire protocol and every governance transition around it.
pub fn parse_harness_provider_responses(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Option<HarnessProviderResponsesFixture>, HarnessFixtureError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let fixture = value
        .clone()
        .deserialize_into::<HarnessProviderResponsesFixture>()
        .map_err(|error| HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: error.to_string(),
        })?;
    validate_harness_provider_responses(fixture, field).map(Some)
}

fn validate_harness_provider_responses(
    fixture: HarnessProviderResponsesFixture,
    field: &str,
) -> Result<HarnessProviderResponsesFixture, HarnessFixtureError> {
    use std::collections::BTreeSet;

    const MAX_OPERATIONS: usize = 32;
    let invalid = |field: String, message: &str| HarnessFixtureError::Invalid {
        field,
        message: message.to_owned(),
    };
    if fixture.principal_id.trim().is_empty() || fixture.principal_id.len() > 256 {
        return Err(invalid(
            format!("{field}.principal_id"),
            "must be a non-empty principal id no longer than 256 characters",
        ));
    }
    let grant = &fixture.grant;
    if grant.grant_id.trim().is_empty()
        || grant.provider.trim().is_empty()
        || grant.scopes.is_empty()
        || grant.scopes.iter().any(|scope| scope.trim().is_empty())
    {
        return Err(invalid(
            format!("{field}.grant"),
            "grant_id, provider, and every declared scope must be non-empty",
        ));
    }
    if fixture.operations.is_empty() || fixture.operations.len() > MAX_OPERATIONS {
        return Err(invalid(
            format!("{field}.operations"),
            "must contain between 1 and 32 operations",
        ));
    }
    let mut operations = BTreeSet::new();
    for (index, operation) in fixture.operations.iter().enumerate() {
        let operation_field = format!("{field}.operations[{index}]");
        if operation.operation.trim().is_empty() || operation.target.trim().is_empty() {
            return Err(invalid(
                operation_field,
                "operation and target must be non-empty",
            ));
        }
        let identity = (
            operation.operation.as_str(),
            operation.target.as_str(),
            operation.access,
        );
        if !operations.insert(identity) {
            return Err(invalid(
                operation_field,
                "duplicates an earlier operation, target, and access tuple",
            ));
        }
    }
    Ok(fixture)
}

/// Parse the deterministic HTTP-response lane shared by conventional and inline
/// harness fixtures. The caller owns exact request URLs; the runtime owns
/// transport selection and never falls through to the network when this map is
/// present.
pub fn parse_harness_http_responses(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<BTreeMap<String, HarnessHttpResponseFixture>, HarnessFixtureError> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let encoded = serde_json::to_value(value).map_err(|error| HarnessFixtureError::Invalid {
        field: field.to_owned(),
        message: error.to_string(),
    })?;
    let responses = serde_json::from_value::<BTreeMap<String, HarnessHttpResponseFixture>>(encoded)
        .map_err(|error| HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: error.to_string(),
        })?;
    if responses.is_empty() {
        return Err(HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: "must contain at least one exact response when declared".to_owned(),
        });
    }
    validate_harness_http_responses(responses, field)
}

/// Parse request-sensitive deterministic exchanges. Unlike URL-only response
/// fixtures, an exact exchange may admit a mutation because the harness proves
/// the complete outbound request and never reaches the network.
pub fn parse_harness_mcp_calls(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Vec<HarnessMcpCallFixture>, HarnessFixtureError> {
    const MAX_CALLS: usize = 32;
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let invalid = |message: String| HarnessFixtureError::Invalid {
        field: field.to_owned(),
        message,
    };
    let encoded = serde_json::to_value(value).map_err(|error| invalid(error.to_string()))?;
    let calls = serde_json::from_value::<Vec<HarnessMcpCallFixture>>(encoded)
        .map_err(|error| invalid(error.to_string()))?;
    if calls.is_empty() || calls.len() > MAX_CALLS {
        return Err(invalid(format!(
            "must contain one to {MAX_CALLS} exact calls when declared"
        )));
    }
    if calls.iter().any(|call| call.tool.trim().is_empty()) {
        return Err(invalid("every call names its tool".to_owned()));
    }
    Ok(calls)
}

pub fn parse_harness_http_exchanges(
    value: Option<&JsonValue>,
    field: &str,
) -> Result<Vec<HarnessHttpExchangeFixture>, HarnessFixtureError> {
    const MAX_EXCHANGES: usize = 32;
    const MAX_REQUEST_BODY_BYTES: usize = 1_048_576;
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let encoded = serde_json::to_value(value).map_err(|error| HarnessFixtureError::Invalid {
        field: field.to_owned(),
        message: error.to_string(),
    })?;
    let mut exchanges = serde_json::from_value::<Vec<HarnessHttpExchangeFixture>>(encoded)
        .map_err(|error| HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: error.to_string(),
        })?;
    if exchanges.is_empty() {
        return Err(HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: "must contain at least one exact exchange when declared".to_owned(),
        });
    }
    if exchanges.len() > MAX_EXCHANGES {
        return Err(HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: format!("must contain at most {MAX_EXCHANGES} exchanges"),
        });
    }
    let mut identities = BTreeSet::new();
    for (index, exchange) in exchanges.iter_mut().enumerate() {
        let request_field = format!("{field}[{index}].request");
        if !matches!(
            exchange.request.method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
        ) {
            return Err(HarnessFixtureError::Invalid {
                field: format!("{request_field}.method"),
                message: "must be GET, POST, PUT, PATCH, or DELETE".to_owned(),
            });
        }
        exchange.request.url =
            validate_harness_http_url(&exchange.request.url, &format!("{request_field}.url"))?;
        let body_identity = match &exchange.request.body {
            HarnessHttpRequestBodyFixture::None(value) if value == "none" => "none".to_owned(),
            HarnessHttpRequestBodyFixture::None(_) => {
                return Err(HarnessFixtureError::Invalid {
                    field: format!("{request_field}.body"),
                    message: "must be `none` or an object with one `json` field".to_owned(),
                });
            }
            HarnessHttpRequestBodyFixture::Json { json } => {
                let encoded =
                    serde_json::to_string(json).map_err(|error| HarnessFixtureError::Invalid {
                        field: format!("{request_field}.body.json"),
                        message: error.to_string(),
                    })?;
                if encoded.len() > MAX_REQUEST_BODY_BYTES {
                    return Err(HarnessFixtureError::Invalid {
                        field: format!("{request_field}.body.json"),
                        message: format!(
                            "must be no larger than {MAX_REQUEST_BODY_BYTES} UTF-8 bytes"
                        ),
                    });
                }
                format!("json:{encoded}")
            }
        };
        if !identities.insert((
            exchange.request.method.clone(),
            exchange.request.url.clone(),
            body_identity,
        )) {
            return Err(HarnessFixtureError::Invalid {
                field: request_field,
                message: "duplicates an earlier exact method, URL, and JSON body".to_owned(),
            });
        }
        validate_harness_http_response(&exchange.response, &format!("{field}[{index}].response"))?;
    }
    Ok(exchanges)
}

fn validate_harness_http_responses(
    responses: BTreeMap<String, HarnessHttpResponseFixture>,
    field: &str,
) -> Result<BTreeMap<String, HarnessHttpResponseFixture>, HarnessFixtureError> {
    const MAX_RESPONSES: usize = 32;
    if responses.len() > MAX_RESPONSES {
        return Err(HarnessFixtureError::Invalid {
            field: field.to_owned(),
            message: format!("must contain at most {MAX_RESPONSES} responses"),
        });
    }
    let mut normalized = BTreeMap::new();
    for (url, response) in responses {
        let response_field = format!("{field}.{url}");
        let url = validate_harness_http_url(&url, &response_field)?;
        validate_harness_http_response(&response, &response_field)?;
        if normalized.insert(url, response).is_some() {
            return Err(HarnessFixtureError::Invalid {
                field: response_field,
                message: "duplicates an earlier canonical final URL".to_owned(),
            });
        }
    }
    Ok(normalized)
}

fn validate_harness_http_url(url: &str, field: &str) -> Result<String, HarnessFixtureError> {
    if url.chars().count() > 2048
        || url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid_harness_http_url(field));
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return Err(invalid_harness_http_url(field));
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid_harness_http_url(field));
    }
    Ok(parsed.to_string())
}

fn invalid_harness_http_url(field: &str) -> HarnessFixtureError {
    HarnessFixtureError::Invalid {
        field: field.to_owned(),
        message: "must be an exact absolute HTTP(S) URL no longer than 2048 characters".to_owned(),
    }
}

fn validate_harness_http_response(
    response: &HarnessHttpResponseFixture,
    field: &str,
) -> Result<(), HarnessFixtureError> {
    const MAX_BODY_BYTES: usize = 1_048_576;
    const MAX_HEADERS: usize = 64;
    if !(100..=599).contains(&response.status) {
        return Err(HarnessFixtureError::Invalid {
            field: format!("{field}.status"),
            message: "must be an HTTP status from 100 through 599".to_owned(),
        });
    }
    if response.headers.len() > MAX_HEADERS {
        return Err(HarnessFixtureError::Invalid {
            field: format!("{field}.headers"),
            message: format!("must contain at most {MAX_HEADERS} headers"),
        });
    }
    if response.headers.iter().any(|(name, value)| {
        name.is_empty()
            || name.len() > 256
            || value.len() > 8192
            || name.contains('\r')
            || name.contains('\n')
            || value.contains('\r')
            || value.contains('\n')
    }) {
        return Err(HarnessFixtureError::Invalid {
            field: format!("{field}.headers"),
            message: "header names and values must be bounded single-line strings".to_owned(),
        });
    }
    if response.body.len() > MAX_BODY_BYTES {
        return Err(HarnessFixtureError::Invalid {
            field: format!("{field}.body"),
            message: format!("must be no larger than {MAX_BODY_BYTES} UTF-8 bytes"),
        });
    }
    Ok(())
}

fn validate_operator_journey(
    claim: OperatorJourneyClaim,
) -> Result<OperatorJourneyClaim, HarnessFixtureError> {
    for (field, value) in [
        ("operator_journey.request", claim.request.as_str()),
        (
            "operator_journey.expected_outcome",
            claim.expected_outcome.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(HarnessFixtureError::Required {
                field: field.to_owned(),
            });
        }
    }
    if claim
        .confusors
        .iter()
        .chain(&claim.prior_evidence)
        .chain(&claim.must_not_repeat)
        .any(|value| value.trim().is_empty())
    {
        return Err(HarnessFixtureError::Invalid {
            field: "operator_journey".to_owned(),
            message: "confusors, prior_evidence, and must_not_repeat entries must not be empty"
                .to_owned(),
        });
    }
    if claim
        .exercises_runner
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(HarnessFixtureError::Invalid {
            field: "operator_journey.exercises_runner".to_owned(),
            message: "must be a non-empty runner name when declared".to_owned(),
        });
    }
    if claim.mode == OperatorJourneyMode::Composed
        && (claim.prior_evidence.is_empty() || claim.must_not_repeat.is_empty())
    {
        return Err(HarnessFixtureError::Invalid {
            field: "operator_journey".to_owned(),
            message: "composed journeys require prior_evidence and must_not_repeat assertions"
                .to_owned(),
        });
    }
    Ok(claim)
}

fn validate_setup(setup: HarnessSetup) -> Result<HarnessSetup, HarnessFixtureError> {
    const MAX_RECEIPTS: usize = 32;
    if setup.receipts.len() > MAX_RECEIPTS {
        return Err(HarnessFixtureError::Invalid {
            field: "setup.receipts".to_owned(),
            message: format!("must contain at most {MAX_RECEIPTS} receipt files"),
        });
    }
    let mut seen = std::collections::BTreeSet::new();
    for (index, path) in setup.receipts.iter().enumerate() {
        let field = format!("setup.receipts[{index}]");
        if path.is_empty()
            || path.len() > 512
            || path.starts_with('/')
            || path.contains('\\')
            || !path.ends_with(".json")
            || path
                .split('/')
                .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
        {
            return Err(HarnessFixtureError::Invalid {
                field,
                message: "must be a normalized package-relative .json path".to_owned(),
            });
        }
        if !seen.insert(path.clone()) {
            return Err(HarnessFixtureError::Invalid {
                field,
                message: "must not duplicate another receipt path".to_owned(),
            });
        }
    }
    // A connector runs under one selected instance: a session or a
    // credential, named once.
    let mut connectors = std::collections::BTreeSet::new();
    for (field, connector) in setup
        .sessions
        .iter()
        .enumerate()
        .map(|(index, connector)| (format!("setup.sessions[{index}]"), connector))
        .chain(
            setup
                .credentials
                .iter()
                .enumerate()
                .map(|(index, connector)| (format!("setup.credentials[{index}]"), connector)),
        )
    {
        if !connectors.insert(connector) {
            return Err(HarnessFixtureError::Invalid {
                field,
                message: "names a connector twice".to_owned(),
            });
        }
    }
    Ok(setup)
}

fn validate_expectation(
    expectation: RawHarnessExpectation,
) -> Result<HarnessExpectation, HarnessFixtureError> {
    Ok(HarnessExpectation {
        status: expectation.status,
        receipt: expectation
            .receipt
            .map(validate_receipt_expectation)
            .transpose()?,
        steps: expectation.steps,
        output: expectation
            .output
            .map(|expectation| validate_json_expectation(expectation, "expect.output"))
            .transpose()?,
        step_outputs: expectation
            .step_outputs
            .into_iter()
            .map(|(step_id, expectation)| {
                let field = format!("expect.step_outputs.{step_id}");
                validate_json_expectation(expectation, &field)
                    .map(|expectation| (step_id, expectation))
            })
            .collect::<Result<_, _>>()?,
    })
}

fn validate_json_expectation(
    expectation: HarnessJsonExpectation,
    field: &str,
) -> Result<HarnessJsonExpectation, HarnessFixtureError> {
    if expectation.exact.is_none() && expectation.subset.is_none() {
        return Err(HarnessFixtureError::Required {
            field: format!("{field}.exact or {field}.subset"),
        });
    }
    Ok(expectation)
}

fn validate_receipt_expectation(
    receipt: RawReceiptExpectation,
) -> Result<ReceiptExpectation, HarnessFixtureError> {
    if let Some(field) = receipt.extra.keys().next() {
        let field_path = format!("expect.receipt.{field}");
        if is_retired_receipt_field(field) {
            return Err(HarnessFixtureError::RetiredReceiptField { field_path });
        }
        return Err(HarnessFixtureError::UnknownReceiptField { field_path });
    }
    Ok(ReceiptExpectation {
        schema: receipt.schema,
        body_digest: receipt.body_digest,
        receipt_id: receipt.receipt_id,
        receipt_digest: receipt.receipt_digest,
        harness_id: receipt.harness_id,
        state: receipt.state,
        disposition: receipt.disposition,
        reason_code: receipt.reason_code,
        act_ids: receipt.act_ids,
        decision_ids: receipt.decision_ids,
        child_receipt_refs: receipt.child_receipt_refs,
        child_receipt_count: receipt.child_receipt_count,
        verification_refs: receipt.verification_refs,
    })
}

fn is_retired_receipt_field(field: &str) -> bool {
    RETIRED_RECEIPT_FIELDS.contains(&field)
        || field == retired_execution_receipt_field("skill")
        || field == retired_execution_receipt_field("graph")
}

fn retired_execution_receipt_field(prefix: &str) -> String {
    format!("{prefix}_{}", "execution")
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), HarnessFixtureError> {
    if value.is_empty() {
        Err(HarnessFixtureError::Empty { field })
    } else {
        Ok(())
    }
}

fn default_receipt_schema() -> ReceiptSchema {
    ReceiptSchema::V1
}

#[cfg(test)]
mod tests;
