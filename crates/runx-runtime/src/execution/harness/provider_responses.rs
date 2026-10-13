use runx_contracts::{JsonObject, JsonValue};
use runx_parser::{HarnessProviderAccess, HarnessProviderResponsesFixture};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::http::{
    HarnessExchanges, HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse,
    RuntimeHttpTransport,
};

pub(crate) const HARNESS_PROVIDER_BASE_URL: &str = "https://provider-fixture.runx.invalid";
pub(crate) const HARNESS_PROVIDER_TOKEN: &str = "runx-harness-provider-token";

/// The harness's transport beneath operation steps. It answers the provider
/// fixture's hosted endpoints, including the declared requests runx Connect
/// renders and sends, and the case's exact HTTP exchanges, and refuses every
/// other request: a harness run never reaches a live provider.
#[derive(Clone, Debug)]
pub(super) struct HarnessTransport {
    pub(super) provider: Option<HarnessProviderResponsesFixture>,
    /// The exact connector the fixture's grant is for, as runx Connect
    /// lists and executes it.
    pub(super) grant_connector: Option<Arc<crate::kernel::connectors::LoadedConnector>>,
    pub(super) exchanges: Arc<HarnessExchanges>,
    pub(super) mutation_keys: Arc<Mutex<BTreeMap<(String, String), String>>>,
}

impl RuntimeHttpTransport for HarnessTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if !request.url.starts_with(HARNESS_PROVIDER_BASE_URL) {
            return self.exchanges.answer(&request);
        }
        let fixture = self
            .provider
            .as_ref()
            .ok_or_else(|| transport_error("the case declares no provider fixture".to_owned()))?;
        validate_authorization(&request)?;
        match (request.method, request.url.as_str()) {
            (HttpMethod::Get, url) if url == format!("{HARNESS_PROVIDER_BASE_URL}/v1/me") => {
                json_response(JsonValue::Object(JsonObject::from([
                    ("status".to_owned(), JsonValue::String("success".to_owned())),
                    (
                        "principal".to_owned(),
                        JsonValue::Object(JsonObject::from([(
                            "principal_id".to_owned(),
                            JsonValue::String(fixture.principal_id.clone()),
                        )])),
                    ),
                ])))
            }
            (HttpMethod::Get, url) if url == format!("{HARNESS_PROVIDER_BASE_URL}/v1/grants") => {
                let binding = self
                    .grant_connector
                    .as_ref()
                    .map(|connector| connector.binding());
                let binding = binding.as_ref().ok_or_else(|| {
                    transport_error("the provider fixture's grant names no connector".to_owned())
                })?;
                let grant = serde_json::json!({
                    "grant_id": fixture.grant.grant_id,
                    "provider": fixture.grant.provider,
                    "connector_binding": binding,
                    "status": "active",
                    "scopes": fixture.grant.scopes,
                });
                json_response(
                    JsonValue::from_serialize(&serde_json::json!({
                        "status": "success",
                        "grants": [grant],
                    }))
                    .map_err(|error| transport_error(error.to_string()))?,
                )
            }
            (HttpMethod::Post, url)
                if url == format!("{HARNESS_PROVIDER_BASE_URL}/v1/provider-operations") =>
            {
                self.provider_operation(fixture, request.body.as_deref())
            }
            (HttpMethod::Post, url)
                if url == format!("{HARNESS_PROVIDER_BASE_URL}/v1/provider-requests") =>
            {
                self.provider_request(fixture, request.body.as_deref())
            }
            _ => Err(transport_error(format!(
                "the harness provider fixture has no response for {} {}",
                request.method.as_str(),
                request.url
            ))),
        }
    }
}

fn validate_authorization(request: &RuntimeHttpRequest) -> Result<(), RuntimeHttpError> {
    let expected = format!("Bearer {HARNESS_PROVIDER_TOKEN}");
    if request
        .headers
        .iter()
        .any(|header| header.name.eq_ignore_ascii_case("authorization") && header.value == expected)
    {
        return Ok(());
    }
    Err(transport_error(
        "the harness provider request omitted its isolated bearer binding".to_owned(),
    ))
}

