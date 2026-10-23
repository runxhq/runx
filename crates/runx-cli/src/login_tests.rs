use super::*;

use runx_runtime::{
    HttpMethod, RuntimeHttpError, RuntimeHttpRequest as HttpRequest,
    RuntimeHttpResponse as HttpResponse,
};
use std::cell::RefCell;
use std::fs;

#[derive(Default)]
struct StubTransport {
    requests: RefCell<Vec<HttpRequest>>,
    responses: RefCell<Vec<HttpResponse>>,
}

impl StubTransport {
    fn with_responses(responses: Vec<HttpResponse>) -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            responses: RefCell::new(responses.into_iter().rev().collect()),
        }
    }
}

impl Transport for StubTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, RuntimeHttpError> {
        self.requests.borrow_mut().push(request);
        Ok(self
            .responses
            .borrow_mut()
            .pop()
            .unwrap_or_else(|| HttpResponse::new(500, "missing stub response")))
    }
}

#[test]
fn parses_login_plan() -> Result<(), String> {
    let args = vec![
        OsString::from("login"),
        OsString::from("--api-base-url"),
        OsString::from("https://runx.test/"),
        OsString::from("--provider"),
        OsString::from("github"),
        OsString::from("--for"),
        OsString::from("publish"),
        OsString::from("--token-stdin"),
        OsString::from("--allow-local-api"),
        OsString::from("-j"),
    ];
    assert_eq!(
        parse_login_plan(&args)?,
        LoginPlan {
            api_base_url: Some("https://runx.test/".to_owned()),
            provider: Some("github".to_owned()),
            purpose: Some(HostedApiCredentialPurpose::Publish),
            token_stdin: true,
            allow_local_api: true,
            json: true,
        }
    );
    Ok(())
}

#[test]
fn rejects_unknown_login_purpose_before_network_access() {
    let result = parse_login_plan(&[
        OsString::from("login"),
        OsString::from("--for"),
        OsString::from("billing"),
    ]);

    assert_eq!(result, Err("--for must be default or publish".to_owned()));
}

#[test]
fn login_exchange_stores_encrypted_public_api_token() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile_dir()?;
    let env = BTreeMap::from([("RUNX_HOME".to_owned(), temp.to_string_lossy().to_string())]);
    runx_runtime::store_authenticated_hosted_environment(
        &env,
        &temp,
        HostedApiCredentialPurpose::Default,
        "https://runx.test",
        "user_1",
        "rxk_default",
    )?;
    let transport = StubTransport::with_responses(vec![
        HttpResponse::new(
            201,
            serde_json::json!({
                "status": "pending",
                "session_id": "login_1",
                "login_token": "ticket_1",
                "authorization_url": "https://runx.test/connect/login_1",
                "poll_after_ms": 0
            })
            .to_string(),
        ),
        HttpResponse::new(
            202,
            serde_json::json!({
                "status": "pending",
                "session_id": "login_1",
                "poll_after_ms": 0
            })
            .to_string(),
        ),
        HttpResponse::new(
            200,
            serde_json::json!({
                "status": "success",
                "session_id": "login_1",
                "principal_id": "user_1",
                "credential_id": "cred_1",
                "token": "rxk_secret"
            })
            .to_string(),
        ),
    ]);
    let output = run_login_command_with_transport(
        &LoginPlan {
            api_base_url: Some("https://runx.test/".to_owned()),
            provider: Some("github".to_owned()),
            purpose: Some(HostedApiCredentialPurpose::Publish),
            token_stdin: false,
            allow_local_api: false,
            json: true,
        },
        &env,
        &temp,
        &transport,
        |_| {},
    )?;

    assert!(output.contains("\"status\": \"success\""));
    let config_text = fs::read_to_string(temp.join("config.json"))?;
    assert!(config_text.contains("publish_api_token_material_id"));
    assert!(!config_text.contains("rxk_secret"));
    assert!(!config_text.contains("rxk_default"));
    let config = runx_runtime::load_runx_config_file(&temp.join("config.json"))?;
    let public = config.public.ok_or("missing public config")?;
    assert_eq!(
        runx_runtime::load_local_public_api_token(
            &home_paths(&temp),
            public
                .api_token_material_id
                .as_deref()
                .ok_or("missing default token")?,
        )?,
        "rxk_default"
    );
    assert_eq!(
        runx_runtime::load_local_public_api_token(
            &home_paths(&temp),
            public
                .publish_api_token_material_id
                .as_deref()
                .ok_or("missing publish token")?,
        )?,
        "rxk_secret"
    );

    let requests = transport.requests.borrow();
    assert_eq!(requests[0].url, "https://runx.test/v1/login/sessions");
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert_eq!(
        request_json_body(&requests[0])?,
        serde_json::json!({"provider":"github","purpose":"publish"})
    );
    assert_eq!(
        requests[1].url,
        "https://runx.test/v1/login/sessions/login_1/complete"
    );
    assert_eq!(
        request_json_body(&requests[1])?,
        serde_json::json!({"login_token":"ticket_1"})
    );
    assert_eq!(
        requests[2].url,
        "https://runx.test/v1/login/sessions/login_1/complete"
    );
    Ok(())
}

