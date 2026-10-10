use runx_contracts::connector::OperationAccess;
use runx_contracts::{JsonObject, JsonValue};

use crate::hosted_api::{
    AuthenticatedHostedApiEnvironment, HostedConnectAction, execute_hosted_connect,
    request::send_json,
};
use crate::http::{HttpMethod, RuntimeHttpTransport as Transport};

mod error;

pub use error::ProviderOperationError;

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderOperationRequest {
    pub grant_id: String,
    pub operation: String,
    /// What the caller admitted the operation as; the route refuses a digest
    /// it does not know.
    pub operation_digest: String,
    pub target: String,
    pub scopes: Vec<String>,
    pub input: JsonObject,
    /// What the operation declares; the answer must state the same.
    pub access: OperationAccess,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostedProviderGrant {
    pub grant_id: String,
    pub provider: String,
    pub scopes: Vec<String>,
    pub status: String,
    pub target_locator: Option<String>,
}

pub fn list_provider_grants<T: Transport + ?Sized>(
    transport: &T,
    environment: &AuthenticatedHostedApiEnvironment,
) -> Result<Vec<HostedProviderGrant>, ProviderOperationError> {
    let response = execute_hosted_connect(transport, environment, HostedConnectAction::List)?;
    let response: JsonValue = serde_json::from_value(response).map_err(|error| {
        ProviderOperationError::InvalidResponse(format!(
            "grant response could not be projected: {error}"
        ))
    })?;
    let response = response.as_object().ok_or_else(|| {
        ProviderOperationError::InvalidResponse("grant response must be an object".to_owned())
    })?;
    if response.get("status").and_then(JsonValue::as_str) != Some("success") {
        return Err(ProviderOperationError::InvalidResponse(
            "grant response status is not success".to_owned(),
        ));
    }
    let grants = response
        .get("grants")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            ProviderOperationError::InvalidResponse(
                "grant response grants must be an array".to_owned(),
            )
        })?;
    grants
        .iter()
        .map(parse_provider_grant)
        .collect::<Result<Vec<_>, _>>()
}

