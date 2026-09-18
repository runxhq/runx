use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "cli-tool")]
use base64::Engine;
#[cfg(feature = "cli-tool")]
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
#[cfg(feature = "cli-tool")]
use ring::signature::KeyPair;
use runx_contracts::JsonValue;
#[cfg(all(feature = "cli-tool", feature = "catalog"))]
use runx_receipts::ReceiptTreeConfig;
#[cfg(feature = "cli-tool")]
use runx_runtime::registry::TrustTier;
use runx_runtime::registry::{FileRegistryStore, IngestSkillOptions, ingest_skill_markdown};
use runx_runtime::{
    LocalOrchestrator, LocalReceiptStore, RUNX_RECEIPT_DIR_ENV, RunResult, RuntimeOptions,
    SkillRunRequest,
};
#[cfg(feature = "cli-tool")]
use runx_runtime::{
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_KID_ENV,
};
use tempfile::tempdir;

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";
#[cfg(feature = "cli-tool")]
const TEST_MANIFEST_KEY_ID: &str = "runx-runtime-registry-test-key";
#[cfg(feature = "cli-tool")]
const TEST_MANIFEST_SIGNER_ID: &str = "runx-runtime-registry-test-signer";
#[cfg(feature = "cli-tool")]
const TEST_MANIFEST_SEED: [u8; 32] = [9; 32];

#[cfg(feature = "cli-tool")]
fn registry_child_profile_document() -> String {
    r#"
skill: registry-child
runners:
  child-cli:
    default: true
    type: cli-tool
    command: sh
    args:
      - -c
      - |
        cat >/dev/null
        printf '%s\n' '{"nested":{"message":"registry child"}}'
    input_mode: stdin
    outputs:
      nested: object
"#
    .to_owned()
}

#[cfg(feature = "cli-tool")]
fn trusted_manifest_env() -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    trusted_manifest_env_for_owner("acme", None)
}

#[cfg(feature = "cli-tool")]
fn trusted_manifest_env_for_owner(
    owner: &str,
    source_authority: Option<&str>,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let key_pair = test_manifest_key_pair()?;
    let mut env = [
        (
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV.to_owned(),
            TEST_MANIFEST_KEY_ID.to_owned(),
        ),
        (
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV.to_owned(),
            STANDARD.encode(key_pair.public_key().as_ref()),
        ),
        (
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV.to_owned(),
            owner.to_owned(),
        ),
    ]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    if let Some(source_authority) = source_authority {
        env.insert(
            runx_runtime::registry::RUNX_REGISTRY_SOURCE_AUTHORITY_ENV.to_owned(),
            source_authority.to_owned(),
        );
    }
    Ok(env)
}

#[cfg(feature = "cli-tool")]
fn sign_registry_version(
    registry_dir: &Path,
    skill_id: &str,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let version_path = registry_version_path(registry_dir, skill_id, version)?;
    let mut version_record =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&version_path)?)?;
    version_record["signed_manifest"] = signed_manifest(&version_record)?;
    fs::write(
        version_path,
        format!("{}\n", serde_json::to_string_pretty(&version_record)?),
    )?;
    Ok(())
}

#[cfg(feature = "cli-tool")]
fn tamper_registry_version_markdown(
    registry_dir: &Path,
    skill_id: &str,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let version_path = registry_version_path(registry_dir, skill_id, version)?;
    let mut version_record =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&version_path)?)?;
    let markdown = version_record["markdown"]
        .as_str()
        .ok_or("registry version missing markdown")?;
    version_record["markdown"] =
        serde_json::Value::String(markdown.replace("Registry", "Tampered"));
    fs::write(
        version_path,
        format!("{}\n", serde_json::to_string_pretty(&version_record)?),
    )?;
    Ok(())
}

#[cfg(feature = "cli-tool")]
fn registry_version_path(
    registry_dir: &Path,
    skill_id: &str,
    version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let (owner, name) = skill_id
        .split_once('/')
        .ok_or("registry test skill id must be owner/name")?;
    Ok(registry_dir
        .join(owner)
        .join(name)
        .join(format!("{version}.json")))
}

#[cfg(feature = "cli-tool")]
fn signed_manifest(
    version_record: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let skill_id = version_record["skill_id"]
        .as_str()
        .ok_or("missing skill_id")?;
    let version = version_record["version"]
        .as_str()
        .ok_or("missing version")?;
    let digest = version_record["digest"].as_str().ok_or("missing digest")?;
    let profile_digest = version_record["profile_digest"].as_str();
    let package_digest = version_record["package_digest"].as_str();
    let payload =
        registry_manifest_payload(skill_id, version, digest, profile_digest, package_digest);
    let signature = test_manifest_key_pair()?.sign(payload.as_bytes());
    Ok(serde_json::json!({
        "schema": runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA,
        "skill_id": skill_id,
        "version": version,
        "digest": digest,
        "profile_digest": profile_digest,
        "package_digest": package_digest,
        "signer": {
            "id": TEST_MANIFEST_SIGNER_ID,
            "key_id": TEST_MANIFEST_KEY_ID,
        },
        "signature": {
            "alg": "ed25519",
            "value": format!(
                "base64:{}",
                URL_SAFE_NO_PAD.encode(signature.as_ref())
            ),
        },
    }))
}

#[cfg(feature = "cli-tool")]
fn registry_manifest_payload(
    skill_id: &str,
    version: &str,
    digest: &str,
    profile_digest: Option<&str>,
    package_digest: Option<&str>,
) -> String {
    format!(
        "{}\nskill_id={skill_id}\nversion={version}\ndigest={digest}\nprofile_digest={}\npackage_digest={}\nsigner_id={TEST_MANIFEST_SIGNER_ID}\nkey_id={TEST_MANIFEST_KEY_ID}\n",
        runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA,
        profile_digest.unwrap_or(""),
        package_digest.unwrap_or("")
    )
}

#[cfg(feature = "cli-tool")]
fn test_manifest_key_pair() -> Result<ring::signature::Ed25519KeyPair, std::io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&TEST_MANIFEST_SEED).map_err(|error| {
        std::io::Error::other(format!("static registry manifest seed rejected: {error:?}"))
    })
}

#[test]
fn runtime_options_local_development_uses_live_timestamp() {
    let options = RuntimeOptions::local_development(std::env::vars().collect());

    assert_ne!(options.created_at, FIXTURE_CREATED_AT);
    assert!(options.created_at.ends_with('Z'));
    assert!(options.created_at.contains('T'));
}

#[test]
fn native_skill_run_pauses_with_agent_act_request() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let expected_instructions = fs::read_to_string(skill_dir.join("SKILL.md"))?;
    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: None,
        run_id: None,
        answers_path: None,
        inputs: [(
            "thread_title".to_owned(),
            JsonValue::String("Docs bug".to_owned()),
        )]
        .into_iter()
        .collect(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "schema"), Some("runx.skill_run.v1"));
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run id")?;
    assert!(run_id.starts_with("run_agent_task-issue-intake-output_"));
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(string_field(request, "kind"), Some("agent_act"));
    assert_eq!(
        string_field(request, "id"),
        Some("agent_task.issue-intake.output")
    );
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    assert_eq!(string_field(invocation, "source_type"), Some("agent-task"));
    let envelope = object_field(invocation, "envelope").ok_or("missing envelope")?;
    assert_eq!(string_field(envelope, "run_id"), Some(run_id));
    let instructions = string_field(envelope, "instructions").ok_or("missing instructions")?;
    assert_eq!(instructions, expected_instructions);
    let inputs = object_field(envelope, "inputs").ok_or("missing inputs")?;
    assert_eq!(
        inputs.get("thread_title"),
        Some(&JsonValue::String("Docs bug".to_owned()))
    );
    assert!(
        object_field(envelope, "execution_location")
            .and_then(|location| string_field(location, "skill_directory"))
            .is_some()
    );

    let other = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: None,
        answers_path: None,
        inputs: [(
            "thread_title".to_owned(),
            JsonValue::String("Different docs bug".to_owned()),
        )]
        .into_iter()
        .collect(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let other = object(&other.output, "second skill run result")?;
    assert_ne!(
        string_field(other, "run_id"),
        Some(run_id),
        "agent checkpoints with different inputs must never collide"
    );

    Ok(())
}

#[test]
fn configured_model_credentials_do_not_enable_managed_agent_without_run_consent()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let env = BTreeMap::from([
        ("RUNX_AGENT_PROVIDER".to_owned(), "anthropic".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "claude-test".to_owned()),
        ("RUNX_AGENT_API_KEY".to_owned(), "test-secret".to_owned()),
    ]);
    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "configured no-consent result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    Ok(())
}