impl HarnessTransport {
    fn provider_operation(
        &self,
        fixture: &HarnessProviderResponsesFixture,
        body: Option<&str>,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        let request = serde_json::from_str::<JsonObject>(
            body.ok_or_else(|| transport_error("provider operation body is missing".to_owned()))?,
        )
        .map_err(|error| transport_error(format!("provider operation body is invalid: {error}")))?;
        let grant_id = required_string(&request, "grant_id")?;
        let operation = required_string(&request, "operation")?;
        let target = required_string(&request, "target")?;
        let access = required_string(&request, "access")?;
        let access = match access {
            "read" => HarnessProviderAccess::Read,
            "mutate" => HarnessProviderAccess::Mutate,
            value => {
                return Err(transport_error(format!(
                    "unsupported provider access {value:?}"
                )));
            }
        };
        if grant_id != fixture.grant.grant_id {
            return Err(transport_error(format!(
                "the harness provider fixture holds no grant {grant_id:?}"
            )));
        }
        let provider = fixture.grant.provider.clone();
        let fixture = fixture
            .operations
            .iter()
            .find(|fixture| {
                fixture.operation == operation
                    && fixture.target == target
                    && fixture.access == access
            })
            .ok_or_else(|| {
                transport_error(format!(
                    "the harness provider fixture has no exact {access:?} operation {operation:?} for target {target:?}"
                ))
            })?;
        let identity = runx_contracts::sha256_prefixed(
            format!("{grant_id}\0{operation}\0{target}").as_bytes(),
        );
        let idempotency_key = if access == HarnessProviderAccess::Mutate {
            let input = request
                .get("input")
                .and_then(JsonValue::as_object)
                .ok_or_else(|| transport_error("provider mutation input is missing".to_owned()))?;
            Some(required_string(input, "idempotency_key")?.to_owned())
        } else {
            None
        };
        let mut result = fixture.result.clone();
        if let (Some(idempotency_key), JsonValue::Object(result)) =
            (idempotency_key.as_deref(), &mut result)
            && result.get("idempotency_key").and_then(JsonValue::as_str)
                == Some("$request.idempotency_key")
        {
            result.insert(
                "idempotency_key".to_owned(),
                JsonValue::String(idempotency_key.to_owned()),
            );
        }
        if let JsonValue::Object(result) = &mut result
            && result.get("idempotency_key").and_then(JsonValue::as_str)
                == Some("$previous.idempotency_key")
        {
            let previous = self
                .mutation_keys
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&(grant_id.to_owned(), target.to_owned()))
                .cloned()
                .ok_or_else(|| {
                    transport_error(format!(
                        "the harness provider fixture has no prior mutation idempotency for grant {grant_id:?} and target {target:?}"
                    ))
                })?;
            result.insert("idempotency_key".to_owned(), JsonValue::String(previous));
        }
        let mut response = JsonObject::from([
            ("status".to_owned(), JsonValue::String("success".to_owned())),
            ("provider".to_owned(), JsonValue::String(provider)),
            (
                "operation".to_owned(),
                JsonValue::String(operation.to_owned()),
            ),
            ("target".to_owned(), JsonValue::String(target.to_owned())),
            (
                "access".to_owned(),
                JsonValue::String(access_name(access).to_owned()),
            ),
            ("result".to_owned(), result),
            (
                "readback_ref".to_owned(),
                JsonValue::String(format!("runx:harness:provider-readback:{identity}")),
            ),
        ]);
        if let Some(idempotency_key) = idempotency_key {
            self.mutation_keys
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    (grant_id.to_owned(), target.to_owned()),
                    idempotency_key.clone(),
                );
            response.insert(
                "operation_id".to_owned(),
                JsonValue::String(format!("harness-{identity}")),
            );
            response.insert(
                "idempotency_key".to_owned(),
                JsonValue::String(idempotency_key),
            );
        }
        json_response(JsonValue::Object(response))
    }
}

/// One declared request of a hosted operation, as the kernel sends it to
/// runx Connect.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredRequest {
    connector_binding: runx_contracts::connector::ConnectorBinding,
    grant_id: String,
    operation: String,
    operation_digest: String,
    implementation_digest: String,
    input: JsonObject,
    #[serde(default)]
    idempotency_key: Option<String>,
    request: String,
    #[serde(default)]
    item: Option<usize>,
    #[serde(default)]
    responses: Option<BTreeMap<String, JsonObject>>,
}

