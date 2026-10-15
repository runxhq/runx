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
    /// The exact connectors the fixture's holds are of.
    pub(super) held_connectors: Vec<Arc<crate::kernel::connectors::LoadedConnector>>,
    pub(super) exchanges: Arc<HarnessExchanges>,
    pub(super) mutation_keys: Arc<Mutex<BTreeMap<(String, String), String>>>,
    /// The fixture principal's artifact space, as runx keeps it: what the
    /// case's artifact calls stored, by reference.
    pub(super) artifacts: Arc<Mutex<BTreeMap<String, JsonObject>>>,
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
                let grants = match &fixture.grant {
                    Some(grant) => {
                        let connector = self.grant_connector.as_ref().ok_or_else(|| {
                            transport_error(
                                "the provider fixture's grant names no connector".to_owned(),
                            )
                        })?;
                        // Connect records the provider of the connector a grant
                        // binds, however the fixture named that connector.
                        vec![serde_json::json!({
                            "grant_id": grant.grant_id,
                            "provider": connector.manifest.id.as_str(),
                            "connector_binding": connector.binding(),
                            "status": "active",
                            "scopes": grant.scopes,
                        })]
                    }
                    None => Vec::new(),
                };
                json_response(
                    JsonValue::from_serialize(&serde_json::json!({
                        "status": "success",
                        "grants": grants,
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
            (HttpMethod::Post, url)
                if url == format!("{HARNESS_PROVIDER_BASE_URL}/v1/artifact-operations") =>
            {
                self.artifact_operation(fixture, request.body.as_deref())
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
        let Some(grant) = fixture
            .grant
            .as_ref()
            .filter(|grant| grant.grant_id == grant_id)
        else {
            return Err(transport_error(format!(
                "the harness provider fixture holds no grant {grant_id:?}"
            )));
        };
        let provider = self.grant_connector.as_ref().map_or_else(
            || grant.provider.clone(),
            |connector| connector.manifest.id.as_str().to_owned(),
        );
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
    #[serde(default)]
    grant_id: Option<String>,
    /// What a purchase holds on one resource, presented in place of a grant.
    #[serde(default)]
    held: Option<DeclaredHold>,
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

/// The held resource a declared request is sent under.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredHold {
    credential: String,
    target: String,
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
        // A request is sent under a grant, or under what a purchase holds on
        // one resource, never both.
        let connector = match (&call.grant_id, &call.held) {
            (Some(grant_id), None) => {
                if fixture
                    .grant
                    .as_ref()
                    .is_none_or(|grant| &grant.grant_id != grant_id)
                {
                    return refused(
                        404,
                        "unknown-grant",
                        format!("the harness provider fixture holds no grant {grant_id:?}"),
                    );
                }
                self.grant_connector.as_ref().ok_or_else(|| {
                    transport_error("the provider fixture's grant names no connector".to_owned())
                })?
            }
            (None, Some(held)) => {
                if !fixture.holds.iter().any(|hold| {
                    hold.connector == call.connector_binding.connector.as_str()
                        && hold.credential == held.credential
                        && hold.target == held.target
                }) {
                    return refused(
                        403,
                        "credential_unavailable",
                        format!(
                            "Nothing is held for {}; it was never bought, has ended, or is another account's.",
                            held.target
                        ),
                    );
                }
                let Some(connector) = self
                    .held_connectors
                    .iter()
                    .find(|connector| connector.binding() == call.connector_binding)
                else {
                    return refused(
                        409,
                        "operation_contract_mismatch",
                        format!(
                            "{} is not a revision runx Connect executes; update runx.",
                            call.connector_binding.connector
                        ),
                    );
                };
                connector
            }
            _ => {
                return refused(
                    400,
                    "invalid_input",
                    "A request names a grant or a held resource, never both.",
                );
            }
        };
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
        if let Some(held) = &call.held {
            // A hold is presented only to an operation authorized by its
            // purchase credential, on the resource it holds.
            let purchase = connector
                .manifest
                .auth
                .credentials
                .iter()
                .any(|(name, declaration)| {
                    name.as_str() == held.credential && declaration.is_purchase()
                });
            if !purchase
                || !operation
                    .credentials
                    .iter()
                    .any(|credential| credential.as_str() == held.credential)
            {
                return refused(
                    400,
                    "operation_unsupported",
                    format!(
                        "operation '{}' is not one on a resource a purchase holds",
                        call.operation
                    ),
                );
            }
            let arguments = JsonValue::Object(call.input.clone());
            let target = operation.target.template.render_string(
                &runx_contracts::connector::TemplateInputs {
                    arguments: Some(&arguments),
                    ..runx_contracts::connector::TemplateInputs::default()
                },
            );
            if target.as_deref().ok() != Some(held.target.as_str()) {
                return refused(
                    400,
                    "invalid_input",
                    "The call names another resource than the one it is sent under.",
                );
            }
        } else if operation.scopes.iter().any(|scope| {
            !fixture
                .grant
                .as_ref()
                .is_some_and(|grant| grant.scopes.iter().any(|granted| granted == scope.as_str()))
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
        let body = match operation.holds.as_ref() {
            Some(hold) if hold.action == runx_contracts::connector::HoldAction::Take => {
                held_answer(hold, spec, answer.status, &answer.body).map_err(transport_error)?
            }
            _ => answer.body,
        };
        json_response(JsonValue::Object(JsonObject::from([
            (
                "status".to_owned(),
                JsonValue::Number(runx_contracts::JsonNumber::from(u64::from(answer.status))),
            ),
            ("body".to_owned(), JsonValue::String(body)),
        ])))
    }
}

/// What runx Connect answers a take with: the provider's answer with every
/// field the hold takes replaced by the hold's evidence, so the kernel sees
/// that a hold was taken and never what it holds. An error answer keeps none
/// of the fields.
fn held_answer(
    hold: &runx_contracts::connector::OperationHold,
    spec: &runx_contracts::connector::HttpImplementation,
    status: u16,
    body: &str,
) -> Result<String, String> {
    let paths = hold
        .fields
        .iter()
        .map(|(name, binding)| {
            binding
                .response_reference()
                .and_then(|reference| reference.strip_prefix("write."))
                .map(|path| (name.as_str().to_owned(), path.to_owned()))
                .ok_or_else(|| format!("held field {name} names no path in the write's answer"))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let decoded = serde_json::from_str::<serde_json::Value>(body).ok();
    let Some(serde_json::Value::Object(mut answer)) = decoded else {
        return if (200..300).contains(&status) {
            Err("the provider's answer is not the JSON object its hold reads".to_owned())
        } else {
            Ok(body.to_owned())
        };
    };
    if !(200..300).contains(&status) {
        for (_, path) in &paths {
            replace_at(&mut answer, path, serde_json::Value::Null);
        }
        return serde_json::to_string(&answer).map_err(|error| error.to_string());
    }
    let acknowledgement: JsonValue =
        serde_json::from_value(serde_json::Value::Object(answer.clone()))
            .map_err(|error| error.to_string())?;
    let source = JsonValue::Object(JsonObject::from([
        ("acknowledgement".to_owned(), acknowledgement.clone()),
        ("readbacks".to_owned(), JsonValue::Array(Vec::new())),
    ]));
    let result = JsonValue::Object(
        spec.result_paths
            .iter()
            .filter_map(|(field, path)| {
                crate::kernel::operation::confirm::at_path(&source, path)
                    .map(|value| (field.clone(), value.clone()))
            })
            .collect(),
    );
    let inputs = runx_contracts::connector::TemplateInputs {
        result: Some(&result),
        ..runx_contracts::connector::TemplateInputs::default()
    };
    let target = hold
        .target
        .render_string(&inputs)
        .map_err(|error| format!("the provider's answer names no held resource: {error}"))?;
    let expires_at = hold
        .expires_at
        .as_ref()
        .ok_or("a take states when its resource ends")?
        .render_string(&inputs)
        .map_err(|error| format!("the provider's answer names no expiry: {error}"))?;
    let mut fields = serde_json::Map::new();
    for (name, path) in &paths {
        let value = crate::kernel::operation::confirm::at_path(&acknowledgement, path)
            .and_then(JsonValue::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("the provider's answer holds no {name}"))?;
        fields.insert(name.clone(), serde_json::Value::String(value.to_owned()));
    }
    let digest =
        runx_contracts::canonical_json::canonical_stable_json(&serde_json::Value::Object(fields))
            .map(|text| runx_contracts::sha256_prefixed(text.as_bytes()))
            .map_err(|error| error.to_string())?;
    let evidence = serde_json::json!({
        runx_contracts::connector::HOLD_EVIDENCE_KEY: {
            "target": target,
            "expires_at": expires_at,
            "value_digest": digest,
        }
    });
    for (_, path) in &paths {
        replace_at(&mut answer, path, evidence.clone());
    }
    serde_json::to_string(&answer).map_err(|error| error.to_string())
}

/// Replace the value at a dotted path that exists; nothing else changes.
fn replace_at(
    object: &mut serde_json::Map<String, serde_json::Value>,
    path: &str,
    value: serde_json::Value,
) {
    let segments = path.split('.').collect::<Vec<_>>();
    let Some((last, parents)) = segments.split_last() else {
        return;
    };
    let mut current = object;
    for segment in parents {
        let Some(serde_json::Value::Object(next)) = current.get_mut(*segment) else {
            return;
        };
        current = next;
    }
    if let Some(slot) = current.get_mut(*last) {
        *slot = value;
    }
}

/// When the harness states every artifact was stored.
const HARNESS_ARTIFACT_CREATED_AT: &str = "2026-01-01T00:00:00.000Z";

impl HarnessTransport {
    /// Store or read one artifact in the fixture principal's space, as runx
    /// does: an allocation is keyed by its idempotency key, holds exactly the
    /// bytes whose digest the call states, and answers with the same
    /// reference on every repeat; an inspection reads what is held.
    fn artifact_operation(
        &self,
        fixture: &HarnessProviderResponsesFixture,
        body: Option<&str>,
    ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        use base64::Engine as _;

        let request = serde_json::from_str::<JsonObject>(
            body.ok_or_else(|| transport_error("artifact operation body is missing".to_owned()))?,
        )
        .map_err(|error| transport_error(format!("artifact operation body is invalid: {error}")))?;
        let operation = required_string(&request, "operation")?;
        let input = request
            .get("input")
            .and_then(JsonValue::as_object)
            .ok_or_else(|| transport_error("artifact operation input is missing".to_owned()))?;
        let principal_ref = format!("runx:principal:{}", fixture.principal_id);
        let mut artifacts = self
            .artifacts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (artifact_ref, evidence, idempotency_key) = match operation {
            "artifact.allocate" => {
                let key = required_string(input, "idempotency_key")?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(required_string(input, "data_base64")?)
                    .map_err(|error| {
                        transport_error(format!("artifact bytes are invalid: {error}"))
                    })?;
                let content_digest = runx_contracts::sha256_prefixed(&bytes);
                if required_string(input, "content_digest")? != content_digest {
                    return Err(transport_error(
                        "artifact bytes do not match their stated digest".to_owned(),
                    ));
                }
                let artifact_ref = format!(
                    "runx:artifact:{}",
                    runx_contracts::sha256_prefixed(format!("{principal_ref}\0{key}").as_bytes())
                );
                let evidence = JsonObject::from([
                    (
                        "artifact_ref".to_owned(),
                        JsonValue::String(artifact_ref.clone()),
                    ),
                    (
                        "content_digest".to_owned(),
                        JsonValue::String(content_digest),
                    ),
                    (
                        "media_type".to_owned(),
                        JsonValue::String(required_string(input, "media_type")?.to_owned()),
                    ),
                    (
                        "size_bytes".to_owned(),
                        JsonValue::Number(runx_contracts::JsonNumber::from(bytes.len() as u64)),
                    ),
                    (
                        "created_at".to_owned(),
                        JsonValue::String(HARNESS_ARTIFACT_CREATED_AT.to_owned()),
                    ),
                ]);
                match artifacts.get(&artifact_ref) {
                    Some(held) if held != &evidence => {
                        return Err(transport_error(format!(
                            "artifact idempotency key {key:?} already holds other bytes"
                        )));
                    }
                    Some(_) => {}
                    None => {
                        artifacts.insert(artifact_ref.clone(), evidence.clone());
                    }
                }
                (artifact_ref, evidence, Some(key.to_owned()))
            }
            "artifact.inspect" => {
                let artifact_ref = required_string(input, "artifact_ref")?.to_owned();
                let evidence = artifacts.get(&artifact_ref).cloned().ok_or_else(|| {
                    transport_error(format!("the case stored no artifact {artifact_ref:?}"))
                })?;
                (artifact_ref, evidence, None)
            }
            other => {
                return Err(transport_error(format!(
                    "the harness holds no artifact operation {other:?}"
                )));
            }
        };
        let mut packet = JsonObject::from([
            (
                "schema".to_owned(),
                JsonValue::String("runx.provider.operation.v1".to_owned()),
            ),
            ("status".to_owned(), JsonValue::String("success".to_owned())),
            ("provider".to_owned(), JsonValue::String("runx".to_owned())),
            (
                "operation".to_owned(),
                JsonValue::String(operation.to_owned()),
            ),
            ("target".to_owned(), JsonValue::String(artifact_ref.clone())),
            ("result".to_owned(), JsonValue::Object(evidence)),
            (
                "transport".to_owned(),
                JsonValue::String("hosted_loopback".to_owned()),
            ),
            (
                "readback_ref".to_owned(),
                JsonValue::String(format!("runx:harness:artifact-readback:{artifact_ref}")),
            ),
            (
                "access".to_owned(),
                JsonValue::String(
                    if idempotency_key.is_some() {
                        "mutate"
                    } else {
                        "read"
                    }
                    .to_owned(),
                ),
            ),
            ("principal_ref".to_owned(), JsonValue::String(principal_ref)),
            (
                "finality".to_owned(),
                JsonValue::String("verified".to_owned()),
            ),
        ]);
        if let Some(key) = idempotency_key {
            packet.insert("idempotency_key".to_owned(), JsonValue::String(key));
        }
        json_response(JsonValue::Object(packet))
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use runx_contracts::connector::{HttpImplementation, OperationHold};
    use serde_json::json;

    use super::held_answer;

    fn hold() -> OperationHold {
        serde_json::from_value(json!({
            "action": "take",
            "credential": "lease",
            "target": {"$concat": ["acme:inbox/", {"$result": "/lease/inbox_id"}]},
            "expires_at": {"$result": "/lease/expires_at"},
            "fields": {"capability": {"$response": "write.capability"}},
        }))
        .expect("a take")
    }

    fn spec() -> HttpImplementation {
        serde_json::from_value(json!({
            "allowed_hosts": ["inbox.acme.test"],
            "requests": [{"id": "provision", "method": "POST", "url": "https://inbox.acme.test/inboxes"}],
            "result_paths": {"lease": "acknowledgement.lease"},
        }))
        .expect("an http implementation")
    }

    #[test]
    fn a_take_answers_the_hold_s_evidence_in_place_of_what_it_holds() {
        let answer = held_answer(
            &hold(),
            &spec(),
            200,
            r#"{"lease":{"inbox_id":"inb_1","expires_at":"2026-10-16T00:00:00.000Z"},"capability":"auc_secret"}"#,
        )
        .expect("a held answer");
        assert!(!answer.contains("auc_secret"));
        let answer: serde_json::Value = serde_json::from_str(&answer).expect("json");
        assert_eq!(
            answer["capability"]["$held"],
            json!({
                "target": "acme:inbox/inb_1",
                "expires_at": "2026-10-16T00:00:00.000Z",
                "value_digest": runx_contracts::sha256_prefixed(br#"{"capability":"auc_secret"}"#),
            })
        );
        assert_eq!(answer["lease"]["inbox_id"], json!("inb_1"));
    }

    #[test]
    fn a_take_holds_nothing_from_an_error_or_an_answer_without_its_fields() {
        let refused = held_answer(
            &hold(),
            &spec(),
            409,
            r#"{"error":"quota","capability":"auc_secret"}"#,
        )
        .expect("an error answer");
        assert_eq!(refused, r#"{"capability":null,"error":"quota"}"#);
        held_answer(
            &hold(),
            &spec(),
            200,
            r#"{"lease":{"inbox_id":"inb_1","expires_at":"2026-10-16T00:00:00.000Z"}}"#,
        )
        .expect_err("no capability to hold");
        held_answer(&hold(), &spec(), 200, "not json").expect_err("not an object");
    }
}