#[test]
fn native_agent_task_skill_run_infers_bundled_tool_roots() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let bundled_tools = skill_dir.join("tools");
    fs::create_dir_all(&bundled_tools)?;
    // The runtime canonicalizes inferred tool roots; macOS tempdirs resolve
    // through /private, so compare against the canonical form.
    let bundled_tools = bundled_tools.canonicalize()?;

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let request = object(
        array_field(output, "requests")
            .and_then(|requests| requests.first())
            .ok_or("missing request")?,
        "request",
    )?;
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    let envelope = object_field(invocation, "envelope").ok_or("missing envelope")?;
    let execution_location =
        object_field(envelope, "execution_location").ok_or("missing execution_location")?;
    let tool_roots = array_field(execution_location, "tool_roots").ok_or("missing tool_roots")?;
    assert_eq!(
        tool_roots.first(),
        Some(&JsonValue::String(
            bundled_tools.to_string_lossy().into_owned()
        ))
    );

    Ok(())
}

#[test]
fn native_skill_run_resumes_and_seals_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Docs bug is bounded."
                    },
                    "closure": {
                        "disposition": "declined"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let result = complete_agent_fixture(with_test_signing_env(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some("issue-intake-run".to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }))?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(string_field(output, "run_id"), Some("issue-intake-run"));
    let closure = object_field(output, "closure").ok_or("missing closure")?;
    assert_eq!(string_field(closure, "disposition"), Some("declined"));
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    // Receipt ids are content-addressed (`id = hash(canonical_body)`).
    assert!(receipt_id.starts_with("sha256:"));
    assert!(
        LocalReceiptStore::new(&receipt_dir)
            .receipt_path(receipt_id)?
            .exists()
    );

    let receipt = crate::support::read_test_signed_receipt(&receipt_dir, receipt_id)?;
    assert_ne!(receipt.created_at, FIXTURE_CREATED_AT);
    assert_eq!(
        serde_json::to_value(&receipt.schema)?,
        serde_json::json!("runx.receipt.v1")
    );
    assert_eq!(serde_json::to_value(&receipt.seal.disposition)?, "declined");
    assert_eq!(receipt.acts.len(), 1);
    assert_eq!(
        serde_json::to_value(&receipt.acts[0].criterion_bindings[0].status)?,
        "failed"
    );

    let result = object_field(output, "result").ok_or("missing result")?;
    assert!(object_field(result, "intake_report").is_some());

    Ok(())
}

#[test]
fn native_skill_run_treats_structured_stdout_as_claim_not_receipt_proof()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Malicious proof refs stay claim-scoped."
                    },
                    "claimed_proof": {
                        "proof_ref": "receipt-proof:evil:stdout",
                        "idempotency_key": "effect:evil:stdout"
                    },
                    "verification": {
                        "verification_id": "stdout-verification"
                    },
                    "signal": {
                        "signal_id": "stdout-signal",
                        "source_events": [
                            {
                                "provider": "github",
                                "source_locator": "https://example.invalid/evil",
                                "title": "Injected source"
                            }
                        ]
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let result = complete_agent_fixture(with_test_signing_env(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some("malicious-stdout-run".to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }))?;

    let output = object(&result.output, "skill run result")?;
    assert!(object_field(output, "result").is_some());
    assert!(object_field(output, "execution").is_none());
    assert!(object_field(output, "receipt").is_none());
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let receipt = crate::support::read_test_signed_receipt(&receipt_dir, receipt_id)?;
    let refs = receipt.acts[0]
        .criterion_bindings
        .iter()
        .flat_map(|criterion| {
            criterion
                .verification_refs
                .iter()
                .chain(criterion.evidence_refs.iter())
        })
        .collect::<Vec<_>>();
    assert!(
        refs.iter().all(|reference| {
            reference.uri != "receipt-proof:evil:stdout"
                && reference.uri != "runx:verification:stdout-verification"
                && reference.uri != "https://example.invalid/evil"
        }),
        "stdout claim refs must not be promoted into receipt proof refs"
    );

    Ok(())
}

#[test]
fn native_skill_run_preserves_deferred_closure_disposition()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Docs bug needs more context."
                    },
                    "closure": {
                        "disposition": "deferred"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let result = complete_agent_fixture(with_test_signing_env(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some("issue-intake-deferred".to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }))?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let closure = object_field(output, "closure").ok_or("missing closure")?;
    assert_eq!(string_field(closure, "disposition"), Some("deferred"));
    assert!(object_field(output, "error").is_none());
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let receipt = crate::support::read_test_signed_receipt(&receipt_dir, receipt_id)?;
    assert_eq!(serde_json::to_value(&receipt.seal.disposition)?, "deferred");

    Ok(())
}

#[test]
fn native_skill_run_uses_runtime_receipt_path_resolution() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let env_receipt_dir = temp.path().join("env-receipts");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Docs bug is bounded."
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let result = complete_agent_fixture(with_test_signing_env(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: Some("env-receipt-run".to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: [(
            RUNX_RECEIPT_DIR_ENV.to_owned(),
            env_receipt_dir.to_string_lossy().into_owned(),
        )]
        .into_iter()
        .collect(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }))?;

    let output = object(&result.output, "skill run result")?;
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    assert!(
        LocalReceiptStore::new(&env_receipt_dir)
            .receipt_path(receipt_id)?
            .exists()
    );

    Ok(())
}

#[test]
#[cfg(feature = "cli-tool")]
fn explicit_receipt_dir_is_available_to_cli_tool_subprocess()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("receipt-env-skill");
    let receipt_dir = temp.path().join("explicit-receipts");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: receipt-env-skill\ndescription: Test receipt store propagation.\n---\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: receipt-env-skill
runners:
  inspect:
    default: true
    type: cli-tool
    command: sh
    args:
      - -c
      - printf '{"receipt_dir":"%s","verify_kid":"%s","verify_key":"%s"}\n' "$RUNX_RECEIPT_DIR" "$RUNX_RECEIPT_VERIFY_KID" "$RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64"
    outputs:
      receipt_dir: string
      verify_kid: string
      verify_key: string
"#,
    )?;

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: [
            (
                RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(),
                "receipt-verifier".to_owned(),
            ),
            (
                RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
                "public-key-material".to_owned(),
            ),
        ]
        .into_iter()
        .collect(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "skill run result")?;
    let structured = object_field(output, "result").ok_or("missing output")?;
    assert_eq!(
        string_field(structured, "receipt_dir"),
        Some(receipt_dir.to_string_lossy().as_ref())
    );
    assert_eq!(
        string_field(structured, "verify_kid"),
        Some("receipt-verifier")
    );
    assert_eq!(
        string_field(structured, "verify_key"),
        Some("public-key-material")
    );
    Ok(())
}

#[test]
#[cfg(feature = "cli-tool")]
fn native_graph_runs_a_javascript_module_without_manifest_process_plumbing()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("javascript-module-skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: javascript-module-skill\ndescription: Exercise the native JavaScript module boundary.\n---\n# JavaScript Module\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: javascript-module-skill
runners:
  main:
    default: true
    type: graph
    inputs:
      value:
        type: string
        required: true
    graph:
      name: javascript-module-skill
      result_from:
        - transform
      steps:
        - id: transform
          inputs:
            value: $input.value
          run:
            type: javascript
            module: domain.mjs
            export: transform
            outputs:
              transformed: object
          artifacts:
            named_emits:
              transformed: transformed
            packets:
              transformed: runx.test.transformed.v1
"#,
    )?;
    fs::create_dir_all(skill_dir.join("packets"))?;
    fs::write(
        skill_dir.join("packets/transformed.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-runx-packet-id": "runx.test.transformed.v1",
  "type": "object",
  "required": ["value"],
  "properties": { "value": { "type": "string" } },
  "additionalProperties": false
}"#,
    )?;
    fs::write(
        skill_dir.join("domain.mjs"),
        "export const transform = ({ value }) => ({ transformed: { value: value.toUpperCase() } });\n",
    )?;
    let env = path_env();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::from([("value".to_owned(), JsonValue::String("runx".to_owned()))]),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "javascript module result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let result = object_field(output, "result").ok_or("missing result")?;
    let transformed = object_field(result, "transformed").ok_or("missing transformed output")?;
    let transformed =
        object_field(transformed, "data").ok_or("missing transformed data envelope")?;
    assert_eq!(string_field(transformed, "value"), Some("RUNX"));
    let steps = object_field(output, "trace")
        .and_then(|trace| trace.get("steps"))
        .and_then(JsonValue::as_array)
        .ok_or("missing graph steps")?;
    let receipt_id = steps
        .first()
        .and_then(JsonValue::as_object)
        .and_then(|step| string_field(step, "receipt_id"))
        .ok_or("missing transform receipt")?;
    let receipt =
        crate::support::read_test_signed_receipt(&temp.path().join("receipts"), receipt_id)?;
    assert!(
        receipt
            .seal
            .criteria
            .iter()
            .any(|criterion| { criterion.criterion_id.as_str() == "packet_schemas_verified" })
    );
    Ok(())
}

