//! Pure parsing and validation for conventional package harness fixtures.

use std::collections::BTreeMap;

use runx_contracts::{JsonObject, JsonValue, ReceiptSchema, RunOutcome, SealReason};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{ParseError, parse_yaml_document};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestFixtureKind {
    Skill,
    Graph,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestExpectedStatus {
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
    pub exercises_method: Option<String>,
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
pub struct TestHttpResponseFixture {
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
pub struct TestHttpRequestFixture {
    pub method: String,
    pub url: String,
    pub body: TestHttpRequestBodyFixture,
}

/// Exact body identity for a request-sensitive harness exchange. `none` is an
/// absent body; `{ json: ... }` is one structural JSON value, including null;
/// `{ text: ... }` is an exact UTF-8 wire body, including a form or XML.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum TestHttpRequestBodyFixture {
    None(String),
    Json { json: JsonValue },
    Text { text: String },
}

/// One tool call the harness answers in place of an MCP server: the exact
/// tool, arguments and session headers a call renders, and the result the
/// server gives (`content`, `structuredContent`, `isError`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestMcpCallFixture {
    pub tool: String,
    #[serde(default)]
    pub arguments: JsonObject,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    pub result: JsonObject,
}

/// One exact connector process-tool call answered by the native harness.
/// This fixture substitutes a result; it never starts the tool's host process.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestProcessCallFixture {
    pub tool: String,
    #[serde(default)]
    pub arguments: JsonObject,
    pub result: JsonValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestHttpExchangeFixture {
    pub request: TestHttpRequestFixture,
    pub response: TestHttpResponseFixture,
}

/// One hosted-provider grant exposed only to deterministic harness execution.
/// This is authority-shaped test evidence, not a production credential or a
/// public runner input.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestProviderGrantFixture {
    pub grant_id: String,
    pub provider: String,
    pub scopes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestProviderAccess {
    Read,
    Mutate,
}

/// One exact provider operation and readback result admitted by the harness.
/// The runtime still performs its normal admission, approval, custody,
/// finality, and receipt transitions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestProviderOperationFixture {
    pub operation: String,
    pub target: String,
    pub access: TestProviderAccess,
    pub result: JsonValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestProviderResponsesFixture {
    pub principal_id: String,
    /// The grant the case's hosted instance is confirmed with. A case that
    /// acts only on what purchases hold names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant: Option<TestProviderGrantFixture>,
    /// The provider operations Connect answers. A case whose operations are
    /// refused before any request leaves declares none.
    #[serde(default)]
    pub operations: Vec<TestProviderOperationFixture>,
    /// The purchase Connect holds for the case's run, which a declared
    /// request sends as `{"$runx": "purchase"}`. Without one such a request
    /// fails closed, as it does for a run no purchase is bound to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_id: Option<String>,
    /// What earlier purchases hold for the principal: Connect presents a hold
    /// only to an operation on that resource, and refuses one the principal
    /// does not hold.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<TestProviderLeaseFixture>,
}

/// One resource a purchase holds for the harness principal: the seller's
/// connector, the purchase credential that holds it, and the resource's
/// target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestProviderLeaseFixture {
    pub connector: String,
    pub credential: String,
    pub target: String,
}

/// An agent turn the caller works: the calls it makes in order, then the
/// output it ends the turn with.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestTurnCalls {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub calls: Vec<TestTurnCall>,
    pub output: JsonObject,
}

/// One call a scripted turn makes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestTurnCall {
    /// The tool the turn offers for the call, by the name its status lists.
    pub tool: String,
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub input: JsonObject,
    /// What the call must return: its result, matched exactly or as a subset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expect: Option<TestJsonExpectation>,
}

