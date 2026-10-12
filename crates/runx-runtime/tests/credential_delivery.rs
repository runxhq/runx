use std::collections::BTreeMap;
use std::path::PathBuf;

use runx_parser::ProcessTool;
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::{
    CredentialDelivery, CredentialDeliveryError, Executable, InvocationStatus, SkillAdapter,
    SkillInvocation,
};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[test]
fn a_delivery_observation_uses_a_live_timestamp() -> Result<(), Box<dyn std::error::Error>> {
    let delivery = CredentialDelivery::process_env(
        "github",
        "bearer",
        "GITHUB_TOKEN",
        "local://github/main",
        "ghs_secret_token",
    )?;
    let observation = delivery
        .public_observation()
        .ok_or("a delivery records its public observation")?;

    assert_ne!(observation.observed_at, FIXTURE_CREATED_AT);
    assert!(observation.observed_at.ends_with('Z'));
    Ok(())
}

#[test]
fn public_observation_metadata_serializes_without_secret_material()
-> Result<(), Box<dyn std::error::Error>> {
    let secret = "ghs_observation_secret_must_not_leak";
    let delivery = CredentialDelivery::process_env(
        "github",
        "bearer",
        "GITHUB_TOKEN",
        "local://github/main",
        secret,
    )?;
    let observation = delivery
        .public_observation()
        .ok_or("a delivery records its public observation")?;

    assert_eq!(observation.provider.as_str(), "github");
    assert_eq!(observation.credential_refs.len(), 1);
    assert!(
        observation.credential_refs[0]
            .uri
            .as_str()
            .starts_with("runx:credential:local:")
    );
    assert!(
        !observation.credential_refs[0]
            .uri
            .as_str()
            .contains("local://github/main")
    );
    assert!(
        observation
            .material_ref_hash
            .as_ref()
            .is_some_and(|hash| hash.as_str().starts_with("sha256:"))
    );

    let serialized = serde_json::to_string(&serde_json::json!({
        "credential_delivery_observations": [observation],
    }))?;
    assert!(serialized.contains("credential_delivery_observations"));
    assert!(!serialized.contains(secret));
    assert!(!serialized.contains("GITHUB_TOKEN"));
    assert!(!serialized.contains("local://github/main"));
    Ok(())
}

#[test]
fn process_tool_delivers_and_redacts_declared_credential() -> Result<(), Box<dyn std::error::Error>>
{
    let delivery = allowed_delivery()?;
    let output = ProcessToolAdapter.invoke(SkillInvocation {
        historical_context: Vec::new(),
        skill_name: "credential.echo".to_owned(),
        step_id: None,
        artifacts: None,
        executable: Executable::Process(process_tool()),
        outputs: None,
        inputs: Default::default(),
        resolved_inputs: Default::default(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory: std::env::current_dir()?,
        env: process_env(),
        requirements: Default::default(),
        credential_delivery: delivery,
    })?;

    assert_eq!(output.status, InvocationStatus::Success);
    assert_eq!(
        output.value,
        runx_contracts::JsonValue::String("[redacted-credential]".to_owned())
    );
    assert!(!output.rendered_value().contains("ghs_secret_token"));
    Ok(())
}

#[test]
fn process_tool_omits_truncated_output_before_redaction() -> Result<(), Box<dyn std::error::Error>>
{
    let output = ProcessToolAdapter.invoke(SkillInvocation {
        historical_context: Vec::new(),
        skill_name: "credential.large-output".to_owned(),
        step_id: None,
        artifacts: None,
        executable: Executable::Process(large_output_process_tool()),
        outputs: None,
        inputs: Default::default(),
        resolved_inputs: Default::default(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory: std::env::current_dir()?,
        env: process_env(),
        requirements: Default::default(),
        credential_delivery: CredentialDelivery::none(),
    })?;

    assert_eq!(output.status, InvocationStatus::Failure);
    assert_eq!(output.value, runx_contracts::JsonValue::Null);
    let diagnostic = output.failure_message().unwrap_or_default();
    assert!(diagnostic.contains("stdout/stderr omitted"));
    assert!(!output.rendered_value().contains("ghs_secret_token"));
    assert!(!diagnostic.contains("ghs_secret_token"));
    Ok(())
}

#[test]
fn credential_delivery_redacts_before_truncating() -> Result<(), Box<dyn std::error::Error>> {
    let output = allowed_delivery()?.redact_bytes_to_string(
        b"prefix ghs_secret_token suffix".to_vec(),
        "prefix [redacted-credential]".len(),
    );

    assert_eq!(output, "prefix [redacted-credential]");
    assert!(!output.contains("ghs_secret_token"));
    Ok(())
}

#[test]
fn credential_delivery_redacts_exact_encoded_values_without_destroying_endpoint_context()
-> Result<(), Box<dyn std::error::Error>> {
    use std::collections::BTreeSet;

    use base64::Engine as _;

    let secret = "token \u{ff}/with+reserved=value";
    let delivery = CredentialDelivery::process_env(
        "fixture",
        "api_key",
        "FIXTURE_TOKEN",
        "secret://fixture/encoded",
        secret,
    )?;
    let percent_encoded =
        url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>();
    let form_encoded = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("", secret)
        .finish()
        .strip_prefix('=')
        .ok_or("missing form value")?
        .to_owned();
    let encoded = BTreeSet::from([
        percent_encoded,
        form_encoded,
        base64::engine::general_purpose::STANDARD.encode(secret),
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(secret),
        base64::engine::general_purpose::URL_SAFE.encode(secret),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret),
    ]);
    let redacted = delivery.redact_text(format!(
        "request https://api.example.test/v1/messages?{}",
        encoded.iter().cloned().collect::<Vec<_>>().join("&")
    ));

    assert!(redacted.contains("https://api.example.test/v1/messages?"));
    assert!(!redacted.contains(secret));
    for value in &encoded {
        assert!(!redacted.contains(value));
    }
    assert_eq!(
        redacted.matches("[redacted-credential]").count(),
        encoded.len()
    );
    Ok(())
}

fn allowed_delivery() -> Result<CredentialDelivery, CredentialDeliveryError> {
    CredentialDelivery::process_env(
        "github",
        "api_key",
        "GITHUB_TOKEN",
        "secret://github/main",
        "ghs_secret_token",
    )
}

fn process_tool() -> ProcessTool {
    ProcessTool {
        command: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "printf '\"%s\"\\n' \"$GITHUB_TOKEN\"".to_owned(),
        ],
        cwd: None,
        timeout_seconds: Some(5),
        input_mode: None,
        environment: Default::default(),
    }
}

fn large_output_process_tool() -> ProcessTool {
    ProcessTool {
        command: "node".to_owned(),
        args: vec![
            "-e".to_owned(),
            "process.stdout.write('x'.repeat(8 * 1024 * 1024 + 1));".to_owned(),
        ],
        ..process_tool()
    }
}

fn process_env() -> BTreeMap<String, String> {
    let mut env = std::env::vars().collect::<BTreeMap<_, _>>();
    env.insert(
        "RUNX_CWD".to_owned(),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .to_string_lossy()
            .into_owned(),
    );
    // The repo is the workspace; its runx state stays out of the operator's.
    env.insert(
        "RUNX_PROJECT_DIR".to_owned(),
        PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("credential-delivery-project")
            .to_string_lossy()
            .into_owned(),
    );
    env
}
