#![cfg(feature = "process-tool")]

use std::collections::BTreeMap;
use std::path::PathBuf;

use runx_contracts::CredentialEnvelopeKind;
use runx_contracts::policy_proof::CredentialEnvelope;
use runx_contracts::{CredentialDeliveryMode, CredentialDeliveryPurpose, CredentialMaterialRole};
use runx_contracts::{
    CredentialDeliveryObservation, CredentialDeliveryObservationSchema,
    CredentialDeliveryObservationStatus, Reference, ReferenceType,
};
use runx_core::policy::CredentialBindingDecision;
use runx_parser::ProcessTool;
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::{
    CredentialDelivery, CredentialDeliveryError, CredentialDeliveryProfile, Executable,
    InMemoryMaterialResolver, InvocationStatus, ResolvedCredentialMaterial, SkillAdapter,
    SkillInvocation,
};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";
/// A minimal delivered-credential observation for the resolution tests, which
/// assert on the delivered secret, not on the observation contents.
fn delivered_observation() -> CredentialDeliveryObservation {
    CredentialDeliveryObservation {
        schema: CredentialDeliveryObservationSchema::V1,
        observation_id: "test-credential-delivery".into(),
        request_id: "test-credential-request".into(),
        response_id: None,
        status: CredentialDeliveryObservationStatus::Delivered,
        harness_ref: Reference::with_uri(
            ReferenceType::Harness,
            "runx:harness:test-credential-binding",
        ),
        host_ref: None,
        profile_id: "github-api-key".into(),
        provider: "github".into(),
        purpose: CredentialDeliveryPurpose::ProviderApi,
        delivery_mode: Some(CredentialDeliveryMode::ProcessEnv),
        credential_refs: Vec::new(),
        material_ref_hash: None,
        delivered_roles: vec![CredentialMaterialRole::ApiKey],
        redaction_refs: None,
        observed_at: FIXTURE_CREATED_AT.into(),
    }
}

#[test]
fn delivery_profile_requires_allowed_binding() -> Result<(), Box<dyn std::error::Error>> {
    let result = CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Deny {
            reasons: vec!["grant mismatch".to_owned()],
        },
        &credential(),
        &github_profile()?,
        &resolver(),
        delivered_observation(),
    );

    match result {
        Err(CredentialDeliveryError::BindingDenied { reasons }) => {
            assert_eq!(reasons, vec!["grant mismatch"]);
        }
        Ok(_) => {
            return Err(std::io::Error::other(
                "credential delivery must fail closed on denied binding",
            )
            .into());
        }
        Err(error) => {
            return Err(std::io::Error::other(format!("unexpected error: {error}")).into());
        }
    }
    Ok(())
}

#[test]
fn delivery_profile_rejects_provider_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    let result = CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Allow {
            reasons: vec!["allowed".to_owned()],
        },
        &credential(),
        &CredentialDeliveryProfile::env_token("slack", "api_key", "SLACK_TOKEN")?,
        &resolver(),
        delivered_observation(),
    );

    match result {
        Err(CredentialDeliveryError::ProviderMismatch {
            credential_provider,
            profile_provider,
        }) => {
            assert_eq!(credential_provider, "github");
            assert_eq!(profile_provider, "slack");
        }
        Ok(_) => {
            return Err(std::io::Error::other(
                "credential delivery must reject mismatched providers",
            )
            .into());
        }
        Err(error) => {
            return Err(std::io::Error::other(format!("unexpected error: {error}")).into());
        }
    }
    Ok(())
}

#[test]
fn delivery_profile_maps_process_env_contract_profile() -> Result<(), Box<dyn std::error::Error>> {
    let profile = CredentialDeliveryProfile::from_contract_profile(&contract_profile(
        vec![CredentialMaterialRole::ApiKey],
        "GITHUB_TOKEN",
    ))?;
    let delivery = CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Allow {
            reasons: vec!["allowed".to_owned()],
        },
        &credential(),
        &profile,
        &resolver(),
        delivered_observation(),
    )?;

    assert_eq!(profile.provider(), "github");
    assert_eq!(profile.auth_mode(), "api_key");
    assert_eq!(
        delivery.secret_env().get("GITHUB_TOKEN"),
        Some("ghs_secret_token")
    );
    Ok(())
}

#[test]
fn local_descriptor_observation_uses_live_timestamp() -> Result<(), Box<dyn std::error::Error>> {
    let delivery = CredentialDelivery::from_local_descriptor(
        "github",
        "bearer",
        "GITHUB_TOKEN",
        "local://github/main",
        vec!["repo".to_owned()],
        "ghs_secret_token",
    )?;
    let observation = delivery
        .public_observation()
        .ok_or("local descriptor must record public observation")?;

    assert_ne!(observation.observed_at, FIXTURE_CREATED_AT);
    assert!(observation.observed_at.ends_with('Z'));
    Ok(())
}

