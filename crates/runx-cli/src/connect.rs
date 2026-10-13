//! Provider-neutral CLI presentation for Runx Connect grant administration.
//!
//! OAuth custody and bounded provider calls stay in Cloud. The native CLI owns
//! environment/principal resolution and grant setup, inspection, and revocation.
//! An active grant becomes a connector instance through `runx connector use
//! <provider> --grant <grant-id>`; provider calls execute only as operation
//! steps of governed skills.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::Path;
use std::process::ExitCode;

use runx_runtime::{
    HostedApiOperationError, HostedConnectAction, HostedConnectStart,
    RuntimeHttpTransport as Transport, WorkspaceEnv,
};
use serde_json::Value;

mod plan;

pub use plan::{ConnectAction, ConnectPlan, ConnectStartPlan, parse_connect_plan};

pub fn run_native_connect(plan: ConnectPlan, workspace: &WorkspaceEnv) -> ExitCode {
    run_native_connect_with_stdin(plan, workspace, io::empty())
}

pub fn run_native_connect_with_stdin(
    plan: ConnectPlan,
    workspace: &WorkspaceEnv,
    stdin: impl Read,
) -> ExitCode {
    let transport = match runx_runtime::hosted_api_transport(
        runx_runtime::hosted_private_network_allowed(plan.allow_local_api, workspace.env()),
    ) {
        Ok(transport) => transport,
        Err(error) => {
            return fail(
                &plan,
                &format!("failed to initialize HTTP transport: {error}"),
            );
        }
    };
    let credentials = match read_connect_credentials(&plan, stdin) {
        Ok(credentials) => credentials,
        Err(error) => return fail(&plan, &error),
    };
    match run_connect_with_transport(
        &plan,
        workspace.env(),
        workspace.cwd(),
        &transport,
        credentials.as_ref(),
    ) {
        Ok(output) => crate::cli_io::write_stdout_code(&output, 0),
        Err(error) => fail(&plan, &error.to_string()),
    }
}

fn read_connect_credentials(plan: &ConnectPlan, stdin: impl Read) -> Result<Option<Value>, String> {
    let ConnectAction::Start(start) = &plan.action else {
        return Ok(None);
    };
    if !start.credentials_from_stdin {
        return Ok(None);
    }
    const MAX_CREDENTIAL_BYTES: u64 = 64 * 1024;
    let mut raw = String::new();
    stdin
        .take(MAX_CREDENTIAL_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(|error| format!("failed to read connect credentials from stdin: {error}"))?;
    if raw.len() as u64 > MAX_CREDENTIAL_BYTES {
        return Err("connect credentials from stdin exceed 64 KiB".to_owned());
    }
    let credentials: Value = serde_json::from_str(raw.trim())
        .map_err(|error| format!("connect credentials from stdin are not valid JSON: {error}"))?;
    if !credentials
        .as_object()
        .is_some_and(|object| !object.is_empty())
    {
        return Err("connect credentials from stdin must be a non-empty JSON object".to_owned());
    }
    Ok(Some(credentials))
}

fn run_connect_with_transport<T: Transport>(
    plan: &ConnectPlan,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    transport: &T,
    credentials: Option<&Value>,
) -> Result<String, ConnectError> {
    let environment =
        runx_runtime::HostedApiEnvironment::resolve(plan.api_base_url.as_deref(), env, cwd)
            .map_err(|error| ConnectError::Environment(error.to_string()))?;
    let authenticated = environment
        .authenticate(transport)
        .map_err(|error| ConnectError::Environment(error.to_string()))?;
    let action = hosted_connect_action(&plan.action);
    let mut response = runx_runtime::execute_hosted_connect(transport, &authenticated, action)?;
    if let Some(credentials) = credentials {
        let status = string_field(&response, "status").unwrap_or("unknown");
        if status == "credential_required" {
            let session_id = string_field(&response, "session_id").ok_or_else(|| {
                ConnectError::InvalidJson(
                    "credential-required connect response is missing session_id".to_owned(),
                )
            })?;
            response = runx_runtime::execute_hosted_connect(
                transport,
                &authenticated,
                HostedConnectAction::SubmitCredentials {
                    session_id,
                    credentials,
                },
            )?;
        } else if !matches!(status, "created" | "unchanged") {
            return Err(ConnectError::InvalidJson(format!(
                "connect credential submission cannot continue from status {status}"
            )));
        }
    }
    render_connect_result(
        plan.json,
        authenticated.base_url(),
        authenticated.principal_id(),
        &plan.action,
        response,
    )
}

fn hosted_connect_action(action: &ConnectAction) -> HostedConnectAction<'_> {
    match action {
        ConnectAction::List => HostedConnectAction::List,
        ConnectAction::Status { session_id } => HostedConnectAction::Status { session_id },
        ConnectAction::Revoke { grant_id } => HostedConnectAction::Revoke { grant_id },
        ConnectAction::Start(start) => HostedConnectAction::Start(HostedConnectStart {
            provider: &start.provider,
            scopes: &start.scopes,
            scope_family: start.scope_family.as_deref(),
            authority_kind: start.authority_kind.as_deref(),
            target_repo: start.target_repo.as_deref(),
            target_locator: start.target_locator.as_deref(),
            binding_id: start.binding_id.as_deref(),
            credential_grant_id: start.credential_grant_id.as_deref(),
        }),
    }
}

