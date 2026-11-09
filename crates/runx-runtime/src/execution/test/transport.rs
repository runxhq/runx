use std::collections::BTreeMap;
use std::sync::Arc;

use runx_parser::{TestHttpRequestBodyFixture, TestHttpResponseFixture, TestProviders};

use super::provider_responses::{TEST_PROVIDER_BASE_URL, TEST_PROVIDER_TOKEN, TestTransport};
use crate::effects::{RuntimeEffectError, RuntimeEffectRegistry};
use crate::http::{
    HttpMethod, RuntimeHttpHeader, RuntimeHttpResponse, RuntimeTestHttpExchange,
    RuntimeTestHttpRequestBody, TestExchanges,
};
use crate::kernel::connectors::LoadedConnector;
use crate::kernel::operation::OperationEffect;

/// The registry a test case runs with: operation steps run over the
/// test transport, which answers the provider fixture and the case's
/// exact HTTP exchanges. Every other request is refused; a test run never
/// reaches the network. No public runtime input, environment variable or
/// provider configuration can create this state; only the test engine
/// calls this function.
pub(super) fn effects_over(
    effects: &RuntimeEffectRegistry,
    transport: TestTransport,
    providers: &TestProviders,
) -> Result<RuntimeEffectRegistry, RuntimeEffectError> {
    let mut effects = effects.clone();
    effects.replace_effect(OperationEffect::test(
        transport,
        &providers.mcp,
        &providers.processes,
    ))?;
    Ok(effects)
}

/// The transport the test's providers stand in for the world with.
/// `closure` is every connector the case's run binds through its closure.
pub(super) fn test_transport(
    providers: &TestProviders,
    scope: &crate::ConnectorScope,
    closure: &[Arc<LoadedConnector>],
    skill_dir: &std::path::Path,
) -> Result<TestTransport, String> {
    let grant_connector = providers
        .hosted
        .as_ref()
        .and_then(|provider| provider.grant.as_ref())
        .map(|grant| case_connector(scope, closure, skill_dir, &grant.provider))
        .transpose()?;
    let mut connectors_with_leases: Vec<Arc<LoadedConnector>> = Vec::new();
    for lease in providers
        .hosted
        .iter()
        .flat_map(|provider| &provider.leases)
    {
        let connector = case_connector(scope, closure, skill_dir, &lease.connector)?;
        if !connectors_with_leases
            .iter()
            .any(|known| known.binding() == connector.binding())
        {
            connectors_with_leases.push(connector);
        }
    }
    let exchanges = Arc::new(TestExchanges::new(
        providers
            .http
            .iter()
            .map(|exchange| RuntimeTestHttpExchange {
                method: runtime_method(&exchange.request.method),
                url: exchange.request.url.clone(),
                body: match &exchange.request.body {
                    TestHttpRequestBodyFixture::None(_) => RuntimeTestHttpRequestBody::None,
                    TestHttpRequestBodyFixture::Json { json } => {
                        RuntimeTestHttpRequestBody::Json(json.clone())
                    }
                    TestHttpRequestBodyFixture::Text { text } => {
                        RuntimeTestHttpRequestBody::Text(text.clone())
                    }
                },
                response: runtime_response(&exchange.response),
            })
            .collect(),
    ));
    Ok(TestTransport {
        provider: providers.hosted.clone(),
        grant_connector,
        connectors_with_leases,
        exchanges,
        mutation_keys: Arc::default(),
        artifacts: Arc::default(),
    })
}

/// The connector `id` a case names, for its provider grant, its leases or
/// its setup's sessions and credentials, as its operations reach it: from
/// the case's own package, from whichever of a bundle's packages carries it,
/// or from the closure its run binds, registry children included, since a
/// case runs its whole closure. A name that reaches more than one revision
/// is refused, never settled by where it was found first.
pub(super) fn case_connector(
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
    // A connector has one name; the closure's revisions of it are reached.
    for connector in closure {
        if connector.name.as_str() == id {
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
                .map(|connector| format!("{} at {}", connector.name, connector.manifest_digest))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The environment a case's provider fixture runs under: the test's hosted
/// API, authenticated as the fixture's principal.
pub(crate) fn provider_env(providers: &TestProviders) -> BTreeMap<String, String> {
    if providers.hosted.is_none() {
        return BTreeMap::new();
    }
    BTreeMap::from([
        (
            crate::HOSTED_API_BASE_URL_ENV.to_owned(),
            TEST_PROVIDER_BASE_URL.to_owned(),
        ),
        (
            crate::HOSTED_API_TOKEN_ENV.to_owned(),
            TEST_PROVIDER_TOKEN.to_owned(),
        ),
    ])
}

/// Confirm and select the fixture's grant as the instance the case's hosted
/// operations run under, as `runx connector use <connector> <grant>` would, in the kernel
/// `env` resolves for `cwd`, reading the grant from the test's Connect.
pub(crate) fn seed_provider_grant(
    providers: &TestProviders,
    scope: &crate::ConnectorScope,
    closure: &[Arc<LoadedConnector>],
    env: &BTreeMap<String, String>,
    cwd: &std::path::Path,
) -> Result<(), String> {
    let Some(grant) = providers
        .hosted
        .as_ref()
        .and_then(|provider| provider.grant.as_ref())
    else {
        return Ok(());
    };
    let connector = case_connector(scope, closure, cwd, &grant.provider)?;
    let instances = crate::Instances::new(
        crate::KernelStore::for_project(&crate::paths::RunxPaths::resolve(env, cwd))
            .map_err(|error| error.to_string())?,
    );
    crate::kernel::setup::confirm_grant_over(
        &instances,
        &connector,
        Arc::new(test_transport(providers, scope, closure, cwd)?),
        &grant.grant_id,
        env,
        cwd,
    )
    .map(|_| ())
}

fn runtime_response(fixture: &TestHttpResponseFixture) -> RuntimeHttpResponse {
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
        _ => unreachable!("test HTTP method was validated by runx-parser"),
    }
}