#[test]
#[cfg(feature = "cli-tool")]
fn native_javascript_runner_rejects_a_packet_schema_violation()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("invalid-javascript-packet");
    fs::create_dir_all(skill_dir.join("packets"))?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: invalid-javascript-packet\ndescription: Reject an invalid deterministic packet.\n---\n# Invalid packet\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: invalid-javascript-packet
runners:
  main:
    default: true
    type: javascript
    module: domain.mjs
    outputs:
      result: object
    artifacts:
      named_emits:
        result: result
      packets:
        result: runx.test.result.v1
"#,
    )?;
    fs::write(
        skill_dir.join("domain.mjs"),
        "export default () => ({ result: { value: 42 } });\n",
    )?;
    fs::write(
        skill_dir.join("packets/result.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "x-runx-packet-id": "runx.test.result.v1",
  "type": "object",
  "required": ["value"],
  "properties": { "value": { "type": "string" } },
  "additionalProperties": false
}"#,
    )?;
    let env = path_env();

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("invalid deterministic packet must fail before sealing".into()),
        Err(error) => error,
    };

    assert!(error.to_string().contains("output violates schema"));
    Ok(())
}

#[test]
fn native_skill_run_uses_production_receipt_signing_env() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Docs bug is bounded."
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let env = crate::support::test_signing_env();

    let result = complete_agent_fixture(with_test_signing_env(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some("production-signed-run".to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }))?;

    let output = object(&result.output, "skill run result")?;
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let signature_config = crate::support::test_signature_config()?;
    let receipt = runx_runtime::LocalReceiptStore::new(&receipt_dir)
        .read_exact_with_policy(receipt_id, signature_config.signature_policy())?;
    assert_eq!(receipt.issuer.kid, "runx-runtime-prod-fixture-key");
    assert!(receipt.signature.value.starts_with("base64:"));
    assert!(!receipt.signature.value.starts_with("sig:"));

    Ok(())
}

