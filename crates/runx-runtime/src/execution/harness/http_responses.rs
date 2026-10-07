use std::collections::BTreeMap;
use std::sync::Arc;

use runx_parser::{
    HarnessHttpExchangeFixture, HarnessHttpRequestBodyFixture, HarnessHttpResponseFixture,
    HarnessMcpCallFixture, HarnessProviderResponsesFixture,
};

use super::provider_responses::HarnessTransport;
use crate::effects::{RuntimeEffectError, RuntimeEffectRegistry};
use crate::http::{
    HttpMethod, RuntimeHarnessHttpExchange, RuntimeHarnessHttpRequestBody, RuntimeHttpHeader,
    RuntimeHttpResponse,
};
use crate::kernel::operation::OperationEffect;

/// The registry a harness case runs with: the case's exact HTTP fixtures are
/// attached for the native tools that read them, and operation steps run over
/// the harness transport, which answers the provider fixture and those
/// exchanges and refuses everything else. No public runtime input,
/// environment variable or provider configuration can create this state;
/// only the harness fronts call this function.
#[cfg(feature = "cli-tool")]
pub(crate) fn effects_for_harness(
    effects: &RuntimeEffectRegistry,
    fixtures: &BTreeMap<String, HarnessHttpResponseFixture>,
    exchanges: &[HarnessHttpExchangeFixture],
    provider: Option<&HarnessProviderResponsesFixture>,
    mcp_calls: &[HarnessMcpCallFixture],
) -> Result<RuntimeEffectRegistry, RuntimeEffectError> {
    effects_over(
        effects,
        harness_transport(fixtures, exchanges, provider),
        mcp_calls,
    )
}

/// The registry with `transport` beneath operation steps and its exact
/// fixtures attached for the native tools that read them.
pub(super) fn effects_over(
    effects: &RuntimeEffectRegistry,
    transport: HarnessTransport,
    mcp_calls: &[HarnessMcpCallFixture],
) -> Result<RuntimeEffectRegistry, RuntimeEffectError> {
    let mut effects = effects
        .clone()
        .with_harness_http(transport.responses.clone());
    effects.replace_effect(OperationEffect::harness(transport, mcp_calls))?;
    Ok(effects)
}

/// The transport a case's fixtures stand in for the world with.
pub(super) fn harness_transport(
    fixtures: &BTreeMap<String, HarnessHttpResponseFixture>,
    exchanges: &[HarnessHttpExchangeFixture],
    provider: Option<&HarnessProviderResponsesFixture>,
) -> HarnessTransport {
    let responses = (!fixtures.is_empty()).then(|| {
        Arc::new(
            fixtures
                .iter()
                .map(|(url, fixture)| (url.clone(), runtime_response(fixture)))
                .collect::<BTreeMap<_, _>>(),
        )
    });
    let exchanges = (!exchanges.is_empty()).then(|| {
        Arc::new(
            exchanges
                .iter()
                .map(|exchange| RuntimeHarnessHttpExchange {
                    method: runtime_method(&exchange.request.method),
                    url: exchange.request.url.clone(),
                    body: match &exchange.request.body {
                        HarnessHttpRequestBodyFixture::None(_) => {
                            RuntimeHarnessHttpRequestBody::None
                        }
                        HarnessHttpRequestBodyFixture::Json { json } => {
                            RuntimeHarnessHttpRequestBody::Json(json.clone())
                        }
                    },
                    response: runtime_response(&exchange.response),
                })
                .collect::<Vec<_>>(),
        )
    });
    HarnessTransport {
        provider: provider.cloned(),
        responses,
        exchanges,
        mutation_keys: Arc::default(),
    }
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