#[test]
fn login_surfaces_api_error() -> Result<(), String> {
    let transport = StubTransport::with_responses(vec![HttpResponse::new(
        400,
        serde_json::json!({
            "status": "error",
            "error": {"code": "login_request_invalid", "detail": "provider must be github"}
        })
        .to_string(),
    )]);
    let error = match run_login_command_with_transport(
        &LoginPlan {
            api_base_url: Some("https://runx.test/".to_owned()),
            provider: Some("bad".to_owned()),
            purpose: None,
            token_stdin: false,
            allow_local_api: false,
            json: false,
        },
        &BTreeMap::new(),
        &std::env::temp_dir(),
        &transport,
        |_| {},
    ) {
        Ok(_) => return Err("login should fail".to_owned()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("[login_request_invalid]"));
    Ok(())
}

#[test]
fn a_provider_token_is_exchanged_without_being_stored_or_printed()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile_dir()?;
    let env = BTreeMap::from([("RUNX_HOME".to_owned(), temp.to_string_lossy().to_string())]);
    let transport = StubTransport::with_responses(vec![HttpResponse::new(
        200,
        serde_json::json!({
            "status": "success",
            "principal_id": "user_from_token",
            "credential_id": "cred_from_token",
            "token": "rxk_from_token"
        })
        .to_string(),
    )]);
    let output = run_provider_token_login_with_transport(
        &LoginPlan {
            api_base_url: Some("https://runx.test/".to_owned()),
            provider: Some("github".to_owned()),
            purpose: Some(HostedApiCredentialPurpose::Publish),
            token_stdin: true,
            allow_local_api: false,
            json: true,
        },
        &env,
        &temp,
        &transport,
        "github",
        "provider_token_secret",
    )?;

    assert!(output.contains("user_from_token"));
    assert!(!output.contains("provider_token_secret"));
    let requests = transport.requests.borrow();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url, "https://runx.test/v1/login/provider-token");
    assert_eq!(
        request_json_body(&requests[0])?,
        serde_json::json!({"provider":"github","purpose":"publish"})
    );
    assert_eq!(
        requests[0]
            .headers
            .iter()
            .find(|header| header.name == "authorization")
            .map(|header| header.value.as_str()),
        Some("Bearer provider_token_secret")
    );
    let config = fs::read_to_string(temp.join("config.json"))?;
    assert!(!config.contains("provider_token_secret"));
    assert!(!config.contains("rxk_from_token"));
    Ok(())
}

#[test]
fn a_provider_token_login_rejects_a_noncanonical_principal()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile_dir()?;
    let env = BTreeMap::from([("RUNX_HOME".to_owned(), temp.to_string_lossy().to_string())]);
    let transport = StubTransport::with_responses(vec![HttpResponse::new(
        200,
        serde_json::json!({
            "status": "success",
            "principal_id": " ",
            "credential_id": "cred_from_token",
            "token": "rxk_from_token"
        })
        .to_string(),
    )]);
    let Err(error) = run_provider_token_login_with_transport(
        &LoginPlan {
            api_base_url: Some("https://runx.test/".to_owned()),
            provider: Some("github".to_owned()),
            purpose: Some(HostedApiCredentialPurpose::Publish),
            token_stdin: true,
            allow_local_api: false,
            json: true,
        },
        &env,
        &temp,
        &transport,
        "github",
        "provider_token_secret",
    ) else {
        return Err("login without a principal must fail closed".into());
    };

    assert!(matches!(
        error,
        LoginCliError::Environment(ref message) if message.contains("principal_id must match")
    ));
    assert!(!temp.join("config.json").exists());
    Ok(())
}

fn request_json_body(request: &HttpRequest) -> Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(request.body.as_deref().unwrap_or_default())
}

fn home_paths(home: &std::path::Path) -> runx_runtime::RunxPaths {
    let env = BTreeMap::from([("RUNX_HOME".to_owned(), home.to_string_lossy().to_string())]);
    runx_runtime::RunxPaths::resolve(&env, home)
}

fn tempfile_dir() -> Result<std::path::PathBuf, std::io::Error> {
    let path = std::env::temp_dir().join(format!(
        "runx-cli-login-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    fs::create_dir_all(&path)?;
    Ok(path)
}