#[test]
fn native_skill_run_uses_local_development_without_production_receipt_signing_env()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Docs bug is bounded."
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let result = complete_agent_fixture(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: Some("local-development-signed-run".to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    assert_eq!(result.status, runx_runtime::RunStatus::Sealed);
    assert_eq!(result.receipt_refs.len(), 1);
    Ok(())
}

#[test]
fn native_graph_skill_run_pauses_and_resumes_agent_task() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Graph bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let initial = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&initial.output, "graph skill run result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(
        string_field(request, "id"),
        Some("agent_task.graph-decide.output")
    );
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    let envelope = object_field(invocation, "envelope").ok_or("missing envelope")?;
    let instructions = string_field(envelope, "instructions").ok_or("missing instructions")?;
    assert!(instructions.contains("# Graph Issue To PR"));
    assert!(instructions.contains("Use the full issue context."));
    let envelope_inputs = object_field(envelope, "inputs").ok_or("missing inputs")?;
    assert_eq!(
        envelope_inputs.get("thread_title"),
        Some(&JsonValue::String("Graph bug".to_owned()))
    );

    let state_path = receipt_dir
        .join("runs")
        .join(format!("{run_id}.graph-state.json"));
    let original_state = fs::read_to_string(&state_path)?;
    assert!(
        fs::read_dir(state_path.parent().ok_or("missing graph state parent")?)?
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
        "graph state writes must not leave temporary files behind"
    );
    fs::write(&state_path, "{")?;
    let malformed_answers_path = temp.path().join("malformed-graph-answers.json");
    fs::write(&malformed_answers_path, "{}")?;
    let malformed = match run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(malformed_answers_path),
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("malformed graph state should fail".into()),
        Err(error) => error,
    };
    assert!(
        malformed.to_string().contains("graph state file")
            && malformed.to_string().contains("cannot resume safely"),
        "malformed graph state must fail with a clear resume error; got: {malformed}"
    );
    fs::write(&state_path, &original_state)?;

    let original_state_value: JsonValue = serde_json::from_str(&original_state)?;
    let original_state_object = object(&original_state_value, "graph state")?;
    assert!(
        string_field(original_state_object, "package_digest").is_some(),
        "graph state must bind its skill package"
    );
    assert!(
        string_field(original_state_object, "execution_closure_digest").is_some(),
        "graph state must bind its full execution closure"
    );

    let bad_answers_path = temp.path().join("bad-graph-answers.json");
    fs::write(&bad_answers_path, "{}")?;
    let mut mismatched_binding: JsonValue = serde_json::from_str(&original_state)?;
    object_mut(&mut mismatched_binding, "graph state")?.insert(
        "package_digest".to_owned(),
        JsonValue::String("sha256:stale-package".to_owned()),
    );
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&mismatched_binding)?,
    )?;
    let binding_mismatch = match run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(bad_answers_path.clone()),
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("graph state with a stale package binding should fail".into()),
        Err(error) => error,
    };
    assert!(
        binding_mismatch
            .to_string()
            .contains("graph state package_digest mismatch")
    );
    fs::write(&state_path, &original_state)?;

    let mut mismatched_state: JsonValue = serde_json::from_str(&original_state)?;
    object_mut(&mut mismatched_state, "graph state")?.insert(
        "runner_name".to_owned(),
        JsonValue::String("other-runner".to_owned()),
    );
    fs::write(
        &state_path,
        serde_json::to_string_pretty(&mismatched_state)?,
    )?;
    let mismatch = match run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(bad_answers_path),
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("mismatched graph state should fail".into()),
        Err(error) => error,
    };
    assert!(
        mismatch
            .to_string()
            .contains("graph state runner_name mismatch")
    );
    fs::write(&state_path, original_state)?;

    let answers_path = temp.path().join("graph-answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.graph-decide.output": {
                    "result": {
                        "summary": "Graph fix authored."
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let resumed = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(answers_path),
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&resumed.output, "resumed graph skill run result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let declared_result =
        object_field(public_result, "result").ok_or("missing declared result output")?;
    assert_eq!(
        string_field(declared_result, "summary"),
        Some("Graph fix authored.")
    );
    let completed_state: JsonValue = serde_json::from_str(&fs::read_to_string(&state_path)?)?;
    let completed_state = object(&completed_state, "completed graph state")?;
    let completed_checkpoint =
        object_field(completed_state, "checkpoint").ok_or("missing completed checkpoint")?;
    let completed_graph =
        object_field(completed_checkpoint, "state").ok_or("missing completed graph")?;
    assert_eq!(
        string_field(completed_graph, "status"),
        Some("succeeded"),
        "the durable checkpoint must agree with the sealed graph receipt"
    );

    Ok(())
}

#[test]
fn native_graph_transition_gate_allows_declared_agent_output()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_gated_agent_task_skill_with_field(temp.path(), "decide.approved")?;
    let receipt_dir = temp.path().join("receipts");

    let initial = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&initial.output, "gated graph result")?;
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;

    let answers_path = temp.path().join("gated-answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.gated-decide.output": {
                    "approved": true,
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let resumed = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&resumed.output, "resumed gated graph result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(
        string_field(request, "id"),
        Some("agent_task.gated-followup.output")
    );

    Ok(())
}

#[test]
fn native_graph_guard_rejects_skill_claim_as_fact() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_gated_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");

    let initial = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&initial.output, "gated graph result")?;
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;

    let answers_path = temp.path().join("gated-answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.gated-decide.output": {
                    "approved": true,
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let blocked = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&blocked.output, "blocked graph result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let closure = object_field(output, "closure").ok_or("missing closure")?;
    assert_eq!(string_field(closure, "disposition"), Some("blocked"));
    assert_eq!(string_field(closure, "reason_code"), Some("graph_blocked"));
    assert!(
        string_field(closure, "summary")
            .unwrap_or_default()
            .contains("guard 'decide.skill_claim.approved' is unresolved"),
        "unexpected closure: {closure:?}"
    );

    Ok(())
}

#[test]
fn native_graph_skill_run_pauses_and_resumes_nested_agent_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_agent_skill(temp.path(), "agent")?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Nested agent bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let initial = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&initial.output, "nested agent graph result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(
        string_field(request, "id"),
        Some("agent.child-agent.output")
    );
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    assert_eq!(string_field(invocation, "source_type"), Some("agent"));

    let answers_path = temp.path().join("nested-agent-answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent.child-agent.output": {
                    "result": {
                        "summary": "Nested agent fix authored."
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let resumed = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(answers_path),
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&resumed.output, "resumed nested agent graph result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let result = object_field(public_result, "result").ok_or("missing declared result")?;
    assert_eq!(
        string_field(result, "summary"),
        Some("Nested agent fix authored.")
    );
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    assert_eq!(array_field(trace, "steps").map(Vec::len), Some(1));

    Ok(())
}

#[test]
fn native_graph_skill_run_pauses_and_resumes_nested_agent_task_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_agent_skill(temp.path(), "agent-task")?;
    let receipt_dir = temp.path().join("receipts");

    let initial = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&initial.output, "nested agent-task graph result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(
        string_field(request, "id"),
        Some("agent_task.child-agent-task.output")
    );
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    assert_eq!(string_field(invocation, "source_type"), Some("agent-task"));

    let answers_path = temp.path().join("nested-agent-task-answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.child-agent-task.output": {
                    "result": {
                        "summary": "Nested agent-task fix authored."
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let resumed = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&resumed.output, "resumed nested agent-task graph result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let result = object_field(public_result, "result").ok_or("missing declared result")?;
    assert_eq!(
        string_field(result, "summary"),
        Some("Nested agent-task fix authored.")
    );

    Ok(())
}

#[test]
fn graph_agent_task_injects_registry_skill_as_current_context()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        r#"---
name: taste-profile
description: Portable taste guidance for downstream agents.
---
# Taste Profile

Prefer clear product taste over ornamental flourish. Flag incoherent hierarchy,
weak contrast, and interaction states that feel bolted on.
"#,
        IngestSkillOptions {
            owner: Some("runx".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(
                r#"skill: taste-profile
runners:
  main:
    default: true
    type: agent
    agent: critic
    task: apply taste judgement
"#
                .to_owned(),
            ),
            ..IngestSkillOptions::default()
        },
    )?;
    let skill_dir = write_graph_agent_task_with_context_skill(
        temp.path(),
        "registry:runx/taste-profile@1.0.0",
    )?;
    let env = [(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "registry context graph result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    let envelope = object_field(invocation, "envelope").ok_or("missing envelope")?;
    let current_context =
        array_field(envelope, "current_context").ok_or("missing current_context")?;
    assert_eq!(current_context.len(), 1);
    let context_entry = object(&current_context[0], "skill context entry")?;
    assert_eq!(
        string_field(context_entry, "type"),
        Some("runx.skill.context")
    );
    let data = object_field(context_entry, "data").ok_or("missing context data")?;
    assert_eq!(string_field(data, "source"), Some("runx-registry"));
    assert_eq!(string_field(data, "skill_id"), Some("runx/taste-profile"));
    assert_eq!(string_field(data, "version"), Some("1.0.0"));
    assert_eq!(string_field(data, "content_kind"), Some("skill-manual"));
    assert_eq!(
        string_field(data, "description"),
        Some("Portable taste guidance for downstream agents.")
    );
    assert!(string_field(data, "manual_sha256").is_some_and(|hash| hash.starts_with("sha256:")));
    assert!(string_field(data, "profile_sha256").is_some_and(|hash| hash.starts_with("sha256:")));
    assert!(
        string_field(data, "content").is_some_and(|manual| manual.contains("# Taste Profile")
            && manual.contains("Prefer clear product taste"))
    );
    let meta = object_field(context_entry, "meta").ok_or("missing context meta")?;
    assert!(string_field(meta, "hash").is_some_and(|hash| hash.starts_with("sha256:")));

    Ok(())
}

#[test]
fn graph_agent_task_rejects_parent_path_context_skill() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let context_dir = temp.path().join("taste-profile");
    fs::create_dir_all(&context_dir)?;
    fs::write(
        context_dir.join("SKILL.md"),
        "---\nname: taste-profile\n---\n# Taste Profile\n",
    )?;
    let skill_dir = write_graph_agent_task_with_context_skill(temp.path(), "../taste-profile")?;

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("parent-path context skill should fail".into()),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("context skill ref \"../taste-profile\" must not traverse the package"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[test]
fn graph_agent_task_rejects_graph_stage_context_skill() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_task_with_context_skill(temp.path(), "context-stage")?;
    let stage_dir = skill_dir.join("context-stage");
    fs::create_dir_all(&stage_dir)?;
    fs::write(
        stage_dir.join("SKILL.md"),
        r#"---
name: context-stage
---
# Context Stage
"#,
    )?;
    fs::write(
        stage_dir.join("X.yaml"),
        r#"skill: context-stage
catalog:
  kind: skill
  audience: builder
  visibility: internal
  role: graph-stage
  part_of:
    - graph-agent-context-skill
runners:
  main:
    default: true
    type: agent
    agent: builder
    task: internal implementation detail
"#,
    )?;

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("graph stage context skill should fail".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("catalog.role=graph-stage"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[test]
fn graph_agent_task_rejects_registry_runtime_path_context_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        r#"---
name: runtime-helper
description: Internal runtime helper.
---
# Runtime Helper
"#,
        IngestSkillOptions {
            owner: Some("sourcey".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(
                r#"skill: runtime-helper
catalog:
  kind: skill
  audience: builder
  visibility: internal
  role: runtime-path
  part_of:
    - graph-agent-context-skill
runners:
  main:
    default: true
    type: agent
    agent: builder
    task: internal helper
"#
                .to_owned(),
            ),
            ..IngestSkillOptions::default()
        },
    )?;
    let skill_dir = write_graph_agent_task_with_context_skill(
        temp.path(),
        "registry:sourcey/runtime-helper@1.0.0",
    )?;
    let env = [(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("registry runtime-path context skill should fail".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("catalog.role=runtime-path"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_run_executes_local_tool_step() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_tool_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Graph tool bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs,
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "graph tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(string_field(echo, "message"), Some("Graph tool bug"));

    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn configured_model_credentials_do_not_enable_graph_agent_without_run_consent()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_task_skill(temp.path())?;
    let env = BTreeMap::from([
        ("RUNX_AGENT_PROVIDER".to_owned(), "anthropic".to_owned()),
        ("RUNX_AGENT_MODEL".to_owned(), "claude-test".to_owned()),
        ("RUNX_AGENT_API_KEY".to_owned(), "test-secret".to_owned()),
    ]);
    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "configured graph no-consent result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_run_resolves_agent_task_named_emit_context()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_artifact_context_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.graph-author.output": {
                    "fix_bundle": {
                        "message": "Graph tool bug"
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let pending = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let pending_output = object(&pending.output, "pending graph agent artifact result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending graph agent artifact result missing run_id")?
        .to_owned();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "graph agent artifact result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(string_field(echo, "message"), Some("Graph tool bug"));

    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_resume_preserves_initial_inputs_for_later_tool_steps()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_then_input_tool_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.graph-author.output": {
                    "result": {
                        "accepted": true
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let initial_inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Graph tool bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let pending = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: initial_inputs,
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let pending_output = object(&pending.output, "pending graph input resume result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending graph input resume result missing run_id")?
        .to_owned();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "graph input resume result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(string_field(echo, "message"), Some("Graph tool bug"));

    Ok(())
}

// Transport-envelope probing is intentionally forbidden: an agent claim must
// return the declared contract directly, so an `output`-wrapped answer fails
// with the typed missing-output error instead of being silently unwrapped.
#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_run_rejects_agent_task_output_envelope_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_artifact_context_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers_path = temp.path().join("answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.graph-author.output": {
                    "output": {
                        "fix_bundle": {
                            "message": "Graph tool bug"
                        }
                    },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let pending = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let pending_output = object(
        &pending.output,
        "pending graph agent artifact envelope result",
    )?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending graph agent artifact envelope result missing run_id")?
        .to_owned();

    let rejected = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&rejected.output, "rejected agent envelope result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let closure = object_field(output, "closure").ok_or("missing rejection closure")?;
    assert_eq!(string_field(closure, "disposition"), Some("failed"));
    let result = object_field(output, "result").ok_or("missing rejection result")?;
    let message = string_field(result, "message").ok_or("missing rejection message")?;
    assert!(
        message.contains("runner output contract violation at $.output"),
        "unexpected envelope rejection: {message}"
    );

    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_run_omits_missing_optional_graph_input_references()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_optional_json_tool_skill(temp.path())?;
    write_optional_json_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Graph optional JSON bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs,
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "graph optional JSON tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(
        string_field(echo, "message"),
        Some("Graph optional JSON bug")
    );

    Ok(())
}

#[test]
fn native_graph_skill_run_requires_declared_graph_inputs() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_graph_required_input_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");

    let request = with_test_signing_env(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    });
    let error = LocalOrchestrator::default()
        .run_skill(&request)
        .err()
        .ok_or("missing required input must refuse before execution")?;
    let runx_runtime::OrchestratorError::SkillRun(
        runx_runtime::execution::skill_front::SkillRunError::PreflightRefused {
            source,
            receipt_id,
        },
    ) = error
    else {
        return Err(format!("expected a signed input refusal, got {error}").into());
    };
    assert!(
        source
            .to_string()
            .contains("runner input 'lead' is required")
    );
    let receipt = crate::support::read_test_signed_receipt(&receipt_dir, &receipt_id)?;
    assert_eq!(serde_json::to_value(&receipt.seal.disposition)?, "blocked");
    assert!(
        !receipt_dir.join("runs").exists(),
        "refusal must not leave an unusable continuation"
    );
    Ok(())
}

#[test]
fn native_graph_skill_resume_applies_approval_before_completing_step()
-> Result<(), Box<dyn std::error::Error>> {
    let case = "approvals-field";
    let answers = serde_json::json!({
        "approvals": { "approval-resume.approve": true }
    });
    let temp = tempdir()?;
    let skill_dir = write_graph_approval_resume_skill(temp.path())?;
    let receipt_dir = temp.path().join(format!("receipts-{case}"));
    let answers_path = temp.path().join(format!("answers-{case}.json"));
    fs::write(&answers_path, answers.to_string())?;

    let pending = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let pending_output = object(&pending.output, "pending approval result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending approval result missing run_id")?
        .to_owned();

    let resumed = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let resumed_output = object(&resumed.output, "resumed approval result")?;
    assert_eq!(string_field(resumed_output, "status"), Some("sealed"));
    let result = object_field(resumed_output, "result").ok_or("missing result")?;
    let approval_packet =
        object_field(result, "approval_decision").ok_or("missing approval packet")?;
    let approval_data = object_field(approval_packet, "data").ok_or("missing approval data")?;
    assert_eq!(approval_data.get("approved"), Some(&JsonValue::Bool(true)));
    assert_eq!(string_field(approval_data, "status"), Some("approved"));
    assert_eq!(
        string_field(approval_data, "gate_type"),
        Some("test.claim-bound")
    );

    Ok(())
}

#[test]
fn native_graph_skill_resume_rejects_agent_answer_for_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_approval_resume_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts-agent-approval");
    let answers_path = temp.path().join("answers-agent-approval.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": { "approval-resume.approve": true }
        })
        .to_string(),
    )?;

    let pending = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let pending_output = object(&pending.output, "pending approval result")?;
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending approval result missing run_id")?
        .to_owned();

    let rejected = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&rejected.output, "rejected agent approval result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let closure = object_field(output, "closure").ok_or("missing rejection closure")?;
    assert_eq!(string_field(closure, "disposition"), Some("failed"));
    let result = object_field(output, "result").ok_or("missing rejection result")?;
    assert!(
        string_field(result, "message")
            .is_some_and(|message| message.contains("host-attested human"))
    );

    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_run_uses_canonical_tool_root() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_tool_skill_under_skills(temp.path())?;
    write_echo_tool_at(&temp.path().join("tools/test/echo"), "root tools")?;
    write_echo_tool_at(
        &temp.path().join("packages/cli/tools/test/echo"),
        "stale copy",
    )?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Graph tool bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "graph tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo_data = object_field(echo, "data").ok_or("missing echo data")?;
    assert_eq!(string_field(echo_data, "message"), Some("root tools"));

    Ok(())
}

#[cfg(feature = "catalog")]
#[test]
fn native_graph_skill_run_merges_imported_graph_skill_tool_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_importing_graph_with_bundled_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Graph tool bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "nested graph tool root result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing nested echo output")?;
    let echo_data = object_field(echo, "data").ok_or("missing nested echo data")?;
    assert_eq!(
        string_field(echo_data, "message"),
        Some("Nested graph tool root bug")
    );
    let root_receipt_id = string_field(output, "receipt_id").ok_or("missing receipt id")?;
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    let steps = array_field(trace, "steps").ok_or("missing graph steps")?;
    let nested_step = object(&steps[0], "nested graph step")?;
    let nested_step_id =
        string_field(nested_step, "receipt_id").ok_or("missing nested step receipt id")?;
    let root_receipt = crate::support::read_test_signed_receipt(&receipt_dir, root_receipt_id)?;
    let nested_step_receipt =
        crate::support::read_test_signed_receipt(&receipt_dir, nested_step_id)?;
    let nested_graph_ref = nested_step_receipt
        .lineage
        .as_ref()
        .and_then(|lineage| lineage.children.first())
        .ok_or("nested step receipt missing child graph reference")?;
    let nested_graph_id = nested_graph_ref
        .uri
        .as_str()
        .strip_prefix("runx:receipt:")
        .ok_or("nested graph receipt reference is malformed")?;
    let nested_graph_receipt =
        crate::support::read_test_signed_receipt(&receipt_dir, nested_graph_id)?;
    assert_eq!(
        nested_graph_ref.locator.as_deref(),
        Some(nested_graph_receipt.digest.as_str())
    );
    let nested_child_refs = &nested_graph_receipt
        .lineage
        .as_ref()
        .ok_or("nested graph receipt missing lineage")?
        .children;
    assert_eq!(nested_child_refs.len(), 1);
    let nested_child_id = nested_child_refs[0]
        .uri
        .as_str()
        .strip_prefix("runx:receipt:")
        .ok_or("nested child receipt reference is malformed")?;
    let nested_child_receipt =
        crate::support::read_test_signed_receipt(&receipt_dir, nested_child_id)?;
    let signature_config = crate::support::test_signature_config()?;
    let validation = runx_runtime::receipts::tree::validate_runtime_receipt_tree_with_policy(
        &root_receipt,
        vec![
            nested_step_receipt,
            nested_graph_receipt,
            nested_child_receipt,
        ],
        ReceiptTreeConfig::default(),
        signature_config.signature_policy(),
    );
    assert!(
        validation.is_ok(),
        "the persisted nested receipt tree must resolve end to end: {validation:?}"
    );

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_executes_nested_cli_tool_skill() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_cli_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Nested graph bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "nested graph skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let declared_nested =
        object_field(public_result, "nested").ok_or("missing exposed nested output")?;
    assert_eq!(
        object_field(declared_nested, "data").and_then(|nested| string_field(nested, "message")),
        Some("Nested graph bug")
    );
    let context = object_field(output, "context").ok_or("missing graph context")?;
    let step_outputs =
        object_field(context, "step_outputs").ok_or("missing declared step context")?;
    let nested_context =
        object_field(step_outputs, "nested").ok_or("missing nested step context")?;
    assert_eq!(
        object_field(nested_context, "nested")
            .and_then(|nested| object_field(nested, "data"))
            .and_then(|nested| string_field(nested, "message")),
        Some("Nested graph bug")
    );
    let public_json = serde_json::to_string(&result.output)?;
    for retired in [
        "\"execution\"",
        "\"payload\"",
        "\"receipt\"",
        "\"skill_claim\"",
        "\"structured_output\"",
    ] {
        assert!(
            !public_json.contains(retired),
            "public result leaked retired diagnostic field {retired}"
        );
    }
    assert!(
        public_json.len() < 4_096,
        "small nested result expanded to {} bytes",
        public_json.len()
    );
    let root_receipt_id = string_field(output, "receipt_id").ok_or("missing receipt id")?;
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    let steps = array_field(trace, "steps").ok_or("missing graph steps")?;
    let nested_step_summary = object(&steps[0], "nested step summary")?;
    let nested_receipt_id =
        string_field(nested_step_summary, "receipt_id").ok_or("missing nested receipt id")?;
    let receipt_store = LocalReceiptStore::new(&receipt_dir);
    assert!(receipt_store.receipt_path(root_receipt_id)?.exists());
    assert!(receipt_store.receipt_path(nested_receipt_id)?.exists());

    let root_receipt = crate::support::read_test_signed_receipt(&receipt_dir, root_receipt_id)?;
    let child_receipt = crate::support::read_test_signed_receipt(&receipt_dir, nested_receipt_id)?;
    let child_refs = &root_receipt
        .lineage
        .as_ref()
        .ok_or("root receipt missing lineage")?
        .children;
    assert_eq!(child_refs.len(), 1);
    assert_eq!(
        child_refs[0].uri.as_str(),
        format!("runx:receipt:{nested_receipt_id}")
    );
    assert_eq!(
        child_refs[0].locator.as_deref(),
        Some(child_receipt.digest.as_str())
    );
    assert!(
        child_receipt
            .lineage
            .as_ref()
            .and_then(|lineage| lineage.parent.as_ref())
            .is_none(),
        "reusable child receipts must not be rebound to one parent"
    );
    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_executes_nested_registry_skill() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        "---\nname: registry-child\ndescription: Registry-backed nested child.\n---\n# Registry Child\n",
        IngestSkillOptions {
            owner: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(registry_child_profile_document()),
            ..IngestSkillOptions::default()
        },
    )?;
    sign_registry_version(&registry_dir, "acme/registry-child", "1.0.0")?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let mut env = trusted_manifest_env()?;
    env.insert(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    );

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "nested registry skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("registry child"));

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_rejects_env_promoted_official_nested_registry_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        "---\nname: registry-child\ndescription: Official registry-backed nested child.\n---\n# Registry Child\n",
        IngestSkillOptions {
            owner: Some("runx".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(registry_child_profile_document()),
            trust_tier: Some(TrustTier::FirstParty),
            ..IngestSkillOptions::default()
        },
    )?;
    sign_registry_version(&registry_dir, "runx/registry-child", "1.0.0")?;
    let skill_dir = write_graph_nested_registry_skill_with_ref(
        temp.path(),
        "registry:runx/registry-child@1.0.0",
    )?;
    let receipt_dir = temp.path().join("receipts");
    let mut env = trusted_manifest_env_for_owner("runx", Some("official_runx"))?;
    env.insert(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    );

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => {
            return Err(
                "env-promoted official nested registry skill unexpectedly succeeded".into(),
            );
        }
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("trust configuration is invalid"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_rejects_unsigned_nested_registry_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        "---\nname: registry-child\ndescription: Registry-backed nested child.\n---\n# Registry Child\n",
        IngestSkillOptions {
            owner: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(registry_child_profile_document()),
            ..IngestSkillOptions::default()
        },
    )?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let mut env = trusted_manifest_env()?;
    env.insert(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNX_REGISTRY_URL".to_owned(),
        "https://registry.example.test".to_owned(),
    );

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("unsigned nested registry skill unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("signed manifest is required"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_rejects_tampered_nested_registry_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        "---\nname: registry-child\ndescription: Registry-backed nested child.\n---\n# Registry Child\n",
        IngestSkillOptions {
            owner: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(registry_child_profile_document()),
            ..IngestSkillOptions::default()
        },
    )?;
    sign_registry_version(&registry_dir, "acme/registry-child", "1.0.0")?;
    tamper_registry_version_markdown(&registry_dir, "acme/registry-child", "1.0.0")?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let mut env = trusted_manifest_env()?;
    env.insert(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    );

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("tampered nested registry skill unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("digest mismatch"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_rejects_nested_registry_skill_without_registry_dir()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");

    let error = match run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("nested registry skill unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("RUNX_REGISTRY_DIR is not configured"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_does_not_rerun_final_step() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_cli_counter_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let count_file = temp.path().join("count.txt");
    let inputs = [(
        "count_file".to_owned(),
        JsonValue::String(count_file.to_string_lossy().into_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "counter graph skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(fs::read_to_string(count_file)?, "1");

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_executes_graph_stage_cli_tool_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_stage_cli_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Stage graph bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "stage graph skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("Stage graph bug"));
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    let steps = array_field(trace, "steps").ok_or("missing graph steps")?;
    let nested_step_summary = object(&steps[0], "nested step summary")?;
    assert_eq!(
        string_field(nested_step_summary, "skill"),
        Some("child-echo")
    );

    Ok(())
}

#[cfg(feature = "cli-tool")]
#[test]
fn native_graph_skill_run_executes_nested_x_yaml_runner_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_x_yaml_cli_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Runner manifest bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let result = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers_path: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;

    let output = object(&result.output, "nested X.yaml graph skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("Runner manifest bug"));

    Ok(())
}

#[test]
fn native_skill_run_accepts_named_start_and_requires_identity_for_answers()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;

    let named_start = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: None,
        run_id: Some("issue-intake-run".to_owned()),
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&named_start.output, "named start")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    assert_eq!(string_field(output, "run_id"), Some("issue-intake-run"));

    let answers_only = match run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: None,
        run_id: None,
        answers_path: Some(temp.path().join("answers.json")),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    }) {
        Ok(_) => return Err("answers without run-id should fail".into()),
        Err(error) => error,
    };
    assert!(
        answers_only
            .to_string()
            .contains("agent continuation requires run_id")
    );

    let missing_checkpoint = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: Some("missing".to_owned()),
        answers_path: Some(temp.path().join("answers.json")),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })
    .err()
    .ok_or("an unadmitted run must not accept answers")?;
    assert!(
        missing_checkpoint
            .to_string()
            .contains("missing.agent-state.json")
    );
    Ok(())
}

// Exercise durable admission before applying fixture answers; a named answer
// without an admitted checkpoint is never a continuation.
fn complete_agent_fixture(
    request: SkillRunRequest,
) -> Result<RunResult, Box<dyn std::error::Error>> {
    let mut start = request.clone();
    start.answers_path = None;
    let pending = LocalOrchestrator::default().run_skill(&start)?;
    let mut digests = serde_json::Map::new();
    for pending_request in &pending.pending_requests {
        let pending_object = object(pending_request, "pending request")?;
        let id = string_field(pending_object, "id").ok_or("missing pending request id")?;
        digests.insert(
            id.to_owned(),
            serde_json::Value::String(runx_contracts::sha256_prefixed(&serde_json::to_vec(
                pending_request,
            )?)),
        );
    }
    assert!(!digests.is_empty());
    let pending = serde_json::to_value(&pending.output)?;
    assert_eq!(pending["status"], "needs_agent");
    assert_eq!(pending["run_id"].as_str(), request.run_id.as_deref());
    let answers_path = request
        .answers_path
        .as_ref()
        .ok_or("fixture answers are required")?;
    let mut answers: serde_json::Value = serde_json::from_slice(&fs::read(answers_path)?)?;
    answers["request_digests"] = serde_json::Value::Object(digests);
    fs::write(answers_path, serde_json::to_vec(&answers)?)?;
    LocalOrchestrator::default()
        .run_skill(&request)
        .map_err(Into::into)
}

fn run_skill(request: SkillRunRequest) -> Result<RunResult, Box<dyn std::error::Error>> {
    let request = with_test_signing_env(request);
    LocalOrchestrator::default()
        .run_skill(&request)
        .map_err(|error| error.into())
}

fn with_test_signing_env(mut request: SkillRunRequest) -> SkillRunRequest {
    crate::support::insert_test_signing_env(&mut request.env);
    request
        .env
        .entry("RUNX_HOME".to_owned())
        .or_insert_with(|| request.cwd.join(".runx").to_string_lossy().into_owned());
    request
}

fn write_agent_task_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("issue-intake");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: issue-intake\n---\n# Issue Intake\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: issue-intake
runners:
  intake:
    default: true
    type: agent-task
    agent: builder
    task: issue-intake
    outputs:
      intake_report: object
      claimed_proof:
        type: object
        required: false
      verification:
        type: object
        required: false
      signal:
        type: object
        required: false
    inputs:
      thread_title:
        type: string
        required: false
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_graph_agent_task_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-issue-to-pr");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-issue-to-pr\n---\n# Graph Issue To PR\n\nUse the full issue context.\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-issue-to-pr
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: false
    graph:
      name: graph-issue-to-pr
      result_from:
        - decide
      steps:
        - id: decide
          run:
            type: agent-task
            agent: builder
            task: graph-decide
            outputs:
              result: object
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_graph_agent_task_with_context_skill(
    root: &Path,
    context_skill: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-agent-context-skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-agent-context-skill\n---\n# Graph Agent Context Skill\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        format!(
            r#"
skill: graph-agent-context-skill
runners:
  graph:
    default: true
    type: graph
    graph:
      name: graph-agent-context-skill
      result_from:
        - apply_taste
      steps:
        - id: apply_taste
          run:
            type: agent-task
            agent: builder
            task: apply taste guidance
            outputs:
              summary: string
          context_skills:
            - "{context_skill}"
"#
        ),
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_graph_gated_agent_task_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    write_graph_gated_agent_task_skill_with_field(root, "decide.skill_claim.approved")
}

