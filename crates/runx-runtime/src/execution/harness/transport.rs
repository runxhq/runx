use std::collections::BTreeMap;
use std::sync::Arc;

use runx_parser::{HarnessCaller, HarnessHttpRequestBodyFixture, HarnessHttpResponseFixture};

use super::provider_responses::{
    HARNESS_PROVIDER_BASE_URL, HARNESS_PROVIDER_TOKEN, HarnessTransport,
};
use crate::effects::{RuntimeEffectError, RuntimeEffectRegistry};
use crate::http::{
    HarnessExchanges, HttpMethod, RuntimeHarnessHttpExchange, RuntimeHarnessHttpRequestBody,
    RuntimeHttpHeader, RuntimeHttpResponse,
};
use crate::kernel::connectors::LoadedConnector;
use crate::kernel::operation::OperationEffect;

/// The registry a harness case runs with: operation steps run over the
/// harness transport, which answers the provider fixture and the case's
/// exact HTTP exchanges. Every other request is refused; a harness run never
/// reaches the network. No public runtime input, environment variable or
/// provider configuration can create this state; only the harness engine
/// calls this function.
pub(super) fn effects_over(
    effects: &RuntimeEffectRegistry,
    transport: HarnessTransport,
    caller: &HarnessCaller,
) -> Result<RuntimeEffectRegistry, RuntimeEffectError> {
    let mut effects = effects.clone();
    effects.replace_effect(OperationEffect::harness(transport, &caller.mcp_calls))?;
    Ok(effects)
}

/// The transport the caller's fixtures stand in for the world with.
/// `closure` is every connector the case's run binds through its closure.
pub(super) fn harness_transport(
    caller: &HarnessCaller,
    scope: &crate::ConnectorScope,
    closure: &[Arc<LoadedConnector>],
    skill_dir: &std::path::Path,
) -> Result<HarnessTransport, String> {
    let grant_connector = caller
        .provider_responses
        .as_ref()
        .map(|provider| granted_connector(scope, closure, skill_dir, &provider.grant.provider))
        .transpose()?;
    let exchanges = Arc::new(HarnessExchanges::new(
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
            .collect(),
    ));
    Ok(HarnessTransport {
        provider: caller.provider_responses.clone(),
        grant_connector,
        exchanges,
        mutation_keys: Arc::default(),
    })
}

/// The connector a case's provider grant names, as its operations reach it:
/// from the case's own package, from whichever of a bundle's packages
/// carries it, or from the closure its run binds, registry children
/// included, since a case runs its whole closure. A name that reaches more
/// than one revision is refused, never settled by where it was found first.
fn granted_connector(
    scope: &crate::ConnectorScope,
    closure: &[Arc<LoadedConnector>],
    skill_dir: &std::path::Path,
    id: &str,
) -> Result<Arc<LoadedConnector>, String> {
    let mut reached: Vec<Arc<LoadedConnector>> = Vec::new();
    let mut reach = |connector: Arc<LoadedConnector>| {
        if !reached
            .iter()
            .any(|known| known.binding() == connector.binding())
        {
            reached.push(connector);
        }
    };
    let unreached = match scope.connector(id) {
        Ok(connector) => {
            reach(connector);
            None
        }
        Err(error) => Some(error.to_string()),
    };
    for package in crate::registry::package_bundle::bundle_package_dirs(skill_dir)?
        .into_iter()
        .flatten()
    {
        let scope = crate::ConnectorScope::at(&package).map_err(|error| error.to_string())?;
        if let Ok(connector) = scope.connector(id) {
            reach(connector);
        }
    }
    // A qualified name is one connector; a bare one is its id, as a
    // plugin names its own.
    let qualified = runx_contracts::connector::ConnectorRef::new(id);
    for connector in closure {
        let named = match &qualified {
            Some(reference) => &connector.connector_ref == reference,
            None => connector.connector_ref.id() == id,
        };
        if named {
            reach(Arc::clone(connector));
        }
    }
    match reached.len() {
        0 => Err(unreached.unwrap_or_else(|| format!("connector {id} is not in the case's reach"))),
        1 => Ok(reached.remove(0)),
        _ => Err(format!(
            "connector {id} reaches more than one revision across the case's package, its bundle and its closure ({}); one grant binds one revision",
            reached
                .iter()
                .map(|connector| format!(
                    "{} at {}",
                    connector.connector_ref, connector.manifest_digest
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The environment a case's provider fixture runs under: the harness's hosted
/// API, authenticated as the fixture's principal.
pub(crate) fn provider_env(caller: &HarnessCaller) -> BTreeMap<String, String> {
    if caller.provider_responses.is_none() {
        return BTreeMap::new();
    }
    BTreeMap::from([
        (
            crate::HOSTED_API_BASE_URL_ENV.to_owned(),
            HARNESS_PROVIDER_BASE_URL.to_owned(),
        ),
        (
            crate::HOSTED_API_TOKEN_ENV.to_owned(),
            HARNESS_PROVIDER_TOKEN.to_owned(),
        ),
    ])
}

/// Confirm and select the fixture's grant as the instance the case's hosted
/// operations run under, as `runx connector use <connector> <grant>` would, in the kernel
/// `env` resolves for `cwd`, reading the grant from the harness's Connect.
pub(crate) fn seed_provider_grant(
    caller: &HarnessCaller,
    scope: &crate::ConnectorScope,
    closure: &[Arc<LoadedConnector>],
    env: &BTreeMap<String, String>,
    cwd: &std::path::Path,
) -> Result<(), String> {
    let Some(provider) = &caller.provider_responses else {
        return Ok(());
    };
    let connector = granted_connector(scope, closure, cwd, &provider.grant.provider)?;
    let instances = crate::Instances::new(
        crate::KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))
            .map_err(|error| error.to_string())?,
    );
    crate::kernel::setup::confirm_grant_over(
        &instances,
        &connector,
        Arc::new(harness_transport(caller, scope, closure, cwd)?),
        &provider.grant.grant_id,
        env,
        cwd,
    )
    .map(|_| ())
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