#[test]
fn public_observation_metadata_serializes_without_secret_material()
-> Result<(), Box<dyn std::error::Error>> {
    let secret = "ghs_observation_secret_must_not_leak";
    let delivery = CredentialDelivery::from_local_descriptor(
        "github",
        "bearer",
        "GITHUB_TOKEN",
        "local://github/main",
        vec!["repo".to_owned()],
        secret,
    )?;
    let observation = delivery
        .public_observation()
        .ok_or("local descriptor must record public observation")?;

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
fn delivery_profile_skips_optional_missing_contract_binding()
-> Result<(), Box<dyn std::error::Error>> {
    let mut contract = contract_profile(vec![CredentialMaterialRole::ApiKey], "GITHUB_TOKEN");
    contract
        .material_roles
        .push(CredentialMaterialRole::PersonalToken);
    contract
        .env_bindings
        .push(runx_contracts::CredentialDeliveryEnvBinding {
            role: CredentialMaterialRole::PersonalToken,
            env_var: "GITHUB_REFRESH_TOKEN".to_owned(),
            required: false,
        });
    let profile = CredentialDeliveryProfile::from_contract_profile(&contract)?;
    let delivery = CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Allow {
            reasons: vec!["allowed".to_owned()],
        },
        &credential(),
        &profile,
        &resolver(),
        delivered_observation(),
    )?;

    assert_eq!(
        delivery.secret_env().get("GITHUB_TOKEN"),
        Some("ghs_secret_token")
    );
    assert_eq!(delivery.secret_env().get("GITHUB_REFRESH_TOKEN"), None);
    Ok(())
}

#[test]
fn delivery_profile_resolves_contract_client_secret_role() -> Result<(), Box<dyn std::error::Error>>
{
    let profile = CredentialDeliveryProfile::from_contract_profile(&contract_profile(
        vec![CredentialMaterialRole::ClientSecret],
        "GITHUB_CLIENT_SECRET",
    ))?;
    let resolver = InMemoryMaterialResolver::with_material(
        "secret://github/main",
        ResolvedCredentialMaterial::with_role(
            "secret://github/main",
            runx_runtime::CredentialMaterialRole::ClientSecret,
            "client_secret_value",
        ),
    );
    let delivery = CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Allow {
            reasons: vec!["allowed".to_owned()],
        },
        &credential(),
        &profile,
        &resolver,
        delivered_observation(),
    )?;

    assert_eq!(
        delivery.secret_env().get("GITHUB_CLIENT_SECRET"),
        Some("client_secret_value")
    );
    Ok(())
}

#[test]
fn delivery_profile_rejects_empty_material() -> Result<(), Box<dyn std::error::Error>> {
    let resolver = InMemoryMaterialResolver::with_material(
        "secret://github/main",
        ResolvedCredentialMaterial::api_key("secret://github/main", "  "),
    );
    let result = CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Allow {
            reasons: vec!["allowed".to_owned()],
        },
        &credential(),
        &github_profile()?,
        &resolver,
        delivered_observation(),
    );

    assert!(matches!(
        result,
        Err(CredentialDeliveryError::EmptyMaterial { role }) if role == "api_key"
    ));
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
        allowed_tools: None,
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
        runx_contracts::JsonValue::String("[redacted-credential]\n".to_owned())
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
        allowed_tools: None,
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
    assert_eq!(
        output.value,
        runx_contracts::JsonValue::String(String::new())
    );
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
    let delivery = CredentialDelivery::from_local_descriptor(
        "fixture",
        "api_key",
        "FIXTURE_TOKEN",
        "secret://fixture/encoded",
        Vec::new(),
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
    CredentialDelivery::from_allowed_binding(
        &CredentialBindingDecision::Allow {
            reasons: vec!["credential material matches admitted grant".to_owned()],
        },
        &credential(),
        &github_profile()?,
        &resolver(),
        delivered_observation(),
    )
}

fn resolver() -> InMemoryMaterialResolver {
    InMemoryMaterialResolver::with_material(
        "secret://github/main",
        ResolvedCredentialMaterial::api_key("secret://github/main", "ghs_secret_token"),
    )
}

fn github_profile() -> Result<CredentialDeliveryProfile, CredentialDeliveryError> {
    CredentialDeliveryProfile::env_token("github", "api_key", "GITHUB_TOKEN")
}

fn credential() -> CredentialEnvelope {
    CredentialEnvelope {
        kind: CredentialEnvelopeKind::V1,
        grant_id: "grant_github_main".into(),
        provider: "github".into(),
        auth_mode: "api_key".into(),
        material_kind: "api_key".into(),
        provider_reference: "github-main".into(),
        scopes: vec!["repo".into()],
        grant_reference: None,
        material_ref: "secret://github/main".into(),
    }
}

fn contract_profile(
    roles: Vec<CredentialMaterialRole>,
    env_var: &str,
) -> runx_contracts::CredentialDeliveryProfile {
    runx_contracts::CredentialDeliveryProfile {
        schema: runx_contracts::CredentialDeliveryProfileSchema::V1,
        profile_id: "github-provider-api-env".into(),
        provider: "github".into(),
        auth_mode: "api_key".into(),
        purpose: CredentialDeliveryPurpose::ProviderApi,
        delivery_mode: CredentialDeliveryMode::ProcessEnv,
        material_roles: roles.clone(),
        env_bindings: roles
            .into_iter()
            .map(|role| runx_contracts::CredentialDeliveryEnvBinding {
                role,
                env_var: env_var.to_owned(),
                required: true,
            })
            .collect(),
        redaction_policy_ref: runx_contracts::Reference {
            reference_type: runx_contracts::ReferenceType::RedactionPolicy,
            uri: "runx:redaction-policy:credentials-v1".to_owned().into(),
            provider: None,
            locator: None,
            label: None,
            observed_at: None,
            proof_kind: None,
        },
    }
}

fn process_tool() -> ProcessTool {
    ProcessTool {
        command: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            "printf '%s\\n' \"$GITHUB_TOKEN\"".to_owned(),
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
    env
}