fn write_graph_gated_agent_task_skill_with_field(
    root: &Path,
    gate_field: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-gated-agent-task");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-gated-agent-task\n---\n# Graph Gated Agent Step\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        format!(
            r#"
skill: graph-gated-agent-task
runners:
  graph:
    default: true
    type: graph
    graph:
      name: graph-gated-agent-task
      result_from:
        - gated
      steps:
        - id: decide
          run:
            type: agent-task
            agent: builder
            task: gated-decide
            outputs:
              approved: boolean
        - id: gated
          run:
            type: agent-task
            agent: builder
            task: gated-followup
            outputs:
              result: object
      policy:
        guards:
          - step: gated
            field: {gate_field}
            equals: true
            "#
        ),
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_graph_nested_agent_skill(
    root: &Path,
    source_type: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_name = match source_type {
        "agent" => "child-agent",
        "agent-task" => "child-agent-task",
        _ => return Err(format!("unsupported nested agent source type {source_type}").into()),
    };
    let child_dir = root.join(child_name);
    let runner = if source_type == "agent-task" {
        r#"    type: agent-task
    agent: builder
    task: child-agent-task
    inputs:
      thread_title:
        type: string
        required: false
    outputs:
      result: object
"#
    } else {
        r#"    type: agent
    inputs:
      thread_title:
        type: string
        required: false
    outputs:
      result: object
"#
    };
    crate::support::write_test_skill_package(
        &child_dir,
        format!(
            r#"---
name: {child_name}
---
# {child_name}
"#
        )
        .as_str(),
        format!(
            r#"skill: {child_name}
runners:
  {child_name}:
    default: true
{runner}"#
        )
        .as_str(),
    )?;

    let skill_dir = root.join(format!("graph-nested-{source_type}"));
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: graph-nested-{source_type}\n---\n# Graph Nested {source_type}\n"),
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        format!(
            r#"
skill: graph-nested-{source_type}
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: false
    graph:
      name: graph-nested-{source_type}
      result_from:
        - nested
      steps:
        - id: nested
          skill: ../{child_name}
          inputs:
            thread_title: $input.thread_title
"#
        ),
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "catalog")]
fn write_graph_tool_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-tool");
    write_graph_tool_skill_at(&skill_dir)
}