fn render_connect_result(
    json: bool,
    base_url: &str,
    principal_id: &str,
    action: &ConnectAction,
    response: Value,
) -> Result<String, ConnectError> {
    if json {
        return serde_json::to_string_pretty(&serde_json::json!({
            "status": "success",
            "environment": {
                "base_url": base_url,
                "principal_id": principal_id,
            },
            "connect": response,
        }))
        .map(|serialized| format!("{serialized}\n"))
        .map_err(|error| ConnectError::InvalidJson(error.to_string()));
    }
    let body = match action {
        ConnectAction::List => render_grants(&response)?,
        ConnectAction::Start(start) => render_start(&start.provider, &response),
        ConnectAction::Status { .. } | ConnectAction::Revoke { .. } => pretty_json(&response)?,
    };
    let mut output = format!("runx connect · {principal_id} · {base_url}\n");
    output.push_str(&body);
    Ok(output)
}

fn render_grants(response: &Value) -> Result<String, ConnectError> {
    let grants = response
        .get("grants")
        .and_then(Value::as_array)
        .ok_or_else(|| ConnectError::InvalidJson("grants array is missing".to_owned()))?;
    if grants.is_empty() {
        return Ok("no grants\n".to_owned());
    }
    let rows = grants.iter().map(|grant| {
        let provider = string_field(grant, "provider").unwrap_or("unknown");
        let grant_id = string_field(grant, "grant_id").unwrap_or("unknown");
        let status = string_field(grant, "status").unwrap_or("unknown");
        let scopes = grant
            .get("scopes")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        format!("{provider}  {status}  {grant_id}  {scopes}\n")
    });
    Ok(rows.collect())
}

fn render_start(provider: &str, response: &Value) -> String {
    let mut output = format!(
        "status: {}\n",
        string_field(response, "status").unwrap_or("unknown")
    );
    if let Some(session_id) = string_field(response, "session_id") {
        output.push_str(&format!("session: {session_id}\n"));
    }
    if let Some(url) = string_field(response, "authorization_url") {
        output.push_str(&format!("authorize: {url}\n"));
    }
    if let Some(grant) = response.get("grant")
        && let Some(grant_id) = string_field(grant, "grant_id")
    {
        // The grant names the exact connector it binds; that reference
        // resolves wherever the connector is installed.
        let connector = grant
            .get("connector_binding")
            .and_then(|binding| string_field(binding, "connector"))
            .unwrap_or(provider);
        output.push_str(&format!(
            "grant: {grant_id}\nuse it: runx connector use {connector} --grant {grant_id}\n"
        ));
    }
    output
}