impl HarnessTransport {
    /// Send one declared request as runx Connect does: under the fixture's
    /// grant, for the exact connector it holds at the digests that connector
    /// declares, rendered by Connect's renderer with the case's paid
    /// invocation, and answered only by the case's exchanges. A refusal
    /// answers as Connect's does, before anything is sent.
    fn provider_request(
        &self,
        fixture: &HarnessProviderResponsesFixture,
        body: Option<&str>,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        let Ok(call) = serde_json::from_str::<DeclaredRequest>(body.unwrap_or_default()) else {
            return refused(400, "invalid_request", "the provider request is malformed");
        };
        if call.grant_id != fixture.grant.grant_id {
            return refused(
                404,
                "unknown-grant",
                format!(
                    "the harness provider fixture holds no grant {:?}",
                    call.grant_id
                ),
            );
        }
        let connector = self.grant_connector.as_ref().ok_or_else(|| {
            transport_error("the provider fixture's grant names no connector".to_owned())
        })?;
        let digests = crate::kernel::connectors::operation_digest_entries(
            &connector.manifest,
            &connector.operation_digests,
            |tool| connector.tools.get(tool).map(|tool| &tool.digest),
        )
        .map_err(transport_error)?;
        let declared = connector
            .operation(&call.operation)
            .zip(digests.get(call.operation.as_str()))
            .filter(|(_, digests)| {
                call.connector_binding == connector.binding()
                    && digests.operation.as_str() == call.operation_digest
                    && digests
                        .implementation
                        .as_ref()
                        .map(|digest| digest.as_str())
                        == Some(call.implementation_digest.as_str())
            });
        let Some(((_, operation, _), _)) = declared else {
            return refused(
                409,
                "operation_contract_mismatch",
                format!(
                    "{} operation '{}' is not the one granted; update runx.",
                    call.connector_binding.connector, call.operation
                ),
            );
        };
        let Some(spec) = operation.hosted_http() else {
            return refused(
                400,
                "operation_unsupported",
                format!("operation '{}' declares no hosted requests", call.operation),
            );
        };
        if operation.scopes.iter().any(|scope| {
            !fixture
                .grant
                .scopes
                .iter()
                .any(|granted| granted == scope.as_str())
        }) {
            return refused(
                400,
                "scope_mismatch",
                format!(
                    "grant {:?} does not cover operation '{}'",
                    call.grant_id, call.operation
                ),
            );
        }
        let checking = spec
            .preconditions
            .iter()
            .chain(&spec.readbacks)
            .any(|check| check.request.id.as_str() == call.request);
        if call.responses.is_some()
            && !(operation.access() == runx_contracts::connector::OperationAccess::Mutate
                && checking)
        {
            return refused(
                400,
                "invalid_input",
                "Earlier answers are accepted only for a write's checking reads.",
            );
        }
        let arguments = JsonValue::Object(call.input);
        let rendered = match crate::drivers::http::render_declared(
            spec,
            &crate::drivers::http::DeclaredCall {
                request: &call.request,
                arguments: &arguments,
                parameters: None,
                idempotency_key: call.idempotency_key.as_deref(),
                invocation_id: fixture.invocation_id.as_deref(),
                item: call.item,
                responses: call.responses.as_ref(),
            },
        ) {
            Ok(rendered) => rendered,
            Err(error) => return refused(400, "invalid_input", error),
        };
        let answer = self.exchanges.answer(&rendered)?;
        json_response(JsonValue::Object(JsonObject::from([
            (
                "status".to_owned(),
                JsonValue::Number(runx_contracts::JsonNumber::from(u64::from(answer.status))),
            ),
            ("body".to_owned(), JsonValue::String(answer.body)),
        ])))
    }
}

/// Connect's answer for a request it refuses before sending anything.
fn refused(
    status: u16,
    code: &str,
    error: impl Into<String>,
) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
    Ok(RuntimeHttpResponse::new(
        status,
        serde_json::json!({"error": error.into(), "code": code}).to_string(),
    ))
}

fn access_name(access: HarnessProviderAccess) -> &'static str {
    match access {
        HarnessProviderAccess::Read => "read",
        HarnessProviderAccess::Mutate => "mutate",
    }
}

fn required_string<'a>(object: &'a JsonObject, field: &str) -> Result<&'a str, RuntimeHttpError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| transport_error(format!("provider operation {field} is missing")))
}

fn json_response(value: JsonValue) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
    serde_json::to_string(&value)
        .map(|body| RuntimeHttpResponse::new(200, body))
        .map_err(|error| transport_error(format!("harness provider response failed: {error}")))
}

fn transport_error(message: String) -> RuntimeHttpError {
    RuntimeHttpError::Transport { message }
}