#[cfg(feature = "catalog")]
fn write_graph_tool_skill_under_skills(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("skills/graph-tool");
    write_graph_tool_skill_at(&skill_dir)
}

#[cfg(feature = "catalog")]
fn write_graph_importing_graph_with_bundled_tool(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let parent_dir = root.join("skills/parent-board");
    let child_dir = root.join("skills/child-data");
    fs::create_dir_all(&parent_dir)?;
    fs::create_dir_all(&child_dir)?;
    fs::write(
        parent_dir.join("SKILL.md"),
        "---\nname: parent-board\n---\n# Parent Board\n",
    )?;
    fs::write(
        parent_dir.join("X.yaml"),
        r#"
skill: parent-board
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
    graph:
      name: parent-board
      result_from:
        - nested
      steps:
        - id: nested
          skill: ../child-data
          inputs:
            message: $input.thread_title
"#,
    )?;

    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child-data\n---\n# Child Data\n",
    )?;
    fs::write(
        child_dir.join("X.yaml"),
        r#"
skill: child-data
runners:
  graph:
    default: true
    type: graph
    inputs:
      message:
        type: string
        required: true
    graph:
      name: child-data
      result_from:
        - echo
      steps:
        - id: echo
          tool: test.echo
          scopes: [test.echo]
          inputs:
            message: $input.message
"#,
    )?;
    write_echo_tool_at(
        &child_dir.join("tools/test/echo"),
        "Nested graph tool root bug",
    )?;
    Ok(parent_dir)
}

