//! The hosted placement: runx Connect holds the grant. It sends a declared
//! operation's requests through the provider gateway, and runs any other
//! operation through a private driver. The kernel authenticates once per
//! process and environment, reads grant evidence from the principal's grant
//! listing, and sends every call with the digests it admitted.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use runx_contracts::{JsonNumber, JsonValue, PrincipalReference};

use super::Failure;
use crate::hosted_api::HostedApiOperationError;
use crate::http::RuntimeHttpTransport;
use crate::provider_operations::ProviderOperationError;
use crate::{
    AuthenticatedHostedApiEnvironment, HostedApiEnvironment, HostedProviderGrant,
    ProviderOperationRequest, hosted_private_network_allowed, list_provider_grants,
};

type Transport = Arc<dyn RuntimeHttpTransport + Send + Sync>;

/// A grant as evidence: the listing's view plus the principal holding it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostedGrant {
    pub grant_id: String,
    pub provider: String,
    pub scopes: Vec<String>,
    pub status: String,
    pub target_locator: Option<String>,
    pub principal_ref: String,
}

/// An authenticated hosted session.
pub(crate) struct HostedSession {
    transport: Transport,
    environment: AuthenticatedHostedApiEnvironment,
    pub principal_ref: String,
}

/// The hosted placement's client. One per operation effect; caches the
/// session and the grant listing for the environment it was resolved for.
#[derive(Default)]
pub(crate) struct HostedClient {
    transport: Option<Transport>,
    session: Mutex<Option<(HostedApiEnvironment, AuthenticatedHostedApiEnvironment)>>,
    grants: Mutex<Option<(HostedApiEnvironment, Vec<HostedProviderGrant>)>>,
}

impl HostedClient {
    /// A client over an injected transport: the harness and embedders
    /// verify the production path without live provider traffic.
    pub(crate) fn with_transport(transport: Transport) -> Self {
        Self {
            transport: Some(transport),
            ..Self::default()
        }
    }

    pub(crate) fn session(
        &self,
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<HostedSession, String> {
        let transport = self.transport(env)?;
        let resolved =
            HostedApiEnvironment::resolve(None, env, cwd).map_err(|error| error.to_string())?;
        let mut cached = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let environment = match cached.as_ref() {
            Some((environment, authenticated)) if environment == &resolved => authenticated.clone(),
            _ => {
                let authenticated = resolved
                    .authenticate(transport.as_ref())
                    .map_err(|error| error.to_string())?;
                *cached = Some((resolved, authenticated.clone()));
                authenticated
            }
        };
        let principal_ref =
            PrincipalReference::from_runx_principal_id(environment.runx_principal_id().clone())
                .as_reference()
                .uri
                .as_str()
                .to_owned();
        Ok(HostedSession {
            transport,
            environment,
            principal_ref,
        })
    }

    /// The listed grant with this id, held by the authenticated principal.
    pub(crate) fn grant(
        &self,
        env: &BTreeMap<String, String>,
        cwd: &Path,
        grant_id: &str,
    ) -> Result<HostedGrant, String> {
        let session = self.session(env, cwd)?;
        let resolved =
            HostedApiEnvironment::resolve(None, env, cwd).map_err(|error| error.to_string())?;
        let mut cached = self
            .grants
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cached
            .as_ref()
            .is_none_or(|(environment, _)| environment != &resolved)
        {
            let grants = list_provider_grants(session.transport.as_ref(), &session.environment)
                .map_err(|error| error.to_string())?;
            *cached = Some((resolved, grants));
        }
        let grant = cached
            .as_ref()
            .and_then(|(_, grants)| grants.iter().find(|grant| grant.grant_id == grant_id))
            .ok_or_else(|| "the authenticated principal holds no such grant".to_owned())?;
        Ok(HostedGrant {
            grant_id: grant.grant_id.clone(),
            provider: grant.provider.clone(),
            scopes: grant.scopes.clone(),
            status: grant.status.clone(),
            target_locator: grant.target_locator.clone(),
            principal_ref: session.principal_ref,
        })
    }

    pub(crate) fn call(
        session: &HostedSession,
        request: &ProviderOperationRequest,
    ) -> Result<super::packet::Readback, Failure> {
        crate::provider_operations::invoke_provider_operation(
            session.transport.as_ref(),
            &session.environment,
            request,
        )
        .map_err(|error| match rejection(&error) {
            Some((status, code, reason)) => Failure::Rejected {
                status,
                release: refused_before_execution(&code),
                code,
                reason,
            },
            None => Failure::Unknown(error.to_string()),
        })
    }

    fn transport(&self, env: &BTreeMap<String, String>) -> Result<Transport, String> {
        if let Some(transport) = &self.transport {
            return Ok(Arc::clone(transport));
        }
        crate::hosted_provider_api_transport(hosted_private_network_allowed(false, env))
            .map(|transport| Arc::new(transport) as Transport)
            .map_err(|error| error.to_string())
    }
}

/// A hosted operation's declared requests, sent through runx Connect. The
/// route renders each from its own copy of the declaration and presents the
/// grant's credential through the provider gateway, so the kernel drives the
/// same exchange, checks and custody on both placements.
pub(crate) struct Gateway<'a> {
    pub session: &'a HostedSession,
    pub grant_id: &'a str,
    pub operation: &'a str,
    pub operation_digest: &'a str,
    pub implementation_digest: &'a str,
    pub input: &'a runx_contracts::JsonObject,
    pub idempotency_key: Option<&'a str>,
}