fn pretty_json(value: &Value) -> Result<String, ConnectError> {
    serde_json::to_string_pretty(value)
        .map(|serialized| format!("{serialized}\n"))
        .map_err(|error| ConnectError::InvalidJson(error.to_string()))
}

fn string_field<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

fn fail(plan: &ConnectPlan, message: &str) -> ExitCode {
    if plan.json {
        return crate::cli_io::write_stdout_code(
            &crate::router::json_failure_output(message, "connect_failed"),
            1,
        );
    }
    let _ignored = crate::cli_io::write_stderr(&format!("runx connect: {message}\n"));
    ExitCode::from(1)
}

#[derive(Debug)]
enum ConnectError {
    Environment(String),
    Operation(HostedApiOperationError),
    InvalidJson(String),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(message) => formatter.write_str(message),
            Self::Operation(error) => write!(formatter, "{error}"),
            Self::InvalidJson(message) => {
                write!(formatter, "Connect API returned invalid JSON: {message}")
            }
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<HostedApiOperationError> for ConnectError {
    fn from(error: HostedApiOperationError) -> Self {
        Self::Operation(error)
    }
}

#[cfg(test)]
mod tests {

    /// The operator's token, as the environment supplies it.
    fn token_env() -> BTreeMap<String, String> {
        BTreeMap::from([("RUNX_PUBLIC_API_TOKEN".to_owned(), "rxk_test".to_owned())])
    }
    use std::cell::RefCell;

    use runx_runtime::{
        HttpMethod, RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
    };

    use super::*;

    #[test]
    fn rejects_unsafe_path_identifiers_and_raw_provider_invocation() {
        let unsafe_grant = parse_connect_plan(&[
            "connect".into(),
            "revoke".into(),
            "../another-principal".into(),
        ]);
        assert!(unsafe_grant.is_err());

        let raw_invocation = parse_connect_plan(&[
            "connect".into(),
            "invoke".into(),
            "--grant".into(),
            "grant_slack_1".into(),
        ]);
        assert!(raw_invocation.is_err());
    }