#[cfg(feature = "catalog")]
fn write_graph_tool_skill_at(skill_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-tool\n---\n# Graph Tool\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-tool
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
    graph:
      name: graph-tool
      result_from:
        - echo
      steps:
        - id: echo
          tool: test.echo
          scopes: [test.echo]
          inputs:
            message: $input.thread_title
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

#[cfg(feature = "catalog")]
fn write_graph_agent_artifact_context_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-agent-artifact-context");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-agent-artifact-context\n---\n# Graph Agent Artifact Context\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-agent-artifact-context
runners:
  graph:
    default: true
    type: graph
    graph:
      name: graph-agent-artifact-context
      result_from:
        - echo
      steps:
        - id: author
          run:
            type: agent-task
            agent: builder
            task: graph-author
            outputs:
              fix_bundle: object
          artifacts:
            named_emits:
              fix_bundle: fix_bundle
        - id: echo
          tool: test.echo
          scopes: [test.echo]
          context:
            message: author.fix_bundle.data.message
"#,
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "catalog")]
fn write_graph_agent_then_input_tool_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-agent-then-input-tool");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-agent-then-input-tool\n---\n# Graph Agent Then Input Tool\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-agent-then-input-tool
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
    graph:
      name: graph-agent-then-input-tool
      result_from:
        - echo
      steps:
        - id: author
          run:
            type: agent-task
            agent: builder
            task: graph-author
            outputs:
              result: object
        - id: echo
          tool: test.echo
          scopes: [test.echo]
          inputs:
            message: $input.thread_title
"#,
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "catalog")]
fn write_graph_optional_json_tool_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-optional-json-tool");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-optional-json-tool\n---\n# Graph Optional JSON Tool\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-optional-json-tool
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
      harness:
        type: json
        required: false
    graph:
      name: graph-optional-json-tool
      result_from:
        - echo
      steps:
        - id: echo
          tool: test.optional-json
          scopes: [test.optional-json]
          inputs:
            message: $input.thread_title
            harness: $input.harness
"#,
    )?;
    Ok(skill_dir)
}

fn write_graph_required_input_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-required-input");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-required-input\n---\n# Graph Required Input\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-required-input
runners:
  graph:
    default: true
    type: graph
    inputs:
      lead:
        type: json
        required: true
        description: Lead packet to route.
    graph:
      name: graph-required-input
      result_from:
        - approve
      steps:
        - id: approve
          run:
            type: approval
          inputs:
            gate_id: graph-required-input.approve
            reason: approve the graph
"#,
    )?;
    Ok(skill_dir)
}

fn write_graph_approval_resume_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("approval-resume");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: approval-resume\n---\n# Approval Resume\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: approval-resume
runners:
  graph:
    default: true
    type: graph
    graph:
      name: approval-resume
      result_from:
        - approve
      steps:
        - id: approve
          run:
            type: approval
          inputs:
            gate_id: approval-resume.approve
            gate_type: test.claim-bound
            reason: approve the test graph
          artifacts:
            wrap_as: approval_decision
            packet: runx.approval.decision.v1
"#,
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "catalog")]
fn write_echo_tool(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    write_echo_tool_at(&root.join("tools/test/echo"), "Graph tool bug")
}

#[cfg(feature = "catalog")]
fn write_echo_tool_at(tool_dir: &Path, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(tool_dir)?;
    fs::write(
        tool_dir.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.echo",
  "source": {
    "type": "cli-tool",
    "command": "sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "message": { "type": "string", "required": true }
  },
  "artifacts": {
    "named_emits": {
      "echo": "test.echo.v1"
    }
  },
  "scopes": ["test.echo"]
}
"#,
    )?;
    fs::write(
        tool_dir.join("run.sh"),
        format!(
            r#"raw="$(cat)"
case "$raw" in
  *"Graph tool bug"*) printf '%s\n' '{{"echo":{{"message":"{}"}}}}' ;;
  *) printf '%s\n' '{{"echo":{{"message":"unexpected"}}}}' ;;
esac
"#,
            message
        ),
    )?;
    Ok(())
}

#[cfg(feature = "catalog")]
fn write_optional_json_tool(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tool_dir = root.join("tools/test/optional-json");
    fs::create_dir_all(&tool_dir)?;
    fs::write(
        tool_dir.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.optional-json",
  "source": {
    "type": "cli-tool",
    "command": "sh",
    "args": ["./run.sh"],
    "input_mode": "stdin"
  },
  "inputs": {
    "message": { "type": "string", "required": true },
    "harness": { "type": "json", "required": false }
  },
  "artifacts": {
    "named_emits": {
      "echo": "test.optional-json.v1"
    }
  },
  "scopes": ["test.optional-json"]
}
"#,
    )?;
    fs::write(
        tool_dir.join("run.sh"),
        r#"raw="$(cat)"
case "$raw" in
  *'$input.harness'*)
    printf '%s\n' '{"error":"unresolved harness reference reached tool input"}'
    exit 2
    ;;
  *'"harness"'*)
    printf '%s\n' '{"error":"optional harness should be omitted when absent"}'
    exit 3
    ;;
  *"Graph optional JSON bug"*)
    printf '%s\n' '{"echo":{"message":"Graph optional JSON bug"}}'
    ;;
  *)
    printf '%s\n' '{"echo":{"message":"unexpected"}}'
    ;;
esac
"#,
    )?;
    Ok(())
}

#[cfg(feature = "cli-tool")]
fn write_graph_nested_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-echo");
    crate::support::write_test_skill_package(
        &child_dir,
        r#"---
name: child-echo
---
# Child Echo
"#,
        r#"skill: child-echo
runners:
  child-echo:
    default: true
    type: cli-tool
    inputs:
      message:
        type: string
        required: true
    command: node
    args:
      - run.mjs
    input_mode: stdin
    outputs:
      nested: object
    artifacts:
      named_emits:
        nested: nested
"#,
    )?;
    fs::write(
        child_dir.join("run.mjs"),
        r#"import fs from "node:fs";
const raw = fs.readFileSync(0, "utf8");
const input = raw.trim() ? JSON.parse(raw) : {};
console.log(JSON.stringify({ nested: { message: input.message } }));
"#,
    )?;

    let skill_dir = root.join("graph-nested-cli");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-nested-cli\n---\n# Graph Nested CLI\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-nested-cli
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
    graph:
      name: graph-nested-cli
      result_from:
        - nested
      steps:
        - id: nested
          skill: ../child-echo
          inputs:
            message: $input.thread_title