impl Gateway<'_> {
    pub(crate) fn send(
        &self,
        request: &str,
        item: Option<usize>,
        responses: Option<&std::collections::BTreeMap<String, runx_contracts::JsonObject>>,
    ) -> Result<crate::http::RuntimeHttpResponse, super::confirm::Unperformed> {
        // Connect renders in TypeScript, where a JSON integer past 2^53 is
        // rounded: refuse it rather than let the two renderings differ.
        if self.input.values().any(beyond_exact_integers) {
            return Err(super::confirm::Unperformed::NotSent {
                code: "invalid_request",
                reason: "an integer beyond 2^53 cannot be rendered exactly by runx Connect"
                    .to_owned(),
            });
        }
        crate::provider_operations::send_provider_request(
            self.session.transport.as_ref(),
            &self.session.environment,
            &crate::provider_operations::ProviderRequest {
                grant_id: self.grant_id,
                operation: self.operation,
                operation_digest: self.operation_digest,
                implementation_digest: self.implementation_digest,
                input: self.input,
                idempotency_key: self.idempotency_key,
                request,
                item,
                responses,
            },
        )
        .map_err(|error| match error {
            // The route answers 4xx only before it sends anything; the
            // provider's own answers come back whole inside a success.
            ProviderOperationError::HostedApi(HostedApiOperationError::Api {
                status: 400..=499,
                code,
                detail,
                ..
            }) => super::confirm::Unperformed::NotSent {
                code: "connect_refused",
                reason: format!("{code}: {detail}"),
            },
            error => super::confirm::Unperformed::MaybeSent {
                code: "transport_failed",
                reason: error.to_string(),
            },
        })
    }
}

/// Whether a value holds an integer JavaScript cannot represent exactly.
fn beyond_exact_integers(value: &JsonValue) -> bool {
    const EXACT: u64 = (1 << 53) - 1;
    match value {
        JsonValue::Number(JsonNumber::I64(number)) => number.unsigned_abs() > EXACT,
        JsonValue::Number(JsonNumber::U64(number)) => *number > EXACT,
        JsonValue::Array(values) => values.iter().any(beyond_exact_integers),
        JsonValue::Object(fields) => fields.values().any(beyond_exact_integers),
        _ => false,
    }
}

/// A conflict is a definite refusal only when Connect says the key belongs to
/// another request, or that the call was admitted under another contract. An
/// in-flight claim or gateway failure may already have reached the provider,
/// so it stays unknown.
fn rejection(error: &ProviderOperationError) -> Option<(u16, String, String)> {
    match error {
        ProviderOperationError::HostedApi(HostedApiOperationError::Api {
            status,
            code,
            detail,
            ..
        }) if matches!(*status, 400 | 404)
            || (*status == 403
                && matches!(
                    code.as_str(),
                    "binding_unavailable" | "placement_not_public"
                ))
            || (*status == 409
                && matches!(
                    code.as_str(),
                    "idempotency_conflict" | "operation_contract_mismatch"
                )) =>
        {
            Some((*status, code.clone(), detail.clone()))
        }
        _ => None,
    }
}

/// Refusal codes that prove the provider never executed the call, so a
/// standing approval use it spent is released.
pub(crate) fn refused_before_execution(code: &str) -> bool {
    matches!(
        code,
        "connect_refused"
            | "operation_contract_mismatch"
            | "placement_not_public"
            | "scope_mismatch"
            | "precondition_failed"
            | "access_mismatch"
            | "target_mismatch"
            | "operation_unsupported"
            | "binding_unavailable"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_error(status: u16, code: &str) -> ProviderOperationError {
        ProviderOperationError::HostedApi(HostedApiOperationError::Api {
            operation: "provider operation",
            status,
            code: code.to_owned(),
            detail: "AWS rejected the request (BadRequestException): languageCode".to_owned(),
            hint: None,
            retry_after_seconds: None,
        })
    }

    #[test]
    fn definite_refusals_are_rejections_and_unknown_outcomes_are_not() {
        for status in [400, 404] {
            let (http_status, code, reason) =
                rejection(&api_error(status, "invalid_input")).unwrap_or_default();
            assert_eq!(http_status, status);
            assert_eq!(code, "invalid_input");
            assert!(reason.contains("languageCode"));
        }
        for status in [409, 429, 500, 502, 503] {
            assert!(rejection(&api_error(status, "invalid_input")).is_none());
        }
        assert!(rejection(&api_error(409, "idempotency_conflict")).is_some());
        assert!(rejection(&api_error(409, "operation_contract_mismatch")).is_some());
        assert!(rejection(&api_error(403, "placement_not_public")).is_some());
        assert!(rejection(&api_error(409, "idempotency_in_flight")).is_none());
        assert!(rejection(&api_error(403, "binding_unavailable")).is_some());
        assert!(rejection(&ProviderOperationError::InvalidOperation).is_none());
    }

    #[test]
    fn integers_javascript_rounds_are_found_wherever_they_nest() {
        let number = |value: u64| JsonValue::Number(JsonNumber::U64(value));
        assert!(!beyond_exact_integers(&number((1 << 53) - 1)));
        assert!(beyond_exact_integers(&JsonValue::Array(vec![number(
            1 << 53
        )])));
        assert!(beyond_exact_integers(&JsonValue::Number(JsonNumber::I64(
            i64::MIN
        ))));
        assert!(!beyond_exact_integers(&JsonValue::Number(JsonNumber::F64(
            1.5
        ))));
    }
}