/// What the caller stands in for during a harness run: the agent acts it
/// answers or works, the approval gates it decides, and the world the run
/// reaches. Fixture files, inline runner cases and dev fixtures share this
/// one shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestCaller {
    /// The act the caller states for the run, which its steps read as
    /// `$act`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act: Option<runx_contracts::CallAct>,
    /// Agent act outputs, by request id.
    #[serde(default, skip_serializing_if = "JsonObject::is_empty")]
    pub answers: JsonObject,
    /// Agent turns the caller works call by call, by request id. An act is
    /// answered or worked, not both.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub turns: BTreeMap<String, TestTurnCalls>,
    /// Approval decisions, by the address of the request each decides: an
    /// operation step's id or an ask's gate at the root, `<child>#<id>` below.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub approvals: BTreeMap<String, bool>,
    /// The exact HTTP exchanges the run may make; any other request is
    /// refused, never sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub http_exchanges: Vec<TestHttpExchangeFixture>,
    /// The MCP tool calls the run may make.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_calls: Vec<TestMcpCallFixture>,
    /// Exact local connector process-tool results. Unmatched calls fail closed;
    /// they never fall through to a real process when these fixtures are set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub process_calls: Vec<TestProcessCallFixture>,
    /// The hosted provider the run's operations reach.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_responses: Option<TestProviderResponsesFixture>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TestFixture {
    pub name: String,
    pub kind: TestFixtureKind,
    pub target: String,
    pub runner: Option<String>,
    pub setup: TestSetup,
    pub inputs: JsonObject,
    pub env: BTreeMap<String, String>,
    pub caller: TestCaller,
    pub expect: TestExpectation,
    pub operator_journeys: Vec<OperatorJourneyClaim>,
    pub metadata: JsonObject,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestSetup {
    /// Parameters pinned when the fixture confirms a session or credential instance.
    #[serde(default)]
    pub parameters: BTreeMap<String, JsonObject>,
    #[serde(default)]
    pub receipts: Vec<String>,
    /// Connectors whose local session the case confirms and selects before
    /// it runs, the way `runx connector use <connector> --session` would.
    /// Each is named as the skill's `connectors:` block keys it: a connector
    /// id, or a qualified `<publisher>/<id>` ref.
    #[serde(default)]
    pub sessions: Vec<String>,
    /// Connectors whose credential the case stores from its environment and
    /// selects, the way `runx connector use --credential` would: the one
    /// declared credential whose variable the case sets. Named as `sessions`
    /// names them.
    #[serde(default)]
    pub credentials: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TestExpectation {
    pub status: Option<TestExpectedStatus>,
    pub receipt: Option<ReceiptExpectation>,
    /// The steps that ran, in order, asserted when present.
    pub steps: Option<Vec<String>>,
    pub output: Option<TestJsonExpectation>,
    pub step_outputs: BTreeMap<String, TestJsonExpectation>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestJsonExpectation {
    pub exact: Option<JsonValue>,
    pub subset: Option<JsonValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReceiptExpectation {
    pub schema: ReceiptSchema,
    pub body_digest: Option<String>,
    pub receipt_id: Option<String>,
    pub receipt_digest: Option<String>,
    pub subject_uri: Option<String>,
    pub state: Option<String>,
    pub outcome: Option<RunOutcome>,
    pub reason: Option<SealReason>,
    /// Each list is asserted when present, an empty one included: `[]`
    /// asserts there is none.
    pub act_ids: Option<Vec<String>>,
    pub decision_ids: Option<Vec<String>>,
    pub child_receipt_uris: Option<Vec<String>>,
    pub child_receipt_count: Option<usize>,
    pub verification_uris: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTestFixture {
    name: String,
    kind: TestFixtureKind,
    #[serde(default)]
    target: Option<String>,
    runner: Option<String>,
    #[serde(default)]
    setup: TestSetup,
    #[serde(default)]
    inputs: JsonObject,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    caller: JsonObject,
    #[serde(default)]
    expect: RawTestExpectation,
    #[serde(default)]
    operator_journeys: Vec<OperatorJourneyClaim>,
    #[serde(default)]
    metadata: JsonObject,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTestExpectation {
    status: Option<TestExpectedStatus>,
    receipt: Option<RawReceiptExpectation>,
    steps: Option<Vec<String>>,
    output: Option<TestJsonExpectation>,
    #[serde(default)]
    step_outputs: BTreeMap<String, TestJsonExpectation>,
}

#[derive(Debug, Deserialize)]
struct RawReceiptExpectation {
    #[serde(default = "default_receipt_schema")]
    schema: ReceiptSchema,
    body_digest: Option<String>,
    receipt_id: Option<String>,
    receipt_digest: Option<String>,
    subject_uri: Option<String>,
    state: Option<String>,
    outcome: Option<RunOutcome>,
    reason: Option<SealReason>,
    act_ids: Option<Vec<String>>,
    decision_ids: Option<Vec<String>>,
    child_receipt_uris: Option<Vec<String>>,
    child_receipt_count: Option<usize>,
    verification_uris: Option<Vec<String>>,
    #[serde(flatten)]
    extra: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(Debug, Error)]
pub enum TestFixtureError {
    #[error("failed to parse harness fixture YAML: {0}")]
    Parse(#[from] ParseError),
    #[error("harness fixture {field} is required")]
    Required { field: String },
    #[error("harness fixture {field} must not be empty")]
    Empty { field: &'static str },
    #[error("invalid harness fixture {field}: {message}")]
    Invalid { field: String, message: String },
    #[error("unknown receipt expectation field {field_path}")]
    UnknownReceiptField { field_path: String },
}

pub fn parse_test_fixture(contents: &str) -> Result<TestFixture, TestFixtureError> {
    validate_fixture(parse_yaml_document::<RawTestFixture>(contents)?)
}

pub(crate) fn parse_operator_journey(
    value: JsonObject,
) -> Result<OperatorJourneyClaim, TestFixtureError> {
    let value = serde_json::to_value(value).map_err(|error| TestFixtureError::Invalid {
        field: "operator_journey".to_owned(),
        message: error.to_string(),
    })?;
    let claim = serde_json::from_value(value).map_err(|error| TestFixtureError::Invalid {
        field: "operator_journey".to_owned(),
        message: error.to_string(),
    })?;
    validate_operator_journey(claim)
}

/// Validate an inline method-test expectation through the same contract as
/// a conventional fixture. Inline `run.yaml` cases and standalone fixture files
/// must not acquire parallel expectation vocabularies.
pub fn parse_test_expectation(value: JsonObject) -> Result<TestExpectation, TestFixtureError> {
    let value = serde_json::to_value(value).map_err(|error| TestFixtureError::Invalid {
        field: "expect".to_owned(),
        message: error.to_string(),
    })?;
    let expectation = serde_json::from_value::<RawTestExpectation>(value).map_err(|error| {
        TestFixtureError::Invalid {
            field: "expect".to_owned(),
            message: error.to_string(),
        }
    })?;
    validate_expectation(expectation)
}

/// Validate a caller through the same contract wherever it is written.
pub fn parse_test_caller(value: JsonObject) -> Result<TestCaller, TestFixtureError> {
    let caller = JsonValue::Object(value)
        .deserialize_into::<TestCaller>()
        .map_err(|error| TestFixtureError::Invalid {
            field: "caller".to_owned(),
            message: error.to_string(),
        })?;
    validate_caller(caller)
}

fn validate_caller(caller: TestCaller) -> Result<TestCaller, TestFixtureError> {
    for (id, script) in &caller.turns {
        if caller.answers.contains_key(id) {
            return Err(TestFixtureError::Invalid {
                field: format!("caller.turns.{id}"),
                message: "an act is answered or worked, not both".to_owned(),
            });
        }
        for (index, call) in script.calls.iter().enumerate() {
            if call.tool.trim().is_empty() {
                return Err(TestFixtureError::Invalid {
                    field: format!("caller.turns.{id}.calls[{index}].tool"),
                    message: "must name a tool the turn offers".to_owned(),
                });
            }
        }
    }
    Ok(TestCaller {
        http_exchanges: validate_http_exchanges(caller.http_exchanges, "caller.http_exchanges")?,
        mcp_calls: validate_mcp_calls(caller.mcp_calls, "caller.mcp_calls")?,
        process_calls: validate_process_calls(caller.process_calls)?,
        provider_responses: caller
            .provider_responses
            .map(|fixture| validate_test_provider_responses(fixture, "caller.provider_responses"))
            .transpose()?,
        ..caller
    })
}

fn validate_fixture(fixture: RawTestFixture) -> Result<TestFixture, TestFixtureError> {
    require_non_empty(&fixture.name, "name")?;
    let target = fixture.target.unwrap_or_default();
    require_non_empty(&target, "target")?;
    if let Some(runner) = &fixture.runner {
        require_non_empty(runner, "runner")?;
    }
    Ok(TestFixture {
        name: fixture.name,
        kind: fixture.kind,
        target,
        runner: fixture.runner,
        setup: validate_setup(fixture.setup)?,
        inputs: fixture.inputs,
        env: fixture.env,
        caller: parse_test_caller(fixture.caller)?,
        expect: validate_expectation(fixture.expect)?,
        operator_journeys: fixture
            .operator_journeys
            .into_iter()
            .map(validate_operator_journey)
            .collect::<Result<_, _>>()?,
        metadata: fixture.metadata,
    })
}

fn validate_test_provider_responses(
    fixture: TestProviderResponsesFixture,
    field: &str,
) -> Result<TestProviderResponsesFixture, TestFixtureError> {
    use std::collections::BTreeSet;

    const MAX_OPERATIONS: usize = 32;
    let invalid = |field: String, message: &str| TestFixtureError::Invalid {
        field,
        message: message.to_owned(),
    };
    if fixture.principal_id.trim().is_empty() || fixture.principal_id.len() > 256 {
        return Err(invalid(
            format!("{field}.principal_id"),
            "must be a non-empty principal id no longer than 256 characters",
        ));
    }
    if let Some(grant) = &fixture.grant
        && (grant.grant_id.trim().is_empty()
            || grant.provider.trim().is_empty()
            || grant.scopes.is_empty()
            || grant.scopes.iter().any(|scope| scope.trim().is_empty()))
    {
        return Err(invalid(
            format!("{field}.grant"),
            "grant_id, provider, and every declared scope must be non-empty",
        ));
    }
    if fixture.holds.iter().any(|hold| {
        hold.connector.trim().is_empty()
            || hold.credential.trim().is_empty()
            || hold.target.trim().is_empty()
    }) {
        return Err(invalid(
            format!("{field}.holds"),
            "each hold names its connector, credential and target",
        ));
    }
    if fixture.operations.len() > MAX_OPERATIONS {
        return Err(invalid(
            format!("{field}.operations"),
            "must contain at most 32 operations",
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

fn validate_mcp_calls(
    calls: Vec<TestMcpCallFixture>,
    field: &str,
) -> Result<Vec<TestMcpCallFixture>, TestFixtureError> {
    const MAX_CALLS: usize = 32;
    let invalid = |message: String| TestFixtureError::Invalid {
        field: field.to_owned(),
        message,
    };
    if calls.len() > MAX_CALLS {
        return Err(invalid(format!("must contain at most {MAX_CALLS} calls")));
    }
    if calls.iter().any(|call| call.tool.trim().is_empty()) {
        return Err(invalid("every call names its tool".to_owned()));
    }
    Ok(calls)
}

fn validate_process_calls(
    calls: Vec<TestProcessCallFixture>,
) -> Result<Vec<TestProcessCallFixture>, TestFixtureError> {
    let invalid = |message: &str| TestFixtureError::Invalid {
        field: "caller.process_calls".to_owned(),
        message: message.to_owned(),
    };
    if calls.len() > 32 {
        return Err(invalid("must contain at most 32 calls"));
    }
    for (index, call) in calls.iter().enumerate() {
        if call.tool.trim().is_empty() || call.tool.len() > 256 {
            return Err(invalid("every call names a tool of at most 256 bytes"));
        }
        if serde_json::to_vec(&call.arguments).map_or(true, |bytes| bytes.len() > 65536)
            || serde_json::to_vec(&call.result).map_or(true, |bytes| bytes.len() > 2 * 1024 * 1024)
        {
            return Err(invalid(
                "call arguments exceed 64 KiB or its result exceeds 2 MiB",
            ));
        }
        if calls[..index]
            .iter()
            .any(|prior| prior.tool == call.tool && prior.arguments == call.arguments)
        {
            return Err(invalid("duplicates an earlier tool and argument tuple"));
        }
    }
    Ok(calls)
}

/// Exact exchanges: the method, URL and JSON body identify each request, so
/// an exchange may answer a mutation without the harness reaching the network.
/// A request declared more than once is answered by its exchanges in order.
fn validate_http_exchanges(
    mut exchanges: Vec<TestHttpExchangeFixture>,
    field: &str,
) -> Result<Vec<TestHttpExchangeFixture>, TestFixtureError> {
    const MAX_EXCHANGES: usize = 32;
    const MAX_REQUEST_BODY_BYTES: usize = 1_048_576;
    if exchanges.len() > MAX_EXCHANGES {
        return Err(TestFixtureError::Invalid {
            field: field.to_owned(),
            message: format!("must contain at most {MAX_EXCHANGES} exchanges"),
        });
    }
    for (index, exchange) in exchanges.iter_mut().enumerate() {
        let request_field = format!("{field}[{index}].request");
        if !matches!(
            exchange.request.method.as_str(),
            "GET" | "POST" | "PUT" | "PATCH" | "DELETE"
        ) {
            return Err(TestFixtureError::Invalid {
                field: format!("{request_field}.method"),
                message: "must be GET, POST, PUT, PATCH, or DELETE".to_owned(),
            });
        }
        exchange.request.url =
            validate_test_http_url(&exchange.request.url, &format!("{request_field}.url"))?;
        match &exchange.request.body {
            TestHttpRequestBodyFixture::None(value) if value == "none" => {}
            TestHttpRequestBodyFixture::None(_) => {
                return Err(TestFixtureError::Invalid {
                    field: format!("{request_field}.body"),
                    message: "must be `none` or an object with one `json` or `text` field"
                        .to_owned(),
                });
            }
            TestHttpRequestBodyFixture::Json { json } => {
                let encoded =
                    serde_json::to_string(json).map_err(|error| TestFixtureError::Invalid {
                        field: format!("{request_field}.body.json"),
                        message: error.to_string(),
                    })?;
                if encoded.len() > MAX_REQUEST_BODY_BYTES {
                    return Err(TestFixtureError::Invalid {
                        field: format!("{request_field}.body.json"),
                        message: format!(
                            "must be no larger than {MAX_REQUEST_BODY_BYTES} UTF-8 bytes"
                        ),
                    });
                }
            }
            TestHttpRequestBodyFixture::Text { text } => {
                if text.len() > MAX_REQUEST_BODY_BYTES {
                    return Err(TestFixtureError::Invalid {
                        field: format!("{request_field}.body.text"),
                        message: format!(
                            "must be no larger than {MAX_REQUEST_BODY_BYTES} UTF-8 bytes"
                        ),
                    });
                }
            }
        }
        validate_test_http_response(&exchange.response, &format!("{field}[{index}].response"))?;
    }
    Ok(exchanges)
}

fn validate_test_http_url(url: &str, field: &str) -> Result<String, TestFixtureError> {
    if url.chars().count() > 2048
        || url
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(invalid_test_http_url(field));
    }
    let Ok(parsed) = url::Url::parse(url) else {
        return Err(invalid_test_http_url(field));
    };
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid_test_http_url(field));
    }
    Ok(parsed.to_string())
}

fn invalid_test_http_url(field: &str) -> TestFixtureError {
    TestFixtureError::Invalid {
        field: field.to_owned(),
        message: "must be an exact absolute HTTP(S) URL no longer than 2048 characters".to_owned(),
    }
}

fn validate_test_http_response(
    response: &TestHttpResponseFixture,
    field: &str,
) -> Result<(), TestFixtureError> {
    const MAX_BODY_BYTES: usize = 1_048_576;
    const MAX_HEADERS: usize = 64;
    if !(100..=599).contains(&response.status) {
        return Err(TestFixtureError::Invalid {
            field: format!("{field}.status"),
            message: "must be an HTTP status from 100 through 599".to_owned(),
        });
    }
    if response.headers.len() > MAX_HEADERS {
        return Err(TestFixtureError::Invalid {
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
        return Err(TestFixtureError::Invalid {
            field: format!("{field}.headers"),
            message: "header names and values must be bounded single-line strings".to_owned(),
        });
    }
    if response.body.len() > MAX_BODY_BYTES {
        return Err(TestFixtureError::Invalid {
            field: format!("{field}.body"),
            message: format!("must be no larger than {MAX_BODY_BYTES} UTF-8 bytes"),
        });
    }
    Ok(())
}

fn validate_operator_journey(
    claim: OperatorJourneyClaim,
) -> Result<OperatorJourneyClaim, TestFixtureError> {
    for (field, value) in [
        ("operator_journey.request", claim.request.as_str()),
        (
            "operator_journey.expected_outcome",
            claim.expected_outcome.as_str(),
        ),
    ] {
        if value.trim().is_empty() {
            return Err(TestFixtureError::Required {
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
        return Err(TestFixtureError::Invalid {
            field: "operator_journey".to_owned(),
            message: "confusors, prior_evidence, and must_not_repeat entries must not be empty"
                .to_owned(),
        });
    }
    if claim
        .exercises_method
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(TestFixtureError::Invalid {
            field: "operator_journey.exercises_method".to_owned(),
            message: "must be a non-empty runner name when declared".to_owned(),
        });
    }
    if claim.mode == OperatorJourneyMode::Composed
        && (claim.prior_evidence.is_empty() || claim.must_not_repeat.is_empty())
    {
        return Err(TestFixtureError::Invalid {
            field: "operator_journey".to_owned(),
            message: "composed journeys require prior_evidence and must_not_repeat assertions"
                .to_owned(),
        });
    }
    Ok(claim)
}

fn validate_setup(setup: TestSetup) -> Result<TestSetup, TestFixtureError> {
    const MAX_RECEIPTS: usize = 32;
    if setup.receipts.len() > MAX_RECEIPTS {
        return Err(TestFixtureError::Invalid {
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
            return Err(TestFixtureError::Invalid {
                field,
                message: "must be a normalized package-relative .json path".to_owned(),
            });
        }
        if !seen.insert(path.clone()) {
            return Err(TestFixtureError::Invalid {
                field,
                message: "must not duplicate another receipt path".to_owned(),
            });
        }
    }
    for connector in setup.parameters.keys() {
        if !setup.sessions.contains(connector) && !setup.credentials.contains(connector) {
            return Err(TestFixtureError::Invalid {
                field: "setup.parameters".to_owned(),
                message: "must name an explicitly configured session or credential".to_owned(),
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
        if !runx_contracts::connector::is_connector_name(connector) {
            return Err(TestFixtureError::Invalid {
                field,
                message: "must name a connector".to_owned(),
            });
        }
        if !connectors.insert(connector) {
            return Err(TestFixtureError::Invalid {
                field,
                message: "names a connector twice".to_owned(),
            });
        }
    }
    Ok(setup)
}

fn validate_expectation(
    expectation: RawTestExpectation,
) -> Result<TestExpectation, TestFixtureError> {
    Ok(TestExpectation {
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
    expectation: TestJsonExpectation,
    field: &str,
) -> Result<TestJsonExpectation, TestFixtureError> {
    if expectation.exact.is_none() && expectation.subset.is_none() {
        return Err(TestFixtureError::Required {
            field: format!("{field}.exact or {field}.subset"),
        });
    }
    Ok(expectation)
}

fn validate_receipt_expectation(
    receipt: RawReceiptExpectation,
) -> Result<ReceiptExpectation, TestFixtureError> {
    if let Some(field) = receipt.extra.keys().next() {
        return Err(TestFixtureError::UnknownReceiptField {
            field_path: format!("expect.receipt.{field}"),
        });
    }
    Ok(ReceiptExpectation {
        schema: receipt.schema,
        body_digest: receipt.body_digest,
        receipt_id: receipt.receipt_id,
        receipt_digest: receipt.receipt_digest,
        subject_uri: receipt.subject_uri,
        state: receipt.state,
        outcome: receipt.outcome,
        reason: receipt.reason,
        act_ids: receipt.act_ids,
        decision_ids: receipt.decision_ids,
        child_receipt_uris: receipt.child_receipt_uris,
        child_receipt_count: receipt.child_receipt_count,
        verification_uris: receipt.verification_uris,
    })
}

fn require_non_empty(value: &str, field: &'static str) -> Result<(), TestFixtureError> {
    if value.is_empty() {
        Err(TestFixtureError::Empty { field })
    } else {
        Ok(())
    }
}

fn default_receipt_schema() -> ReceiptSchema {
    ReceiptSchema::V1
}

#[cfg(test)]
mod tests;
