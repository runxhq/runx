use std::collections::BTreeMap;
use std::sync::Arc;

use runx_parser::{HarnessCaller, HarnessHttpRequestBodyFixture, HarnessHttpResponseFixture};

use super::provider_responses::{
    HARNESS_PROVIDER_BASE_URL, HARNESS_PROVIDER_TOKEN, HarnessTransport, injected_grant_env,
};
use crate::effects::{RuntimeEffectError, RuntimeEffectRegistry};
use crate::http::{
    HttpMethod, RuntimeHarnessHttpExchange, RuntimeHarnessHttpRequestBody, RuntimeHttpHeader,
    RuntimeHttpResponse,
};
use crate::kernel::operation::OperationEffect;

/// The registry a harness case runs with: the case's exact HTTP exchanges are
/// attached for the native tools, and operation steps run over the harness
/// transport, which answers the provider fixture and those exchanges. Every
/// other request is refused; a harness run never reaches the network. No public runtime input,
/// environment variable or provider configuration can create this state;
/// only the harness fronts call this function.
#[cfg(feature = "process-tool")]
pub(crate) fn effects_for_harness(
    effects: &RuntimeEffectRegistry,
    caller: &HarnessCaller,
) -> Result<RuntimeEffectRegistry, RuntimeEffectError> {
    effects_over(effects, harness_transport(caller), caller)
}

/// The registry with `transport` beneath operation steps and the caller's
/// exact exchanges attached for the native tools that read them.
pub(super) fn effects_over(
    effects: &RuntimeEffectRegistry,
    transport: HarnessTransport,
    caller: &HarnessCaller,
) -> Result<RuntimeEffectRegistry, RuntimeEffectError> {
    let mut effects = effects
        .clone()
        .with_harness_http(Some(Arc::clone(&transport.exchanges)));
    effects.replace_effect(OperationEffect::harness(transport, &caller.mcp_calls))?;
    Ok(effects)
}

/// The transport the caller's fixtures stand in for the world with.
pub(super) fn harness_transport(caller: &HarnessCaller) -> HarnessTransport {
    let exchanges = Arc::new(
        caller
            .http_exchanges
            .iter()
            .map(|exchange| RuntimeHarnessHttpExchange {
                method: runtime_method(&exchange.request.method),
                url: exchange.request.url.clone(),
                body: match &exchange.request.body {
                    HarnessHttpRequestBodyFixture::None(_) => RuntimeHarnessHttpRequestBody::None,
                    HarnessHttpRequestBodyFixture::Json { json } => {
                        RuntimeHarnessHttpRequestBody::Json(json.clone())
                    }
                },
                response: runtime_response(&exchange.response),
            })
            .collect::<Vec<_>>(),
    );
    HarnessTransport {
        provider: caller.provider_responses.clone(),
        exchanges,
        mutation_keys: Arc::default(),
    }
}

/// The environment a case's provider fixture runs under: the harness's hosted
/// API and the fixture's grant, injected the way a hosted worker injects one.
pub(crate) fn provider_env(caller: &HarnessCaller) -> Result<BTreeMap<String, String>, String> {
    let Some(provider) = &caller.provider_responses else {
        return Ok(BTreeMap::new());
    };
    let mut env = injected_grant_env(provider)?;
    env.insert(
        crate::HOSTED_API_BASE_URL_ENV.to_owned(),
        HARNESS_PROVIDER_BASE_URL.to_owned(),
    );
    env.insert(
        crate::HOSTED_API_TOKEN_ENV.to_owned(),
        HARNESS_PROVIDER_TOKEN.to_owned(),
    );
    Ok(env)
}

fn runtime_response(fixture: &HarnessHttpResponseFixture) -> RuntimeHttpResponse {
    let mut response = RuntimeHttpResponse::new(fixture.status, fixture.body.clone());
    response.headers = fixture
        .headers
        .iter()
        .map(|(name, value)| RuntimeHttpHeader::new(name, value))
        .collect();
    response
}

fn runtime_method(method: &str) -> HttpMethod {
    match method {
        "GET" => HttpMethod::Get,
        "POST" => HttpMethod::Post,
        "PUT" => HttpMethod::Put,
        "PATCH" => HttpMethod::Patch,
        "DELETE" => HttpMethod::Delete,
        _ => unreachable!("harness HTTP method was validated by runx-parser"),
    }
}