"#,
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "cli-tool")]
fn write_graph_nested_registry_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    write_graph_nested_registry_skill_with_ref(root, "registry:acme/registry-child@1.0.0")
}

#[cfg(feature = "cli-tool")]
fn write_graph_nested_registry_skill_with_ref(
    root: &Path,
    skill_ref: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-nested-registry");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-nested-registry\n---\n# Graph Nested Registry\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        format!(
            r#"
skill: graph-nested-registry
runners:
  graph:
    default: true
    type: graph
    graph:
      name: graph-nested-registry
      result_from:
        - nested
      steps:
        - id: nested
          skill: {skill_ref}
"#
        ),
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "cli-tool")]
fn write_graph_stage_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-stage-cli");
    let stage_dir = skill_dir.join("graph/child-echo");
    crate::support::write_test_skill_package(
        &stage_dir,
        r#"---
name: child-echo
---
# Child Echo
"#,
        r#"skill: child-echo
runners:
  child-echo:
    default: true
    type: cli-tool
    inputs:
      message:
        type: string
        required: true
    command: node
    args:
      - run.mjs
    input_mode: stdin
    outputs:
      nested: object
"#,
    )?;
    fs::write(
        stage_dir.join("run.mjs"),
        r#"import fs from "node:fs";
const raw = fs.readFileSync(0, "utf8");
const input = raw.trim() ? JSON.parse(raw) : {};
console.log(JSON.stringify({ nested: { message: input.message } }));
"#,
    )?;

    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-stage-cli\n---\n# Graph Stage CLI\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-stage-cli
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
    graph:
      name: graph-stage-cli
      result_from:
        - nested
      steps:
        - id: nested
          skill: graph/child-echo
          inputs:
            message: $input.thread_title
"#,
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "cli-tool")]
fn write_graph_nested_cli_counter_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-counter");
    crate::support::write_test_skill_package(
        &child_dir,
        r#"---
name: child-counter
---
# Child Counter
"#,
        r#"skill: child-counter
runners:
  child-counter:
    default: true
    type: cli-tool
    inputs:
      count_file:
        type: string
        required: true
    command: node
    args:
      - run.mjs
    input_mode: stdin
    outputs:
      counted: object
"#,
    )?;
    fs::write(
        child_dir.join("run.mjs"),
        r#"import fs from "node:fs";
const raw = fs.readFileSync(0, "utf8");
const input = raw.trim() ? JSON.parse(raw) : {};
const path = input.count_file;
let count = 0;
try {
  count = Number(fs.readFileSync(path, "utf8")) || 0;
} catch {}
count += 1;
fs.writeFileSync(path, String(count));
console.log(JSON.stringify({ counted: { count } }));
"#,
    )?;

    let skill_dir = root.join("graph-nested-cli-counter");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-nested-cli-counter\n---\n# Graph Nested CLI Counter\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-nested-cli-counter
runners:
  graph:
    default: true
    type: graph
    inputs:
      count_file:
        type: string
        required: true
    graph:
      name: graph-nested-cli-counter
      result_from:
        - counted
      steps:
        - id: counted
          skill: ../child-counter
          inputs:
            count_file: $input.count_file
"#,
    )?;
    Ok(skill_dir)
}

#[cfg(feature = "cli-tool")]
fn write_graph_nested_x_yaml_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-x-cli");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child-x-cli\n---\n# Child X CLI\n",
    )?;
    fs::write(
        child_dir.join("X.yaml"),
        r#"
skill: child-x-cli
runners:
  child-cli:
    default: true
    type: cli-tool
    inputs:
      message:
        type: string
        required: true
    command: node
    args:
      - run.mjs
    input_mode: stdin
    outputs:
      nested: object
"#,
    )?;
    fs::write(
        child_dir.join("run.mjs"),
        r#"import fs from "node:fs";
const raw = fs.readFileSync(0, "utf8");
const input = raw.trim() ? JSON.parse(raw) : {};
console.log(JSON.stringify({ nested: { message: input.message } }));
"#,
    )?;

    let skill_dir = root.join("graph-nested-x-yaml-cli");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-nested-x-yaml-cli\n---\n# Graph Nested X YAML CLI\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-nested-x-yaml-cli
runners:
  graph:
    default: true
    type: graph
    inputs:
      thread_title:
        type: string
        required: true
    graph:
      name: graph-nested-x-yaml-cli
      result_from:
        - nested
      steps:
        - id: nested
          skill: ../child-x-cli
          inputs:
            message: $input.thread_title
"#,
    )?;
    Ok(skill_dir)
}

fn object<'a>(
    value: &'a JsonValue,
    label: &str,
) -> Result<&'a runx_contracts::JsonObject, Box<dyn std::error::Error>> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(format!("{label} was not an object").into()),
    }
}

fn object_mut<'a>(
    value: &'a mut JsonValue,
    label: &str,
) -> Result<&'a mut runx_contracts::JsonObject, Box<dyn std::error::Error>> {
    match value {
        JsonValue::Object(object) => Ok(object),
        _ => Err(format!("{label} was not an object").into()),
    }
}

fn object_field<'a>(
    object: &'a runx_contracts::JsonObject,
    field: &str,
) -> Option<&'a runx_contracts::JsonObject> {
    match object.get(field) {
        Some(JsonValue::Object(value)) => Some(value),
        _ => None,
    }
}

fn array_field<'a>(
    object: &'a runx_contracts::JsonObject,
    field: &str,
) -> Option<&'a Vec<JsonValue>> {
    match object.get(field) {
        Some(JsonValue::Array(value)) => Some(value),
        _ => None,
    }
}

/// Minimal operator environment for tests that spawn real subprocesses: the
/// runtime passes through only declared and baseline variables, so the tests
/// forward the host PATH explicitly instead of relying on ambient fallback.
#[cfg(feature = "cli-tool")]
fn path_env() -> BTreeMap<String, String> {
    std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default()
}

fn string_field<'a>(object: &'a runx_contracts::JsonObject, field: &str) -> Option<&'a str> {
    match object.get(field) {
        Some(JsonValue::String(value)) => Some(value),
        _ => None,
    }
}

fn write_graph_when_branch_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-when-branch");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-when-branch\n---\n# Graph When Branch\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: graph-when-branch
runners:
  graph:
    default: true
    type: graph
    graph:
      name: graph-when-branch
      result_from:
        - branch_go
        - branch_stop
      steps:
        - id: decide
          run:
            type: agent-task
            agent: builder
            task: when-decide
            outputs:
              verdict: string
        - id: branch_go
          when:
            field: decide.verdict
            equals: go
          run:
            type: agent-task
            agent: builder
            task: when-go
            outputs:
              result: object
        - id: branch_stop
          when:
            field: decide.verdict
            equals: stop
          run:
            type: agent-task
            agent: builder
            task: when-stop
            outputs:
              result: object
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

#[test]
fn native_graph_when_skips_unselected_branch() -> Result<(), Box<dyn std::error::Error>> {
    graph_when_selection_journey(false)
}

#[test]
fn native_graph_when_skips_unselected_fanout_branch() -> Result<(), Box<dyn std::error::Error>> {
    graph_when_selection_journey(true)
}

fn graph_when_selection_journey(fanout: bool) -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_when_branch_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    if fanout {
        let profile_path = skill_dir.join("X.yaml");
        let profile = fs::read_to_string(&profile_path)?
            .replace(
                "      name: graph-when-branch\n",
                "      name: graph-when-branch\n      fanout:\n        groups:\n          workers:\n            strategy: all\n            on_branch_failure: halt\n",
            )
            .replace(
                "        - id: branch_go\n",
                "        - id: branch_go\n          mode: fanout\n          fanout_group: workers\n",
            )
            .replace(
                "        - id: branch_stop\n",
                "        - id: branch_stop\n          mode: fanout\n          fanout_group: workers\n",
            );
        assert_eq!(profile.matches("fanout_group: workers").count(), 2);
        assert!(profile.contains("strategy: all"));
        fs::write(profile_path, profile)?;
    }

    let initial = run_skill(SkillRunRequest {
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers_path: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&initial.output, "when graph result")?;
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;

    // answers for decide and the selected branch only; branch_stop gets no
    // answer, so the run can seal only if `when` skipped it.
    let answers_path = temp.path().join("when-answers.json");
    fs::write(
        &answers_path,
        serde_json::json!({
            "answers": {
                "agent_task.when-decide.output": {
                    "verdict": "go",
                    "closure": {
                        "disposition": "closed"
                    }
                },
                "agent_task.when-go.output": {
                    "result": { "ok": true },
                    "closure": {
                        "disposition": "closed"
                    }
                }
            }
        })
        .to_string(),
    )?;

    let sealed = run_skill(SkillRunRequest {
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers_path: Some(answers_path),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
        local_credential: None,
    })?;
    let output = object(&sealed.output, "sealed when graph result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    Ok(())
}
