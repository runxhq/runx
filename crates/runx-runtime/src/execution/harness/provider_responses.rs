use runx_contracts::{JsonObject, JsonValue};
use runx_parser::{HarnessProviderAccess, HarnessProviderResponsesFixture};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::http::{
    HttpMethod, RuntimeHarnessHttpExchange, RuntimeHttpError, RuntimeHttpRequest,
    RuntimeHttpResponse, RuntimeHttpTransport, exact_harness_response,
};

pub(crate) const HARNESS_PROVIDER_BASE_URL: &str = "https://provider-fixture.runx.invalid";
pub(crate) const HARNESS_PROVIDER_TOKEN: &str = "runx-harness-provider-token";

/// The fixture's grant, injected the way a hosted worker injects one, so the
/// run resolves it as its instance.
pub(crate) fn injected_grant_env(
    fixture: &HarnessProviderResponsesFixture,
) -> Result<BTreeMap<String, String>, String> {
    let grant = &fixture.grant;
    let principal_id = runx_contracts::RunxPrincipalId::new(fixture.principal_id.clone())
        .ok_or("the fixture principal id is not a runx principal id")?;
    let principal_ref = runx_contracts::PrincipalReference::from_runx_principal_id(principal_id)
        .as_reference()
        .uri
        .as_str()
        .to_owned();
    Ok(BTreeMap::from([
        (
            crate::INJECTED_GRANT_ID_ENV.to_owned(),
            grant.grant_id.clone(),
        ),
        (
            crate::INJECTED_GRANTED_SCOPES_ENV.to_owned(),
            crate::encode_injected_scopes(&grant.scopes)
                .ok_or("the fixture grant needs at least one scope")?,
        ),
        (crate::INJECTED_PRINCIPAL_REF_ENV.to_owned(), principal_ref),
    ]))
}

/// The harness's transport beneath operation steps. It answers the provider
/// fixture's hosted endpoints and the case's exact HTTP exchanges, and refuses
/// every other request: a harness run never reaches a live provider.
#[derive(Clone, Debug)]
pub(super) struct HarnessTransport {
    pub(super) provider: Option<HarnessProviderResponsesFixture>,
    pub(super) responses: Option<Arc<BTreeMap<String, RuntimeHttpResponse>>>,
    pub(super) exchanges: Option<Arc<Vec<RuntimeHarnessHttpExchange>>>,
    pub(super) mutation_keys: Arc<Mutex<BTreeMap<(String, String), String>>>,
}

impl RuntimeHttpTransport for HarnessTransport {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if !request.url.starts_with(HARNESS_PROVIDER_BASE_URL) {
            return exact_harness_response(
                self.responses.as_deref(),
                self.exchanges.as_deref().map(Vec::as_slice),
                &request,
                false,
            );
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
            (HttpMethod::Post, url)
                if url == format!("{HARNESS_PROVIDER_BASE_URL}/v1/provider-operations") =>
            {
                self.provider_operation(fixture, request.body.as_deref())
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