    #[test]
    fn connect_list_uses_authenticated_runtime_service() -> Result<(), Box<dyn std::error::Error>> {
        let transport = StubTransport::new(vec![
            RuntimeHttpResponse::new(
                200,
                serde_json::json!({
                    "status": "success",
                    "principal": {"principal_id": "user_1"}
                })
                .to_string(),
            ),
            RuntimeHttpResponse::new(
                200,
                serde_json::json!({"status": "success", "grants": []}).to_string(),
            ),
        ]);
        let plan = ConnectPlan {
            action: ConnectAction::List,
            api_base_url: Some("https://runx.test/".to_owned()),
            allow_local_api: false,
            json: true,
        };

        let output = run_connect_with_transport(
            &plan,
            &token_env(),
            &std::env::temp_dir(),
            &transport,
            None,
        )?;

        assert!(output.contains("\"connect\""));
        let requests = transport.requests.borrow();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].url, "https://runx.test/v1/me");
        assert_eq!(requests[1].url, "https://runx.test/v1/grants");
        assert_eq!(requests[1].method, HttpMethod::Get);
        Ok(())
    }

    #[test]
    fn connect_start_passes_opaque_skill_scopes_unchanged() -> Result<(), Box<dyn std::error::Error>>
    {
        let scopes = [
            "vendor.operation:v3".to_owned(),
            "https://provider.example/auth/custom.scope".to_owned(),
            "urn:runx:test:opaque".to_owned(),
            "opaque capability with spaces,commas".to_owned(),
            "vendor.operation:v3".to_owned(),
        ];
        let plan = parse_connect_plan(&[
            "connect".into(),
            "start".into(),
            "future-provider".into(),
            "--scope".into(),
            scopes[0].clone().into(),
            "--scope".into(),
            scopes[1].clone().into(),
            "--scope".into(),
            scopes[2].clone().into(),
            "--scope".into(),
            scopes[3].clone().into(),
            "--scope".into(),
            scopes[4].clone().into(),
            "--api-base-url".into(),
            "https://runx.test/".into(),
            "--credential-grant".into(),
            "grant_x402_existing".into(),
            "--json".into(),
        ])?;
        let transport = StubTransport::new(vec![
            RuntimeHttpResponse::new(
                200,
                serde_json::json!({
                    "status": "success",
                    "principal": {"principal_id": "user_1"}
                })
                .to_string(),
            ),
            // The CLI treats the hosted start status as an opaque string; the
            // stub deliberately avoids private broker vocabulary.
            RuntimeHttpResponse::new(
                201,
                serde_json::json!({
                    "status": "pending",
                    "session_id": "flow_1"
                })
                .to_string(),
            ),
        ]);

        run_connect_with_transport(&plan, &token_env(), &std::env::temp_dir(), &transport, None)?;

        let requests = transport.requests.borrow();
        let body = requests[1]
            .body
            .as_deref()
            .ok_or("connect start request body missing")?;
        let request: serde_json::Value = serde_json::from_str(body)?;
        assert_eq!(request["provider"], "future-provider");
        assert_eq!(request["scopes"], serde_json::json!(scopes));
        assert_eq!(request["credential_grant_id"], "grant_x402_existing");
        Ok(())
    }

    #[test]
    fn connect_start_submits_stdin_credentials_without_rendering_them()
    -> Result<(), Box<dyn std::error::Error>> {
        let plan = parse_connect_plan(&[
            "connect".into(),
            "start".into(),
            "x402".into(),
            "--scope".into(),
            "payment.x402".into(),
            "--credentials-from-stdin".into(),
            "--api-base-url".into(),
            "https://runx.test/".into(),
            "--json".into(),
        ])?;
        let transport = StubTransport::new(vec![
            RuntimeHttpResponse::new(
                200,
                serde_json::json!({
                    "status": "success",
                    "principal": {"principal_id": "user_1"}
                })
                .to_string(),
            ),
            RuntimeHttpResponse::new(
                201,
                serde_json::json!({
                    "status": "credential_required",
                    "session_id": "flow_1"
                })
                .to_string(),
            ),
            RuntimeHttpResponse::new(
                201,
                serde_json::json!({
                    "status": "created",
                    "session_id": "flow_1",
                    "grant": {"grant_id": "grant_x402_1"}
                })
                .to_string(),
            ),
        ]);
        let credentials = serde_json::json!({
            "address": "0x0000000000000000000000000000000000000001",
            "private_key": "secret-private-key"
        });

        let output = run_connect_with_transport(
            &plan,
            &token_env(),
            &std::env::temp_dir(),
            &transport,
            Some(&credentials),
        )?;

        assert!(output.contains("grant_x402_1"));
        assert!(!output.contains("secret-private-key"));
        let requests = transport.requests.borrow();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[2].url,
            "https://runx.test/v1/connect/sessions/flow_1/credentials"
        );
        let request: serde_json::Value = serde_json::from_str(
            requests[2]
                .body
                .as_deref()
                .ok_or("connect credential request body missing")?,
        )?;
        assert_eq!(request["credentials"], credentials);
        Ok(())
    }

    struct StubTransport {
        requests: RefCell<Vec<RuntimeHttpRequest>>,
        responses: RefCell<Vec<RuntimeHttpResponse>>,
    }

    impl StubTransport {
        fn new(responses: Vec<RuntimeHttpResponse>) -> Self {
            Self {
                requests: RefCell::new(Vec::new()),
                responses: RefCell::new(responses.into_iter().rev().collect()),
            }
        }
    }

    impl RuntimeHttpTransport for StubTransport {
        fn send(
            &self,
            request: RuntimeHttpRequest,
        ) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
            self.requests.borrow_mut().push(request);
            self.responses
                .borrow_mut()
                .pop()
                .ok_or_else(|| RuntimeHttpError::Transport {
                    message: "missing stub response".to_owned(),
                })
        }
    }
}