fn parse_provider_grant(value: &JsonValue) -> Result<HostedProviderGrant, ProviderOperationError> {
    let grant = value.as_object().ok_or_else(|| {
        ProviderOperationError::InvalidResponse("provider grant must be an object".to_owned())
    })?;
    let grant_id = required_response_string(grant, "grant_id")?.to_owned();
    validate_provider_grant_id(&grant_id)?;
    let provider = required_response_string(grant, "provider")?.to_owned();
    let status = required_response_string(grant, "status")?.to_owned();
    let target_locator = grant
        .get("target_locator")
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let scopes = grant
        .get("scopes")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            ProviderOperationError::InvalidResponse(
                "provider grant scopes must be an array".to_owned(),
            )
        })?
        .iter()
        .map(|scope| {
            scope
                .as_str()
                .filter(|scope| !scope.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    ProviderOperationError::InvalidResponse(
                        "provider grant scopes must be non-empty strings".to_owned(),
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HostedProviderGrant {
        grant_id,
        provider,
        scopes,
        status,
        target_locator,
    })
}

pub(crate) fn invoke_provider_operation<T: Transport + ?Sized>(
    transport: &T,
    environment: &AuthenticatedHostedApiEnvironment,
    request: &ProviderOperationRequest,
) -> Result<crate::kernel::operation::packet::Readback, ProviderOperationError> {
    validate_provider_grant_id(&request.grant_id)?;
    validate_provider_operation(&request.operation)?;
    if request.scopes.is_empty() || request.scopes.iter().any(|scope| scope.trim().is_empty()) {
        return Err(ProviderOperationError::InvalidScopes);
    }
    let body = JsonObject::from([
        (
            "grant_id".to_owned(),
            JsonValue::String(request.grant_id.clone()),
        ),
        (
            "operation".to_owned(),
            JsonValue::String(request.operation.clone()),
        ),
        (
            "operation_digest".to_owned(),
            JsonValue::String(request.operation_digest.clone()),
        ),
        (
            "target".to_owned(),
            JsonValue::String(request.target.clone()),
        ),
        (
            "scopes".to_owned(),
            JsonValue::Array(
                request
                    .scopes
                    .iter()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        ),
        ("input".to_owned(), JsonValue::Object(request.input.clone())),
        (
            "access".to_owned(),
            JsonValue::String(request.access.as_str().to_owned()),
        ),
    ]);
    let body = serde_json::to_string(&body).map_err(|error| {
        crate::hosted_api::HostedApiOperationError::InvalidRequest {
            operation: "provider operation request",
            message: error.to_string(),
        }
    })?;
    // Sent once: the provider's cost of an operation is not declared, so
    // Connect is never asked to run it twice.
    let response: JsonObject = send_json(
        transport,
        environment.base_url(),
        "provider operation",
        HttpMethod::Post,
        "/v1/provider-operations",
        Some(environment.token()),
        Some(body),
    )?;
    decode_readback(response, request.access)
}

/// One declared request of a hosted operation. runx Connect renders it from
/// its own copy of the declaration, refuses one whose digests differ from
/// what the caller admitted, and sends it through the provider gateway.
pub(crate) struct ProviderRequest<'a> {
    pub grant_id: &'a str,
    pub operation: &'a str,
    pub operation_digest: &'a str,
    pub implementation_digest: &'a str,
    pub input: &'a JsonObject,
    pub idempotency_key: Option<&'a str>,
    /// The declared request's id.
    pub request: &'a str,
    /// The element of a repeated request, by position.
    pub item: Option<usize>,
    /// The write's decoded answer, for a checking read whose request binds
    /// it. Connect accepts earlier answers only for checking reads, which are
    /// GETs inside the operation's declared requests.
    pub responses: Option<&'a std::collections::BTreeMap<String, JsonObject>>,
}

/// The provider's answer to one declared request, whole.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderAnswer {
    status: u16,
    body: String,
}

pub(crate) fn send_provider_request<T: Transport + ?Sized>(
    transport: &T,
    environment: &AuthenticatedHostedApiEnvironment,
    request: &ProviderRequest<'_>,
) -> Result<crate::http::RuntimeHttpResponse, ProviderOperationError> {
    validate_provider_grant_id(request.grant_id)?;
    validate_provider_operation(request.operation)?;
    let text = |value: &str| JsonValue::String(value.to_owned());
    let mut body = JsonObject::from([
        ("grant_id".to_owned(), text(request.grant_id)),
        ("operation".to_owned(), text(request.operation)),
        (
            "operation_digest".to_owned(),
            text(request.operation_digest),
        ),
        (
            "implementation_digest".to_owned(),
            text(request.implementation_digest),
        ),
        ("input".to_owned(), JsonValue::Object(request.input.clone())),
        ("request".to_owned(), text(request.request)),
    ]);
    if let Some(key) = request.idempotency_key {
        body.insert("idempotency_key".to_owned(), text(key));
    }
    if let Some(item) = request.item {
        body.insert(
            "item".to_owned(),
            JsonValue::Number(runx_contracts::JsonNumber::from(item as u64)),
        );
    }
    if let Some(responses) = request.responses {
        body.insert(
            "responses".to_owned(),
            JsonValue::Object(
                responses
                    .iter()
                    .map(|(id, answer)| (id.clone(), JsonValue::Object(answer.clone())))
                    .collect(),
            ),
        );
    }
    let body = serde_json::to_string(&body).map_err(|error| {
        crate::hosted_api::HostedApiOperationError::InvalidRequest {
            operation: "provider request",
            message: error.to_string(),
        }
    })?;
    let (base_url, token) = (environment.base_url(), Some(environment.token()));
    let path = "/v1/provider-requests";
    // Sent once, read or write: the provider's cost of a request is not
    // declared, so Connect is never asked to send it twice.
    let answer: ProviderAnswer = send_json(
        transport,
        base_url,
        "provider request",
        HttpMethod::Post,
        path,
        token,
        Some(body),
    )?;
    Ok(crate::http::RuntimeHttpResponse::new(
        answer.status,
        answer.body,
    ))
}

/// The hosted response as a readback. The route answers for one exact call;
/// the kernel checks that it is this call's answer.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct HostedReadback {
    status: String,
    access: String,
    provider: String,
    operation: String,
    target: String,
    result: JsonValue,
    readback_ref: String,
    #[serde(default)]
    operation_id: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

fn decode_readback(
    response: JsonObject,
    access: OperationAccess,
) -> Result<crate::kernel::operation::packet::Readback, ProviderOperationError> {
    let response = JsonValue::Object(response)
        .deserialize_into::<HostedReadback>()
        .map_err(|error| ProviderOperationError::InvalidResponse(error.to_string()))?;
    if response.status != "success" {
        return Err(ProviderOperationError::InvalidResponse(
            "response status is not success".to_owned(),
        ));
    }
    if response.access != access.as_str() {
        return Err(ProviderOperationError::InvalidResponse(format!(
            "response access {:?} does not match requested access {:?}",
            response.access,
            access.as_str()
        )));
    }
    Ok(crate::kernel::operation::packet::Readback {
        provider: response.provider,
        operation: response.operation,
        target: response.target,
        result: response.result,
        readback_ref: response.readback_ref,
        operation_id: response.operation_id,
        idempotency_key: response.idempotency_key,
    })
}

fn required_response_string<'a>(
    object: &'a JsonObject,
    field: &str,
) -> Result<&'a str, ProviderOperationError> {
    object
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            ProviderOperationError::InvalidResponse(format!(
                "response {field} must be a non-empty string"
            ))
        })
}

pub fn validate_provider_grant_id(value: &str) -> Result<(), ProviderOperationError> {
    if !crate::path_util::is_safe_url_path_identifier(value) {
        return Err(ProviderOperationError::InvalidGrantId);
    }
    Ok(())
}

pub fn validate_provider_operation(value: &str) -> Result<(), ProviderOperationError> {
    if runx_contracts::connector::is_operation_name(value) {
        Ok(())
    } else {
        Err(ProviderOperationError::InvalidOperation)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
