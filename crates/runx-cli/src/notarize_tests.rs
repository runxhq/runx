use super::*;

use runx_runtime::{
    HttpMethod, RuntimeHttpError, RuntimeHttpRequest as HttpRequest,
    RuntimeHttpResponse as HttpResponse, RuntimeHttpTransport as Transport,
};
use std::cell::RefCell;

#[derive(Default)]
struct StubTransport {
    requests: RefCell<Vec<HttpRequest>>,
    response: RefCell<Option<HttpResponse>>,
}

impl Transport for StubTransport {
    fn send(&self, request: HttpRequest) -> Result<HttpResponse, RuntimeHttpError> {
        self.requests.borrow_mut().push(request);
        Ok(self.response.borrow_mut().take().unwrap_or_else(|| {
            HttpResponse::new(
                201,
                serde_json::json!({
                    "status": "notarized",
                    "digest": "sha256:abc",
                    "public_hash": "abc",
                    "mode": "full",
                    "published": true,
                    "public_url": "https://runx.test/r/abc",
                    "verdict": {"valid": true}
                })
                .to_string(),
            )
        }))
    }
}

#[test]
fn parses_notarize_plan() -> Result<(), String> {
    let args = vec![
        OsString::from("notarize"),
        OsString::from("receipt.json"),
        OsString::from("--api-base-url"),
        OsString::from("https://runx.test/"),
        OsString::from("--allow-local-api"),
        OsString::from("-j"),
    ];
    let plan = parse_notarize_plan(&args)?;
    assert_eq!(
        plan,
        NotarizePlan {
            receipt_path: PathBuf::from("receipt.json"),
            api_base_url: Some("https://runx.test/".to_owned()),
            allow_local_api: true,
            json: true,
        }
    );
    Ok(())
}

#[test]
fn resolves_global_api_environment_precedence() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile_dir()?;
    let mut env = BTreeMap::new();
    env.insert("RUNX_HOME".to_owned(), temp.to_string_lossy().to_string());
    env.insert(
        "RUNX_PUBLIC_API_BASE_URL".to_owned(),
        "https://env.runx.test/".to_owned(),
    );
    env.insert(
        "RUNX_PUBLIC_API_TOKEN".to_owned(),
        "public-token".to_owned(),
    );
    let plan = NotarizePlan {
        receipt_path: PathBuf::from("receipt.json"),
        api_base_url: Some("https://plan.runx.test/".to_owned()),
        allow_local_api: false,
        json: false,
    };

    let explicit = runx_runtime::HostedApiEnvironment::resolve_publish(
        plan.api_base_url.as_deref(),
        &env,
        &temp,
    )?;
    assert_eq!(explicit.base_url(), "https://plan.runx.test");
    assert_eq!(explicit.require_token()?, "public-token");

    let from_env = runx_runtime::HostedApiEnvironment::resolve_publish(None, &env, &temp)?;
    assert_eq!(from_env.base_url(), "https://env.runx.test");
    assert_eq!(from_env.require_token()?, "public-token");

    env.insert("RUNX_PUBLIC_API_TOKEN".to_owned(), " ".to_owned());
    let blank = runx_runtime::HostedApiEnvironment::resolve_publish(None, &env, &temp)?;
    assert!(blank.require_token().is_err());

    let empty_url_plan = NotarizePlan {
        receipt_path: PathBuf::from("receipt.json"),
        api_base_url: Some("   ".to_owned()),
        allow_local_api: false,
        json: false,
    };
    let isolated_env = BTreeMap::from([(
        "RUNX_HOME".to_owned(),
        temp.join("isolated").to_string_lossy().into_owned(),
    )]);
    assert_eq!(
        runx_runtime::HostedApiEnvironment::resolve_publish(
            empty_url_plan.api_base_url.as_deref(),
            &isolated_env,
            &temp,
        )?
        .base_url(),
        "https://api.runx.ai"
    );
    Ok(())
}

#[test]
fn resolves_the_one_publish_token_the_environment_and_login_hold()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile_dir()?;
    let env = BTreeMap::from([("RUNX_HOME".to_owned(), temp.to_string_lossy().to_string())]);
    runx_runtime::store_authenticated_hosted_environment(
        &env,
        &temp,
        runx_runtime::HostedApiCredentialPurpose::Publish,
        "https://api.runx.ai",
        "user_1",
        "stored-token",
    )?;
    let environment = runx_runtime::HostedApiEnvironment::resolve_publish(None, &env, &temp)?;
    assert_eq!(environment.require_token()?, "stored-token");
    assert!(
        runx_runtime::HostedApiEnvironment::resolve(None, &env, &temp)?
            .require_token()
            .is_err()
    );

    let mismatched = runx_runtime::HostedApiEnvironment::resolve_publish(
        Some("https://other.runx.test"),
        &env,
        &temp,
    )?;
    assert!(mismatched.require_token().is_err());

    // The environment's token and the stored one are one token or none.
    let mut same = env.clone();
    same.insert(
        "RUNX_PUBLIC_API_TOKEN".to_owned(),
        "stored-token".to_owned(),
    );
    assert_eq!(
        runx_runtime::HostedApiEnvironment::resolve_publish(None, &same, &temp)?.require_token()?,
        "stored-token"
    );
    let mut differing = env.clone();
    differing.insert("RUNX_PUBLIC_API_TOKEN".to_owned(), "other-token".to_owned());
    assert_eq!(
        runx_runtime::HostedApiEnvironment::resolve_publish(None, &differing, &temp)?
            .require_token()
            .err()
            .ok_or("two differing tokens were accepted")?
            .to_string(),
        "RUNX_PUBLIC_API_TOKEN and the token `runx login` stored for https://api.runx.ai differ; unset RUNX_PUBLIC_API_TOKEN or sign in again"
    );
    Ok(())
}

#[test]
fn unauthenticated_environment_does_not_load_stored_credentials()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile_dir()?;
    let env = BTreeMap::from([("RUNX_HOME".to_owned(), temp.to_string_lossy().to_string())]);
    runx_runtime::write_runx_config_file(
        &temp.join("config.json"),
        &runx_runtime::RunxConfigFile {
            agent: None,
            public: Some(runx_runtime::RunxPublicConfig {
                api_base_url: Some("https://stored.runx.test".to_owned()),
                api_token_ref: Some("missing-encrypted-token".to_owned()),
                principal_id: Some("stored-principal".to_owned()),
                publish_api_base_url: None,
                publish_api_token_ref: None,
                publish_principal_id: None,
            }),
            credentials: None,
        },
    )?;

    let environment = runx_runtime::HostedApiEnvironment::resolve_unauthenticated(
        Some("https://login.runx.test/"),
        &env,
        &temp,
    )?;

    assert_eq!(environment.base_url(), "https://login.runx.test");
    assert!(environment.require_token().is_err());
    Ok(())
}

#[test]
fn parses_and_resolves_local_api_override() -> Result<(), String> {
    let args = vec![
        OsString::from("notarize"),
        OsString::from("receipt.json"),
        OsString::from("--allow-local-api"),
    ];
    let plan = parse_notarize_plan(&args)?;
    assert!(plan.allow_local_api);
    assert!(allow_local_api(&plan, &BTreeMap::new()));

    let plan = NotarizePlan {
        receipt_path: PathBuf::from("receipt.json"),
        api_base_url: None,
        allow_local_api: false,
        json: false,
    };
    let mut env = BTreeMap::new();
    env.insert(
        "RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK".to_owned(),
        "true".to_owned(),
    );
    assert!(allow_local_api(&plan, &env));
    Ok(())
}

#[test]
fn posts_full_receipt_notarize_request() -> Result<(), String> {
    let transport = StubTransport::default();
    let receipt: JsonValue =
        serde_json::from_value(serde_json::json!({"id": "receipt_1"})).map_err(stringify)?;
    let response = runx_runtime::notarize_hosted_receipt(
        &transport,
        "https://runx.test/",
        "rxk_test",
        &receipt,
    )
    .map_err(|error| error.to_string())?;

    assert_eq!(
        response.public_url.as_deref(),
        Some("https://runx.test/r/abc")
    );
    let requests = transport.requests.borrow();
    assert_eq!(requests[0].url, "https://runx.test/v1/receipts/notarize");
    assert_eq!(requests[0].method, HttpMethod::Post);
    assert!(
        requests[0]
            .headers
            .iter()
            .any(|header| header.name == "authorization" && header.value == "Bearer rxk_test")
    );
    assert_eq!(
        request_json_body(&requests[0])?,
        serde_json::from_value::<JsonValue>(
            serde_json::json!({"publish": true, "receipt": {"id": "receipt_1"}})
        )
        .map_err(stringify)?
    );
    Ok(())
}

#[test]
fn human_output_reflects_notary_status() -> Result<(), NotarizeCliError> {
    let output = render_notarize_result(
        false,
        &ReceiptNotarization {
            status: "notarized".to_owned(),
            replay_status: Some("fresh".to_owned()),
            digest: "sha256:abc".to_owned(),
            public_hash: "abc".to_owned(),
            mode: "full".to_owned(),
            published: false,
            public_url: None,
            receipt_id: Some("receipt_1".to_owned()),
            verdict: Some(
                serde_json::from_value(serde_json::json!({"valid": true}))
                    .map_err(|error| NotarizeCliError::Serialize(error.to_string()))?,
            ),
        },
    )?;

    assert!(output.contains("notarized receipt sha256:abc (full)"));
    assert!(output.contains("status:      notarized"));
    assert!(output.contains("published:   false"));
    assert!(output.contains("receipt id:  receipt_1"));
    assert!(output.contains("replay:      fresh"));
    assert!(output.contains(r#"verdict:     {"valid":true}"#));
    Ok(())
}

#[test]
fn notarize_error_explains_receipt_scope_mismatch() {
    let message = NotarizeCliError::Publish(HostedApiOperationError::Api {
        operation: "receipt notarize",
        status: 403,
        code: "missing_scope".to_owned(),
        detail: "Missing required scope: receipts:write.".to_owned(),
        hint: None,
        retry_after_seconds: None,
    })
    .to_string();

    assert!(message.contains("can publish skills but not receipts"));
    assert!(message.contains("receipts:write"));
    assert!(message.contains("RUNX_PUBLIC_API_TOKEN"));
}

fn request_json_body(request: &HttpRequest) -> Result<JsonValue, String> {
    let body = request
        .body
        .as_deref()
        .ok_or_else(|| "request should include a body".to_owned())?;
    serde_json::from_str(body).map_err(stringify)
}

fn stringify(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn tempfile_dir() -> Result<std::path::PathBuf, std::io::Error> {
    let path = std::env::temp_dir().join(format!(
        "runx-cli-notarize-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path)?;
    Ok(path)
}
