use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::KeyPair;
use runx_contracts::JsonValue;
use runx_receipts::ReceiptTreeConfig;
use runx_runtime::registry::TrustTier;
use runx_runtime::registry::{FileRegistryStore, IngestSkillOptions, ingest_skill_markdown};
use runx_runtime::{
    LocalOrchestrator, LocalReceiptStore, RUNX_RECEIPT_DIR_ENV, RunResult, SkillRunRequest,
};
use runx_runtime::{
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_KID_ENV,
};
use tempfile::tempdir;

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";
const TEST_MANIFEST_KEY_ID: &str = "runx-runtime-registry-test-key";
const TEST_MANIFEST_SIGNER_ID: &str = "runx-runtime-registry-test-signer";
const TEST_MANIFEST_SEED: [u8; 32] = [9; 32];

fn registry_child_profile_document() -> String {
    r#"
skill: registry-child
runners:
  child-cli:
    default: true
    steps:
      - id: child-cli
        tool:
          {
            command: sh,
            args:
              [
                -c,
                "cat >/dev/null

                  printf '%s\\n' '{\"nested\":{\"message\":\"registry child\"}}'\n"
             ],
            input_mode: stdin
          }
        outputs:
          nested: object
"#
    .to_owned()
}

fn trusted_manifest_env() -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    trusted_manifest_env_for_publisher("acme", None)
}

pub(crate) fn trusted_manifest_env_for_publisher(
    publisher: &str,
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
            runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_PUBLISHER_ENV.to_owned(),
            publisher.to_owned(),
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

pub(crate) fn sign_registry_version(
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

fn registry_version_path(
    registry_dir: &Path,
    skill_id: &str,
    version: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let (publisher, name) = skill_id
        .split_once('/')
        .ok_or("registry test skill id must be publisher/name")?;
    Ok(registry_dir
        .join(publisher)
        .join(name)
        .join(format!("{version}.json")))
}

/// The envelope the registry serves for a stored record's exact files.
fn signed_manifest(
    version_record: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let skill_id = version_record["skill_id"]
        .as_str()
        .ok_or("missing skill_id")?;
    let version = version_record["version"]
        .as_str()
        .ok_or("missing version")?;
    let markdown = version_record["markdown"]
        .as_str()
        .ok_or("missing markdown")?;
    let package_files: Vec<runx_runtime::registry::RegistryPackageFile> =
        serde_json::from_value(version_record["package_files"].clone()).unwrap_or_default();
    let content_digest = runx_runtime::registry::skill_content_digest(
        markdown,
        version_record["profile_document"].as_str(),
        &package_files,
    )?;
    let mut manifest = runx_contracts::RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        subject: runx_contracts::RegistrySubject {
            kind: runx_contracts::RegistrySubjectKind::Skill,
            name: skill_id.to_owned(),
        },
        version: version.to_owned(),
        content_digest,
        signer: runx_contracts::RegistryManifestSigner {
            id: TEST_MANIFEST_SIGNER_ID.to_owned(),
            key_id: TEST_MANIFEST_KEY_ID.to_owned(),
        },
        signature: runx_contracts::RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = test_manifest_key_pair()?.sign(&manifest.signing_payload()?);
    manifest.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(serde_json::to_value(manifest)?)
}

fn test_manifest_key_pair() -> Result<ring::signature::Ed25519KeyPair, std::io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&TEST_MANIFEST_SEED).map_err(|error| {
        std::io::Error::other(format!("static registry manifest seed rejected: {error:?}"))
    })
}

#[test]
fn runtime_options_local_development_uses_live_timestamp() -> Result<(), Box<dyn std::error::Error>>
{
    let (_root, run) = crate::support::scratch_run("run_live_timestamp")?;
    let options = crate::support::fixture_runtime_options(std::env::vars().collect(), run)?;

    assert_eq!(options.clock, runx_runtime::RunClock::System);
    let now = options.clock.now();
    assert_ne!(now, FIXTURE_CREATED_AT);
    assert!(now.ends_with('Z'));
    assert!(now.contains('T'));
    Ok(())
}

#[test]
fn native_skill_run_pauses_with_agent_act_request() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let expected_instructions = fs::read_to_string(skill_dir.join("SKILL.md"))?;
    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: None,
        run_id: None,
        answers: None,
        inputs: [(
            "thread_title".to_owned(),
            JsonValue::String("Docs bug".to_owned()),
        )]
        .into_iter()
        .collect(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "schema"), Some("runx.skill_run.v1"));
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run id")?;
    assert!(run_id.starts_with("run_code-intake_"), "{run_id}");
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(string_field(request, "kind"), Some("agent_act"));
    assert_eq!(string_field(request, "id"), Some("intake"));
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    assert_eq!(string_field(invocation, "task"), Some("code-intake"));
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: None,
        answers: None,
        inputs: [(
            "thread_title".to_owned(),
            JsonValue::String("Different docs bug".to_owned()),
        )]
        .into_iter()
        .collect(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
    let answers = serde_json::json!({
        "answers": {
            "intake": {
                "intake_report": {
                    "summary": "Docs bug is bounded."
                },
                "closure": {
                    "outcome": "declined"
                }
            }
        }
    });

    let result = complete_agent_fixture(
        with_test_signing_env(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipt_dir.clone()),
            run_id: Some("code-intake-run".to_owned()),
            answers: None,
            inputs: BTreeMap::new(),
            env: BTreeMap::new(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        }),
        answers,
    )?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(string_field(output, "run_id"), Some("code-intake-run"));
    assert_eq!(string_field(output, "outcome"), Some("declined"));
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
    assert_eq!(serde_json::to_value(receipt.seal.outcome)?, "declined");
    let step_receipt = crate::support::read_test_first_step_receipt(&receipt_dir, &receipt)?;
    assert_eq!(serde_json::to_value(step_receipt.seal.outcome)?, "declined");
    assert!(step_receipt.acts.is_empty());
    assert_eq!(
        serde_json::to_value(&step_receipt.seal.criteria[0].status)?,
        "failed"
    );

    let result = object_field(output, "result").ok_or("missing result")?;
    assert!(
        object_field(result, "intake_report").is_some(),
        "{output:?}"
    );

    Ok(())
}

#[test]
fn native_skill_run_treats_structured_stdout_as_claim_not_receipt_proof()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers = serde_json::json!({
        "answers": {
            "intake": {
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
                    "outcome": "completed"
                }
            }
        }
    });

    let result = complete_agent_fixture(
        with_test_signing_env(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipt_dir.clone()),
            run_id: Some("malicious-stdout-run".to_owned()),
            answers: None,
            inputs: BTreeMap::new(),
            env: BTreeMap::new(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        }),
        answers,
    )?;

    let output = object(&result.output, "skill run result")?;
    assert!(object_field(output, "result").is_some());
    assert!(object_field(output, "execution").is_none());
    assert!(object_field(output, "receipt").is_none());
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let receipt = crate::support::read_test_signed_receipt(&receipt_dir, receipt_id)?;
    let step_receipt = crate::support::read_test_first_step_receipt(&receipt_dir, &receipt)?;
    let refs = step_receipt
        .seal
        .criteria
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
fn native_skill_run_preserves_deferred_outcome() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers = serde_json::json!({
        "answers": {
            "intake": {
                "intake_report": {
                    "summary": "Docs bug needs more context."
                },
                "closure": {
                    "outcome": "deferred"
                }
            }
        }
    });

    let result = complete_agent_fixture(
        with_test_signing_env(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipt_dir.clone()),
            run_id: Some("code-intake-deferred".to_owned()),
            answers: None,
            inputs: BTreeMap::new(),
            env: BTreeMap::new(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        }),
        answers,
    )?;

    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(string_field(output, "outcome"), Some("deferred"));
    assert!(object_field(output, "error").is_none());
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let receipt = crate::support::read_test_signed_receipt(&receipt_dir, receipt_id)?;
    assert_eq!(serde_json::to_value(receipt.seal.outcome)?, "deferred");

    Ok(())
}

#[test]
fn native_skill_run_uses_runtime_receipt_path_resolution() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let env_receipt_dir = temp.path().join("env-receipts");
    let answers = serde_json::json!({
        "answers": {
            "intake": {
                "intake_report": {
                    "summary": "Docs bug is bounded."
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });

    let result = complete_agent_fixture(
        with_test_signing_env(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: None,
            run_id: Some("env-receipt-run".to_owned()),
            answers: None,
            inputs: BTreeMap::new(),
            env: [(
                RUNX_RECEIPT_DIR_ENV.to_owned(),
                env_receipt_dir.to_string_lossy().into_owned(),
            )]
            .into_iter()
            .collect(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        }),
        answers,
    )?;

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
fn explicit_receipt_dir_is_available_to_process_tool_subprocess()
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
        skill_dir.join("run.yaml"),
        r#"
skill: receipt-env-skill
runners:
  inspect:
    default: true
    steps:
      - id: inspect
        tool: { command: sh, args: [-c, "printf '{\"receipt_dir\":\"%s\",\"verify_kid\":\"%s\",\"verify_key\":\"%s\"}\\n' \"$RUNX_RECEIPT_DIR\" \"$RUNX_RECEIPT_VERIFY_KID\" \"$RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64\""] }
        outputs:
          receipt_dir: string
          verify_kid: string
          verify_key: string
"#,
    )?;

    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
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
        skill_dir.join("run.yaml"),
        r#"
skill: javascript-module-skill
runners:
  main:
    default: true
    inputs:
      value:
        type: string
        required: true
    result_from:
      - transform
    steps:
      - id: transform
        inputs:
          value: $input.value
        tool: { module: domain.mjs, export: transform }
        outputs:
          transformed: object
        artifacts:
          named_emits:
            transformed: transformed
          packets:
            transformed: acme/javascript-module-skill.transformed.v1
"#,
    )?;
    fs::create_dir_all(skill_dir.join("packets"))?;
    fs::write(
        skill_dir.join("packets/transformed.v1.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/javascript-module-skill/transformed/v1.json",
  "x-runx-packet-id": "acme/javascript-module-skill.transformed.v1",
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::from([("value".to_owned(), JsonValue::String("runx".to_owned()))]),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        skill_dir.join("run.yaml"),
        r#"
skill: invalid-javascript-packet
runners:
  main:
    default: true
    steps:
      - id: main
        tool: { module: domain.mjs }
        outputs:
          result: object
        artifacts:
          named_emits:
            result: result
          packets:
            result: acme/invalid-javascript-packet.result.v1
"#,
    )?;
    fs::write(
        skill_dir.join("domain.mjs"),
        "export default () => ({ result: { value: 42 } });\n",
    )?;
    fs::write(
        skill_dir.join("packets/result.v1.schema.json"),
        r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://schemas.runx.ai/packages/acme/invalid-javascript-packet/result/v1.json",
  "x-runx-packet-id": "acme/invalid-javascript-packet.result.v1",
  "type": "object",
  "required": ["value"],
  "properties": { "value": { "type": "string" } },
  "additionalProperties": false
}"#,
    )?;
    let env = path_env();

    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    // The step's packet violation fails the step; the graph seals that
    // failure rather than a result.
    let output = object(&result.output, "skill run result")?;
    assert_eq!(string_field(output, "outcome"), Some("failed"));
    let failure = object_field(output, "result").ok_or("missing failure result")?;
    assert!(
        string_field(failure, "message")
            .is_some_and(|message| message.contains("output violates schema")),
        "{output:?}"
    );
    Ok(())
}

#[test]
fn native_skill_run_uses_production_receipt_signing_env() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let answers = serde_json::json!({
        "answers": {
            "intake": {
                "intake_report": {
                    "summary": "Docs bug is bounded."
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let env = crate::support::test_signing_env();

    let result = complete_agent_fixture(
        with_test_signing_env(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipt_dir.clone()),
            run_id: Some("production-signed-run".to_owned()),
            answers: None,
            inputs: BTreeMap::new(),
            env: env.clone(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        }),
        answers,
    )?;

    let output = object(&result.output, "skill run result")?;
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let signature_config = crate::support::test_signature_config()?;
    let receipt = runx_runtime::LocalReceiptStore::new(&receipt_dir)
        .read_exact(receipt_id, signature_config.signature_policy())?;
    assert_eq!(receipt.issuer.kid, "runx-runtime-prod-fixture-key");
    assert!(receipt.signature.value.starts_with("base64:"));

    Ok(())
}

#[test]
fn native_skill_run_signs_with_the_selected_home_key() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_agent_task_skill(temp.path())?;
    let answers = serde_json::json!({
        "answers": {
            "intake": {
                "intake_report": {
                    "summary": "Docs bug is bounded."
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let receipt_dir = temp.path().join("receipts");
    let env = BTreeMap::from([(
        runx_runtime::RUNX_HOME_ENV.to_owned(),
        temp.path().join("home").display().to_string(),
    )]);
    let result = complete_agent_fixture(
        SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(receipt_dir.clone()),
            run_id: Some("home-signed-run".to_owned()),
            answers: None,
            inputs: BTreeMap::new(),
            env: env.clone(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        },
        answers,
    )?;
    assert_eq!(result.status, runx_runtime::RunStatus::Sealed);
    assert_eq!(result.receipt_ids.len(), 1);

    let output = object(&result.output, "skill run result")?;
    let receipt_id = string_field(output, "receipt_id").ok_or("missing receipt_id")?;
    let paths = runx_runtime::RunxPaths::resolve(&env, temp.path());
    let home_key =
        runx_runtime::receipt_trust(&env, &paths, runx_runtime::ReceiptTrust::HomeAndConfigured)?
            .pop()
            .ok_or("the run created no home key")?
            .key;
    let verifier = runx_runtime::Ed25519ReceiptVerifier::new([home_key.clone()]);
    let receipt = runx_runtime::LocalReceiptStore::new(&receipt_dir).read_exact(
        receipt_id,
        runx_runtime::RuntimeReceiptSignaturePolicy::verifying(&verifier),
    )?;
    assert_eq!(
        receipt.issuer.issuer_type,
        runx_contracts::ReceiptIssuerType::Local
    );
    assert_eq!(receipt.issuer.kid.as_str(), home_key.kid());
    assert_eq!(
        receipt.issuer.public_key_digest,
        home_key.public_key_digest()
    );
    assert!(receipt.signature.value.starts_with("base64:"));

    // Another home's key does not verify it.
    let other = tempdir()?;
    let other_env = BTreeMap::from([(
        runx_runtime::RUNX_HOME_ENV.to_owned(),
        other.path().display().to_string(),
    )]);
    let other_paths = runx_runtime::RunxPaths::resolve(&other_env, other.path());
    runx_runtime::RuntimeReceiptSignatureConfig::resolve(&other_env, &other_paths)?;
    let other_verifier = runx_runtime::receipt_verifier(
        &other_env,
        &other_paths,
        runx_runtime::ReceiptTrust::HomeAndConfigured,
    )?;
    assert!(
        runx_runtime::LocalReceiptStore::new(&receipt_dir)
            .read_exact(
                receipt_id,
                runx_runtime::RuntimeReceiptSignaturePolicy::verifying(&other_verifier),
            )
            .is_err()
    );
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&initial.output, "graph skill run result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(string_field(request, "id"), Some("decide"));
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

    // The paused run's record binds its package, closure and runner.
    let paused = runx_runtime::run_record::paused_run(&BTreeMap::new(), temp.path(), run_id)?
        .ok_or("the run is not paused in its record")?;
    assert!(paused.package_digest.starts_with("sha256:"));
    assert!(paused.execution_closure_digest.starts_with("sha256:"));
    assert_eq!(
        paused
            .waiting
            .iter()
            .map(|request| request.id.as_str())
            .collect::<Vec<_>>(),
        ["decide"]
    );

    // An answer the run refuses is never recorded: the run still waits.
    let malformed_answers = serde_json::json!({"answers": {"decide": {"unexpected": true}}});
    let malformed_answers =
        crate::support::bind_resume(&BTreeMap::new(), temp.path(), run_id, malformed_answers)?;
    let refused = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers: Some(malformed_answers),
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    });
    assert!(
        refused.is_err(),
        "an answer outside the output contract must be refused"
    );
    assert_eq!(
        runx_runtime::run_record::paused_run(&BTreeMap::new(), temp.path(), run_id)?.map(
            |paused| paused
                .waiting
                .into_iter()
                .map(|request| request.id)
                .collect()
        ),
        Some(vec!["decide".to_owned()])
    );

    // A changed package cannot continue the run it did not start.
    let manual = skill_dir.join("SKILL.md");
    let original_manual = fs::read_to_string(&manual)?;
    fs::write(
        &manual,
        format!("{original_manual}\nAn edit after the run paused.\n"),
    )?;
    let bad_answers = runx_contracts::ResumeDocument::default();
    let binding_mismatch = match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers: Some(bad_answers),
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    }) {
        Ok(_) => return Err("a changed package must not continue the run".into()),
        Err(error) => error,
    };
    assert!(
        binding_mismatch
            .to_string()
            .contains("cannot replace its original package digest"),
        "{binding_mismatch}"
    );
    fs::write(&manual, original_manual)?;

    let answers = serde_json::json!({
        "answers": {
            "decide": {
                "result": {
                    "summary": "Graph fix authored."
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let answers = crate::support::bind_resume(&BTreeMap::new(), temp.path(), run_id, answers)?;
    let resumed = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers: Some(answers),
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
    let sealed = runx_runtime::run_record::sealed_run(&BTreeMap::new(), temp.path(), run_id)?
        .ok_or("the resumed run has no seal in its record")?;
    assert_eq!(
        sealed.receipt.seal.outcome,
        runx_contracts::RunOutcome::Completed,
        "the run record must agree with the sealed graph receipt"
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&initial.output, "gated graph result")?;
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;

    let answers = serde_json::json!({
        "answers": {
            "decide": {
                "approved": true,
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });

    let answers = crate::support::bind_resume(&BTreeMap::new(), temp.path(), run_id, answers)?;
    let resumed = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&resumed.output, "resumed gated graph result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    assert_eq!(string_field(request, "id"), Some("gated"));

    Ok(())
}

/// A run started with a non-default value for a defaulted input suspends and
/// resumes with no inputs supplied: the checkpoint's inputs are restored before
/// defaults could fill the omitted one.
#[test]
fn native_graph_resume_keeps_a_non_default_input() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skills/continuation");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: continuation\ndescription: Resume with a non-default input.\n---\n\nPause for approval.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"skill: continuation
version: "0.1.0"
runners:
  default:
    default: true
    inputs:
      repo_root:
        type: string
        required: false
        default: "."
    result_from:
      - approve
    steps:
      - id: approve
        ask: { gate: continuation, reason: Approve the continuation. }
        inputs:
          repo_root: $input.repo_root
"#,
    )?;
    let receipt_dir = temp.path().join("receipts");
    let evidence_root = temp.path().join("evidence").to_string_lossy().into_owned();
    let request = |answers: Option<runx_contracts::ResumeDocument>,
                   inputs: BTreeMap<String, JsonValue>| SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some("run_continuation".to_owned()),
        answers,
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    };

    let paused = run_skill(request(
        None,
        BTreeMap::from([("repo_root".to_owned(), JsonValue::String(evidence_root))]),
    ))?;
    assert_eq!(paused.status, runx_runtime::RunStatus::NeedsAgent);
    let answers = crate::support::bind_resume(
        &BTreeMap::new(),
        temp.path(),
        "run_continuation",
        serde_json::json!({ "approvals": { "continuation": true } }),
    )?;

    let resumed = run_skill(request(Some(answers), BTreeMap::new()))?;
    assert_eq!(resumed.status, runx_runtime::RunStatus::Sealed);
    Ok(())
}

#[test]
fn native_graph_guard_rejects_skill_claim_as_fact() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_gated_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");

    let initial = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&initial.output, "gated graph result")?;
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;

    let answers = serde_json::json!({
        "answers": {
            "decide": {
                "approved": true,
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });

    let answers = crate::support::bind_resume(&BTreeMap::new(), temp.path(), run_id, answers)?;
    let blocked = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.to_owned()),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&blocked.output, "blocked graph result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(string_field(output, "outcome"), Some("blocked"));
    assert_eq!(string_field(output, "seal_reason"), Some("graph_blocked"));
    // Why the guard refused rides the run's output, outside the signed receipt.
    let refusal = object_field(output, "refusal").ok_or("missing refusal")?;
    assert!(
        string_field(refusal, "message")
            .is_some_and(|message| message
                .contains("guard '$steps.decide.skill_claim.approved' is unresolved")),
        "unexpected refusal: {refusal:?}"
    );

    Ok(())
}

#[test]
fn native_graph_skill_run_pauses_and_resumes_nested_agent_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_agent_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Nested agent bug".to_owned()),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let initial = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: inputs.clone(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&initial.output, "nested agent graph result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    // The act waits in the child run of step `nested`, and is answered there.
    assert_eq!(string_field(request, "id"), Some("nested#author"));
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    assert_eq!(string_field(invocation, "role"), Some("builder"));
    assert_eq!(string_field(invocation, "task"), Some("child-agent"));

    let answers = serde_json::json!({
        "answers": {
            "nested#author": {
                "result": {
                    "summary": "Nested agent fix authored."
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let answers = crate::support::bind_resume(&BTreeMap::new(), temp.path(), run_id, answers)?;
    let resumed = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id.to_owned()),
        answers: Some(answers),
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
            publisher_slug: Some("runx".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(
                r#"skill: taste-profile
runners:
  main:
    default: true
    steps:
      - id: main
        agent: { role: critic, task: apply taste judgement }
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
    assert!(string_field(data, "manual_digest").is_some_and(|hash| hash.starts_with("sha256:")));
    assert!(string_field(data, "profile_digest").is_some_and(|hash| hash.starts_with("sha256:")));
    assert!(
        string_field(data, "content").is_some_and(|manual| manual.contains("# Taste Profile")
            && manual.contains("Prefer clear product taste"))
    );
    let meta = object_field(context_entry, "meta").ok_or("missing context meta")?;
    assert!(string_field(meta, "digest").is_some_and(|digest| digest.starts_with("sha256:")));

    Ok(())
}

#[test]
fn graph_agent_task_rejects_parent_path_context_skill() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let context_dir = temp.path().join("taste-profile");
    fs::create_dir_all(&context_dir)?;
    fs::write(
        context_dir.join("SKILL.md"),
        "---\nname: taste-profile\ndescription: taste-profile fixture.\n---\n# Taste Profile\n",
    )?;
    let skill_dir = write_graph_agent_task_with_context_skill(temp.path(), "../taste-profile")?;

    let error = match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
description: context-stage fixture.
---
# Context Stage
"#,
    )?;
    fs::write(
        stage_dir.join("run.yaml"),
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
    steps:
      - id: main
        agent: { role: builder, task: internal implementation detail }
"#,
    )?;

    let error = match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
            publisher_slug: Some("sourcey".to_owned()),
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
    steps:
      - id: main
        agent: { role: builder, task: internal helper }
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs,
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "graph tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(string_field(echo, "message"), Some("Graph tool bug"));

    Ok(())
}

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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "configured graph no-consent result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    Ok(())
}

#[test]
fn native_graph_skill_run_resolves_agent_task_named_emit_context()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_artifact_context_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers = serde_json::json!({
        "answers": {
            "author": {
                "fix_bundle": {
                    "message": "Graph tool bug"
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let pending = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let pending_output = object(&pending.output, "pending graph agent artifact result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending graph agent artifact result missing run_id")?
        .to_owned();

    let answers = crate::support::bind_resume(&env, temp.path(), &run_id, answers)?;
    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "graph agent artifact result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(string_field(echo, "message"), Some("Graph tool bug"));

    Ok(())
}

#[test]
fn native_graph_skill_resume_preserves_initial_inputs_for_later_tool_steps()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_then_input_tool_skill(temp.path())?;
    write_echo_tool_at(
        &temp.path().join("tools/test/echo"),
        "/evidence/non-default",
    )?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers = serde_json::json!({
        "answers": {
            "author": {
                "result": {
                    "accepted": true
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let initial_inputs = [
        (
            "thread_title".to_owned(),
            JsonValue::String("Graph tool bug".to_owned()),
        ),
        (
            "repo_root".to_owned(),
            JsonValue::String("/evidence/non-default".to_owned()),
        ),
    ]
    .into_iter()
    .collect::<BTreeMap<_, _>>();

    let pending = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: initial_inputs,
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let pending_output = object(&pending.output, "pending graph input resume result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending graph input resume result missing run_id")?
        .to_owned();

    let answers = crate::support::bind_resume(&env.clone(), temp.path(), &run_id, answers)?;
    let changed = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.clone()),
        answers: Some(answers.clone()),
        inputs: BTreeMap::from([(
            "repo_root".to_owned(),
            JsonValue::String("/evidence/other".to_owned()),
        )]),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    });
    assert!(changed.err().is_some_and(|error| {
        error
            .to_string()
            .contains("cannot replace its original inputs")
    }));

    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "graph input resume result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo = object_field(echo, "data").ok_or("missing echo data envelope")?;
    assert_eq!(string_field(echo, "message"), Some("/evidence/non-default"));

    Ok(())
}

// Transport-envelope probing is intentionally forbidden: an agent claim must
// return the declared contract directly, so an `output`-wrapped answer fails
// with the typed missing-output error instead of being silently unwrapped.
#[test]
fn native_graph_skill_run_refuses_agent_output_envelope_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_agent_artifact_context_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers = serde_json::json!({
        "answers": {
            "author": {
                "output": {
                    "fix_bundle": {
                        "message": "Graph tool bug"
                    }
                },
                "closure": {
                    "outcome": "completed"
                }
            }
        }
    });
    let env = [(
        "RUNX_TOOL_ROOTS".to_owned(),
        tool_root.to_string_lossy().into_owned(),
    )]
    .into_iter()
    .collect::<BTreeMap<_, _>>();
    let pending = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let pending_output = object(
        &pending.output,
        "pending graph agent artifact envelope result",
    )?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending graph agent artifact envelope result missing run_id")?
        .to_owned();

    // An answer that breaks the act's output contract is refused and the
    // turn stays open: nothing is sealed, and the run can still be answered.
    let answers = crate::support::bind_resume(&env.clone(), temp.path(), &run_id, answers)?;
    let Err(refused) = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.clone()),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env: env.clone(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    }) else {
        return Err("an envelope-shaped agent answer must be refused".into());
    };
    let message = refused.to_string();
    assert!(
        message.contains("runner output contract violation at $.output"),
        "unexpected envelope refusal: {message}"
    );
    let reopened = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&reopened.output, "reopened agent turn")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));

    Ok(())
}

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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs,
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
    assert_eq!(serde_json::to_value(receipt.seal.outcome)?, "blocked");
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

    let pending = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let pending_output = object(&pending.output, "pending approval result")?;
    assert_eq!(
        string_field(pending_output, "status"),
        Some("needs_approval")
    );
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending approval result missing run_id")?
        .to_owned();

    let answers = crate::support::bind_resume(&BTreeMap::new(), temp.path(), &run_id, answers)?;
    let resumed = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: Some(run_id),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
fn native_graph_skill_resume_approves_only_what_the_run_waits_on()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_approval_resume_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts-digest-bound-approval");
    let run_id = "run_assistant_work_digest_bound_approval".to_owned();
    let start = SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.clone()),
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    };
    let pending = run_skill(start.clone())?;
    assert_eq!(pending.status, runx_runtime::RunStatus::NeedsAgent);
    let request =
        runx_runtime::run_record::paused_requests(&BTreeMap::new(), temp.path(), &run_id)?
            .pop()
            .ok_or("the paused run's approval request is missing")?;
    // A person approves only what the run waits on: an approval of any other
    // request, such as one the resume would ask next, is refused.
    let mut resume = start;
    resume.answers = Some(serde_json::from_value(serde_json::json!({
        "approvals": {
            request.id.clone(): { "request_digest": request.digest.clone(), "approved": true },
            "a-later-step": { "request_digest": "sha256:not-shown", "approved": true }
        }
    }))?);
    let rejected = run_skill(resume.clone())
        .err()
        .ok_or("an approval of a request not shown was accepted")?;
    assert!(
        rejected
            .to_string()
            .contains("a-later-step names no request"),
        "{rejected}"
    );
    resume.answers = Some(serde_json::from_value(serde_json::json!({
        "approvals": {
            request.id.clone(): { "request_digest": request.digest.clone(), "approved": true }
        }
    }))?);
    let resumed = run_skill(resume)?;
    assert_eq!(resumed.status, runx_runtime::RunStatus::Sealed);
    assert!(resumed.succeeded());
    Ok(())
}

#[test]
fn native_graph_skill_resume_rejects_agent_answer_for_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_approval_resume_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts-agent-approval");
    let answers = serde_json::json!({
        "answers": { "approval-resume.approve": true }
    });

    let pending = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let pending_output = object(&pending.output, "pending approval result")?;
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending approval result missing run_id")?
        .to_owned();

    let answers = crate::support::bind_resume(&BTreeMap::new(), temp.path(), &run_id, answers)?;
    let rejected = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some(run_id.clone()),
        answers: Some(answers),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })
    .err()
    .ok_or("agent-authored approval must be rejected without sealing")?;
    assert!(
        rejected.to_string().contains("a person's to answer"),
        "{rejected}"
    );
    assert!(
        runx_runtime::run_record::paused_run(&BTreeMap::new(), temp.path(), &run_id)?.is_some()
    );

    Ok(())
}

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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "graph tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    let echo_data = object_field(echo, "data").ok_or("missing echo data")?;
    assert_eq!(string_field(echo_data, "message"), Some("root tools"));

    Ok(())
}

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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs,
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        .children
        .first()
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
    let nested_child_refs = &nested_graph_receipt.lineage.children;
    assert_eq!(nested_child_refs.len(), 1);
    let nested_child_id = nested_child_refs[0]
        .uri
        .as_str()
        .strip_prefix("runx:receipt:")
        .ok_or("nested child receipt reference is malformed")?;
    let nested_child_receipt =
        crate::support::read_test_signed_receipt(&receipt_dir, nested_child_id)?;
    let signature_config = crate::support::test_signature_config()?;
    let validation = runx_runtime::receipts::tree::validate_runtime_receipt_tree(
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

#[test]
fn native_graph_skill_run_executes_nested_process_tool_skill()
-> Result<(), Box<dyn std::error::Error>> {
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
    for diagnostic in [
        "\"execution\"",
        "\"payload\"",
        "\"receipt\"",
        "\"skill_claim\"",
        "\"structured_output\"",
    ] {
        assert!(
            !public_json.contains(diagnostic),
            "public result leaked diagnostic field {diagnostic}"
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
    let child_refs = &root_receipt.lineage.children;
    assert_eq!(child_refs.len(), 1);
    assert_eq!(
        child_refs[0].uri.as_str(),
        format!("runx:receipt:{nested_receipt_id}")
    );
    assert_eq!(
        child_refs[0].locator.as_deref(),
        Some(child_receipt.digest.as_str())
    );
    Ok(())
}

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
            publisher_slug: Some("acme".to_owned()),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "nested registry skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("registry child"));

    Ok(())
}

#[test]
fn skill_inspection_reports_registry_refs_the_registry_directory_does_not_hold()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry_dir = temp.path().join("registry");
    fs::create_dir_all(&registry_dir)?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let mut env = trusted_manifest_env()?;
    env.insert(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    );
    let closure_of = |inspection: &runx_contracts::SkillInspection| -> Result<JsonValue, Box<dyn std::error::Error>> {
        Ok(JsonValue::Object(
            inspection
                .runner_inspections
                .first()
                .ok_or("no runner inspection")?
                .execution_closure
                .clone(),
        ))
    };

    let unheld = closure_of(&runx_runtime::inspect_skill_package(
        &skill_dir,
        None,
        Some(&env),
    )?)?;
    let unheld = object(&unheld, "closure")?;
    assert_eq!(unheld.get("fully_bound"), Some(&JsonValue::Bool(false)));
    assert_eq!(
        unheld.get("unresolved_skill_edges"),
        Some(&JsonValue::Array(vec![JsonValue::String(
            "registry:acme/registry-child@1.0.0#default".to_owned()
        )]))
    );

    let store = FileRegistryStore::new(&registry_dir);
    ingest_skill_markdown(
        &store,
        "---\nname: registry-child\ndescription: Registry-backed nested child.\n---\n# Registry Child\n",
        IngestSkillOptions {
            publisher_slug: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(registry_child_profile_document()),
            ..IngestSkillOptions::default()
        },
    )?;
    sign_registry_version(&registry_dir, "acme/registry-child", "1.0.0")?;
    let held = closure_of(&runx_runtime::inspect_skill_package(
        &skill_dir,
        None,
        Some(&env),
    )?)?;
    let held = object(&held, "closure")?;
    assert_eq!(held.get("fully_bound"), Some(&JsonValue::Bool(true)));
    assert_eq!(
        held.get("unresolved_skill_edges"),
        Some(&JsonValue::Array(Vec::new()))
    );

    Ok(())
}

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
            publisher_slug: Some("runx".to_owned()),
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
    let mut env = trusted_manifest_env_for_publisher("runx", Some("official_runx"))?;
    env.insert(
        "RUNX_REGISTRY_DIR".to_owned(),
        registry_dir.to_string_lossy().into_owned(),
    );

    let error = match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
            publisher_slug: Some("acme".to_owned()),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
            publisher_slug: Some("acme".to_owned()),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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

#[test]
fn native_graph_skill_run_rejects_a_nested_registry_skill_the_project_does_not_hold()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");

    let error = match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    }) {
        Ok(_) => return Err("nested registry skill unexpectedly succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("is not installed in this project"),
        "unexpected error: {error}"
    );

    Ok(())
}

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

    let request = SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir.clone()),
        run_id: Some("run_pinned_counter".to_owned()),
        answers: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    };
    let result = run_skill(request.clone())?;

    let output = object(&result.output, "counter graph skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(fs::read_to_string(&count_file)?, "1");

    let recovered = run_skill(request.clone())?;
    assert_eq!(recovered.output, result.output);
    assert_eq!(recovered.receipt_ids, result.receipt_ids);
    assert_eq!(fs::read_to_string(&count_file)?, "1");

    let mut drift = request.clone();
    drift.inputs.insert(
        "count_file".to_owned(),
        JsonValue::String(
            temp.path()
                .join("different.txt")
                .to_string_lossy()
                .into_owned(),
        ),
    );
    let error = run_skill(drift)
        .err()
        .ok_or("pinned graph identity must reject input drift")?;
    assert!(
        error
            .to_string()
            .contains("cannot replace its original inputs")
    );
    assert_eq!(fs::read_to_string(&count_file)?, "1");

    // A rewritten record fails its chain: the run holds instead of rerunning.
    let store = temp.path().join(".runx/kernel-v2.sqlite");
    let database = rusqlite::Connection::open(&store)?;
    let original_event: Vec<u8> = database.query_row(
        "SELECT data FROM kernel_events WHERE kind = 'run' AND key = 'run_pinned_counter' AND version = 2",
        [],
        |row| row.get(0),
    )?;
    database.execute(
        "UPDATE kernel_events SET data = ?1 WHERE kind = 'run' AND key = 'run_pinned_counter' AND version = 2",
        [b"{}".to_vec()],
    )?;
    let error = run_skill(request.clone())
        .err()
        .ok_or("a rewritten run record must hold")?;
    assert!(error.to_string().contains("integrity chain"), "{error}");
    assert_eq!(fs::read_to_string(&count_file)?, "1");
    database.execute(
        "UPDATE kernel_events SET data = ?1 WHERE kind = 'run' AND key = 'run_pinned_counter' AND version = 2",
        [original_event],
    )?;
    drop(database);

    let receipt_id = result.receipt_ids.first().ok_or("missing graph receipt")?;
    let receipt_path = LocalReceiptStore::new(&receipt_dir).receipt_path(receipt_id)?;
    fs::write(receipt_path, b"{}")?;
    let error = run_skill(request)
        .err()
        .ok_or("tampered sealed receipt must hold")?;
    // The content-addressed store refuses the tampered bytes before recovery
    // trusts them, so the sealed step holds instead of rerunning.
    assert!(
        error
            .to_string()
            .contains("receipt already exists with different content"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(count_file)?, "1");

    Ok(())
}

#[test]
fn native_graph_skill_run_executes_graph_stage_process_tool_skill()
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(receipt_dir),
        run_id: None,
        answers: None,
        inputs,
        env: path_env(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = object(&result.output, "nested run.yaml graph skill result")?;
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
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: None,
        run_id: Some("code-intake-run".to_owned()),
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&named_start.output, "named start")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    assert_eq!(string_field(output, "run_id"), Some("code-intake-run"));

    let answers_only = match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: None,
        run_id: None,
        answers: Some(runx_contracts::ResumeDocument::default()),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    }) {
        Ok(_) => return Err("answers without run-id should fail".into()),
        Err(error) => error,
    };
    assert!(
        answers_only
            .to_string()
            .contains("continuation requires run_id")
    );

    let missing_checkpoint = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: None,
        run_id: Some("missing".to_owned()),
        answers: Some(runx_contracts::ResumeDocument::default()),
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })
    .err()
    .ok_or("an unadmitted run must not accept answers")?;
    assert!(
        missing_checkpoint
            .to_string()
            .contains("run missing has no record to continue"),
        "{missing_checkpoint}"
    );
    Ok(())
}

// Exercise durable admission before applying fixture answers; a named answer
// without an admitted checkpoint is never a continuation.
/// Start `request`'s run, then answer what it pauses on with `answers`,
/// bound to the requests it waits on as a host binds them.
fn complete_agent_fixture(
    mut request: SkillRunRequest,
    answers: serde_json::Value,
) -> Result<RunResult, Box<dyn std::error::Error>> {
    let pending = LocalOrchestrator::default().run_skill(&request)?;
    assert!(!pending.pending_requests.is_empty());
    let pending = serde_json::to_value(&pending.output)?;
    assert_eq!(pending["status"], "needs_agent");
    let run_id = request.run_id.clone().ok_or("fixture runs pin their id")?;
    assert_eq!(pending["run_id"].as_str(), Some(run_id.as_str()));
    request.answers = Some(crate::support::bind_resume(
        &request.env,
        &request.cwd,
        &run_id,
        answers,
    )?);
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
    let skill_dir = root.join("code-intake");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: code-intake\ndescription: code-intake fixture.\n---\n# Issue Intake\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: code-intake
runners:
  intake:
    default: true
    inputs:
      thread_title:
        type: string
        required: false
    steps:
      - id: intake
        agent: { role: builder, task: code-intake }
        inputs:
          thread_title: $input.thread_title
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
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_graph_agent_task_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-issue-to-pr");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-issue-to-pr\ndescription: graph-issue-to-pr fixture.\n---\n# Graph Issue To PR\n\nUse the full issue context.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-issue-to-pr
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: false
    result_from:
      - decide
    steps:
      - id: decide
        agent: { role: builder, task: graph-decide }
        inputs:
          thread_title: $input.thread_title
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
        "---\nname: graph-agent-context-skill\ndescription: graph-agent-context-skill fixture.\n---\n# Graph Agent Context Skill\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
skill: graph-agent-context-skill
runners:
  graph:
    default: true
    result_from:
      - apply_taste
    steps:
      - id: apply_taste
        agent: {{ role: builder, task: apply taste guidance }}
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
        "---\nname: graph-gated-agent-task\ndescription: graph-gated-agent-task fixture.\n---\n# Graph Gated Agent Step\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
skill: graph-gated-agent-task
runners:
  graph:
    default: true
    result_from:
      - gated
    steps:
      - id: decide
        agent: {{ role: builder, task: gated-decide }}
        outputs:
          approved: boolean
      - id: gated
        agent: {{ role: builder, task: gated-followup }}
        outputs:
          result: object
    policy:
      guards:
        - step: gated
          value: $steps.{gate_field}
          equals: true
            "#
        ),
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_graph_nested_agent_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_name = "child-agent";
    let child_dir = root.join(child_name);
    let runner = r#"    inputs:
      thread_title:
        type: string
        required: false
    steps:
      - id: author
        agent: { role: builder, task: child-agent }
        inputs:
          thread_title: $input.thread_title
        outputs:
          result: object
"#;
    crate::support::write_test_skill_package(
        &child_dir,
        format!(
            r#"---
name: {child_name}
description: {child_name} fixture.
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

    let skill_dir = root.join("graph-nested-agent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-nested-agent\ndescription: graph-nested-agent fixture.\n---\n# Graph Nested agent\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
skill: graph-nested-agent
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: false
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

fn write_graph_tool_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-tool");
    write_graph_tool_skill_at(&skill_dir)
}

fn write_graph_tool_skill_under_skills(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("skills/graph-tool");
    write_graph_tool_skill_at(&skill_dir)
}

fn write_graph_importing_graph_with_bundled_tool(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let parent_dir = root.join("skills/parent-board");
    let child_dir = root.join("skills/child-data");
    fs::create_dir_all(&parent_dir)?;
    fs::create_dir_all(&child_dir)?;
    fs::write(
        parent_dir.join("SKILL.md"),
        "---\nname: parent-board\ndescription: parent-board fixture.\n---\n# Parent Board\n",
    )?;
    fs::write(
        parent_dir.join("run.yaml"),
        r#"
skill: parent-board
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
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
        "---\nname: child-data\ndescription: child-data fixture.\n---\n# Child Data\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        r#"
skill: child-data
runners:
  graph:
    default: true
    inputs:
      message:
        type: string
        required: true
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

fn write_graph_tool_skill_at(skill_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-tool\ndescription: graph-tool fixture.\n---\n# Graph Tool\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-tool
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
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

fn write_graph_agent_artifact_context_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-agent-artifact-context");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-agent-artifact-context\ndescription: graph-agent-artifact-context fixture.\n---\n# Graph Agent Artifact Context\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-agent-artifact-context
runners:
  graph:
    default: true
    result_from:
      - echo
    steps:
      - id: author
        agent: { role: builder, task: graph-author }
        outputs:
          fix_bundle: object
        artifacts:
          named_emits:
            fix_bundle: fix_bundle
      - id: echo
        tool: test.echo
        scopes: [test.echo]
        inputs:
          message: $steps.author.fix_bundle.message
"#,
    )?;
    Ok(skill_dir)
}

fn write_graph_agent_then_input_tool_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-agent-then-input-tool");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-agent-then-input-tool\ndescription: graph-agent-then-input-tool fixture.\n---\n# Graph Agent Then Input Tool\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-agent-then-input-tool
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
      repo_root:
        type: string
        default: .
    result_from:
      - echo
    steps:
      - id: author
        agent: { role: builder, task: graph-author }
        outputs:
          result: object
      - id: echo
        tool: test.echo
        scopes: [test.echo]
        inputs:
          message: $input.repo_root
"#,
    )?;
    Ok(skill_dir)
}

fn write_graph_optional_json_tool_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-optional-json-tool");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-optional-json-tool\ndescription: graph-optional-json-tool fixture.\n---\n# Graph Optional JSON Tool\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-optional-json-tool
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
      harness:
        type: json
        required: false
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
        "---\nname: graph-required-input\ndescription: graph-required-input fixture.\n---\n# Graph Required Input\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-required-input
runners:
  graph:
    default: true
    inputs:
      lead:
        type: json
        required: true
        description: Lead packet to route.
    result_from:
      - approve
    steps:
      - id: approve
        ask: { gate: graph-required-input.approve, reason: approve the graph }
"#,
    )?;
    Ok(skill_dir)
}

fn write_graph_approval_resume_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("approval-resume");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: approval-resume\ndescription: approval-resume fixture.\n---\n# Approval Resume\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: approval-resume
runners:
  graph:
    default: true
    result_from:
      - approve
    steps:
      - id: approve
        ask: { gate: approval-resume.approve, reason: approve the test graph, type: test.claim-bound }
"#,
    )?;
    Ok(skill_dir)
}

fn write_echo_tool(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    write_echo_tool_at(&root.join("tools/test/echo"), "Graph tool bug")
}

fn write_echo_tool_at(tool_dir: &Path, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(tool_dir)?;
    fs::write(
        tool_dir.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.echo",
  "source": {
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
  *"Graph tool bug"*|*"{}"*) printf '%s\n' '{{"echo":{{"message":"{}"}}}}' ;;
  *) printf '%s\n' '{{"echo":{{"message":"unexpected"}}}}' ;;
esac
"#,
            message, message
        ),
    )?;
    Ok(())
}

fn write_optional_json_tool(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let tool_dir = root.join("tools/test/optional-json");
    fs::create_dir_all(&tool_dir)?;
    fs::write(
        tool_dir.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.optional-json",
  "source": {
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

fn write_graph_nested_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-echo");
    crate::support::write_test_skill_package(
        &child_dir,
        r#"---
name: child-echo
description: child-echo fixture.
---
# Child Echo
"#,
        r#"skill: child-echo
runners:
  child-echo:
    default: true
    inputs:
      message:
        type: string
        required: true
    steps:
      - id: child-echo
        tool: { command: node, args: [run.mjs], input_mode: stdin }
        inputs:
          message: $input.message
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
        "---\nname: graph-nested-cli\ndescription: graph-nested-cli fixture.\n---\n# Graph Nested CLI\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-nested-cli
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
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

fn write_graph_nested_registry_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    write_graph_nested_registry_skill_with_ref(root, "registry:acme/registry-child@1.0.0")
}

fn write_graph_nested_registry_skill_with_ref(
    root: &Path,
    skill_ref: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-nested-registry");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: graph-nested-registry\ndescription: graph-nested-registry fixture.\n---\n# Graph Nested Registry\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
skill: graph-nested-registry
runners:
  graph:
    default: true
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

fn write_graph_stage_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("graph-stage-cli");
    let stage_dir = skill_dir.join("graph/child-echo");
    crate::support::write_test_skill_package(
        &stage_dir,
        r#"---
name: child-echo
description: child-echo fixture.
---
# Child Echo
"#,
        r#"skill: child-echo
runners:
  child-echo:
    default: true
    inputs:
      message:
        type: string
        required: true
    steps:
      - id: child-echo
        tool: { command: node, args: [run.mjs], input_mode: stdin }
        inputs:
          message: $input.message
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
        "---\nname: graph-stage-cli\ndescription: graph-stage-cli fixture.\n---\n# Graph Stage CLI\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-stage-cli
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
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

fn write_graph_nested_cli_counter_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-counter");
    crate::support::write_test_skill_package(
        &child_dir,
        r#"---
name: child-counter
description: child-counter fixture.
---
# Child Counter
"#,
        r#"skill: child-counter
runners:
  child-counter:
    default: true
    inputs:
      count_file:
        type: string
        required: true
    steps:
      - id: child-counter
        tool: { command: node, args: [run.mjs], input_mode: stdin }
        inputs:
          count_file: $input.count_file
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
        "---\nname: graph-nested-cli-counter\ndescription: graph-nested-cli-counter fixture.\n---\n# Graph Nested CLI Counter\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-nested-cli-counter
runners:
  graph:
    default: true
    inputs:
      count_file:
        type: string
        required: true
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

fn write_graph_nested_x_yaml_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-x-cli");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child-x-cli\ndescription: child-x-cli fixture.\n---\n# Child X CLI\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        r#"
skill: child-x-cli
runners:
  child-cli:
    default: true
    inputs:
      message:
        type: string
        required: true
    steps:
      - id: child-cli
        tool: { command: node, args: [run.mjs], input_mode: stdin }
        inputs:
          message: $input.message
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
        "---\nname: graph-nested-x-yaml-cli\ndescription: graph-nested-x-yaml-cli fixture.\n---\n# Graph Nested X YAML CLI\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-nested-x-yaml-cli
runners:
  graph:
    default: true
    inputs:
      thread_title:
        type: string
        required: true
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
        "---\nname: graph-when-branch\ndescription: graph-when-branch fixture.\n---\n# Graph When Branch\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: graph-when-branch
runners:
  graph:
    default: true
    result_from:
      - branch_go
      - branch_stop
    steps:
      - id: decide
        agent: { role: builder, task: when-decide }
        outputs:
          verdict: string
      - id: branch_go
        when:
          value: $steps.decide.verdict
          equals: go
        agent: { role: builder, task: when-go }
        outputs:
          result: object
      - id: branch_stop
        when:
          value: $steps.decide.verdict
          equals: stop
        agent: { role: builder, task: when-stop }
        outputs:
          result: object
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

#[test]
fn native_graph_when_skips_unselected_branch() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_graph_when_branch_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");

    let initial = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir.clone(),
        receipt_dir: Some(receipt_dir.clone()),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&initial.output, "when graph result")?;
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;

    // Answer decide, then the selected branch it leads to; branch_stop is
    // never asked, so the run can seal only if `when` skipped it.
    let answer = |answers: serde_json::Value| -> Result<RunResult, Box<dyn std::error::Error>> {
        run_skill(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir.clone(),
            receipt_dir: Some(receipt_dir.clone()),
            run_id: Some(run_id.to_owned()),
            answers: Some(crate::support::bind_resume(
                &BTreeMap::new(),
                temp.path(),
                run_id,
                answers,
            )?),
            inputs: BTreeMap::new(),
            env: BTreeMap::new(),
            cwd: temp.path().to_path_buf(),
            managed_agent: Default::default(),
        })
    };
    let branched = answer(serde_json::json!({
        "answers": { "decide": { "verdict": "go", "closure": { "outcome": "completed" } } }
    }))?;
    let branched = object(&branched.output, "branched when graph result")?;
    assert_eq!(string_field(branched, "status"), Some("needs_agent"));
    let sealed = answer(serde_json::json!({
        "answers": { "branch_go": { "result": { "ok": true }, "closure": { "outcome": "completed" } } }
    }))?;
    let output = object(&sealed.output, "sealed when graph result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    Ok(())
}

/// A registry that serves its signed records, and the routes it was asked.
struct RecordRegistry {
    directory: PathBuf,
    asked: std::sync::Mutex<Vec<String>>,
}

impl runx_runtime::RuntimeHttpTransport for RecordRegistry {
    fn send(
        &self,
        request: runx_runtime::RuntimeHttpRequest,
    ) -> Result<runx_runtime::RuntimeHttpResponse, runx_runtime::RuntimeHttpError> {
        let route = request
            .url
            .strip_prefix("https://registry.test")
            .unwrap_or(&request.url)
            .to_owned();
        self.asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(route.clone());
        let record = route
            .strip_prefix("/v1/skills/")
            .and_then(|rest| rest.strip_suffix("/record"))
            .and_then(|rest| {
                let mut parts = rest.split('/');
                let (publisher, name, _, version) =
                    (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
                fs::read_to_string(
                    self.directory
                        .join(publisher)
                        .join(name)
                        .join(format!("{version}.json")),
                )
                .ok()
            });
        Ok(match record {
            Some(record) => runx_runtime::RuntimeHttpResponse::new(
                200,
                format!(r#"{{"status":"success","record":{record}}}"#),
            ),
            None => runx_runtime::RuntimeHttpResponse::new(404, "{}"),
        })
    }
}

#[test]
fn a_publish_binds_another_publishers_skill_from_the_registry_it_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    // The registry the package is published to holds acme's skill.
    let hosted = temp.path().join("hosted");
    ingest_skill_markdown(
        &FileRegistryStore::new(&hosted),
        "---\nname: registry-child\ndescription: Registry-backed nested child.\n---\n# Registry Child\n",
        IngestSkillOptions {
            publisher_slug: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some(FIXTURE_CREATED_AT.to_owned()),
            profile_document: Some(registry_child_profile_document()),
            ..IngestSkillOptions::default()
        },
    )?;
    sign_registry_version(&hosted, "acme/registry-child", "1.0.0")?;
    let skill_dir = write_graph_nested_registry_skill(temp.path())?;
    let env = trusted_manifest_env()?;
    let subject = skill_dir.to_string_lossy().into_owned();
    let request = runx_runtime::registry::RegistryPublishPackageRequest {
        subject: &subject,
        profile: None,
        env: &env,
        cwd: temp.path(),
    };
    assert_eq!(
        runx_runtime::registry::unheld_publish_registry_refs(request)?,
        ["registry:acme/registry-child@1.0.0"]
    );

    let registry = RecordRegistry {
        directory: hosted,
        asked: std::sync::Mutex::default(),
    };
    let client =
        runx_runtime::registry::RegistryClient::with_transport("https://registry.test", registry)?;
    let closure = runx_runtime::registry::materialize_publish_closure(
        &client,
        "https://registry.test",
        request,
    )?
    .ok_or("a publish without a registry directory binds its own")?;

    assert!(
        closure
            .directory()
            .join("acme/registry-child/1.0.0.json")
            .is_file()
    );
    let bound = runx_runtime::registry::RegistryPublishPackageRequest {
        env: &closure.env,
        ..request
    };
    assert!(runx_runtime::registry::unheld_publish_registry_refs(bound)?.is_empty());
    runx_runtime::registry::prepare_registry_publish_package(bound)?;
    Ok(())
}

/// Two drivers of one run: the step that reaches the world runs once. The
/// second driver loses the claim before it dispatches, or finds the claim and
/// waits on its retry.
#[cfg(unix)]
#[test]
fn a_second_driver_of_a_run_never_runs_its_step_again() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempdir()?;
    let skill = root.path().join("skill");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: race\ndescription: Runs one command.\n---\n\nRun one command.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "skill: race\nrunners:\n  default:\n    default: true\n    steps:\n      - id: write\n        tool: { command: sh, args: [./run.sh] }\n        outputs: { done: boolean }\n",
    )?;
    let marker = root.path().join("executed");
    fs::write(
        skill.join("run.sh"),
        format!(
            "echo executed >> '{}'\nsleep 2\nprintf '%s\\n' '{{\"done\":true}}'\n",
            marker.display()
        ),
    )?;
    let request = with_test_signing_env(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill,
        receipt_dir: Some(root.path().join("receipts")),
        run_id: Some("run_race".to_owned()),
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::from([("PATH".to_owned(), "/usr/bin:/bin".to_owned())]),
        cwd: root.path().to_path_buf(),
        managed_agent: Default::default(),
    });
    let first_request = request.clone();
    let first = std::thread::spawn(move || {
        LocalOrchestrator::default()
            .run_skill(&first_request)
            .map(|result| result.succeeded())
            .map_err(|error| error.to_string())
    });
    let started = std::time::Instant::now();
    while !marker.exists() {
        if started.elapsed() > std::time::Duration::from_secs(10) {
            return Err("the first driver never reached its command".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let second = LocalOrchestrator::default().run_skill(&request);
    let first = first.join().map_err(|_| "the first driver panicked")?;

    assert_eq!(first, Ok(true));
    assert!(
        second.as_ref().map_or(true, |result| !result.succeeded()),
        "the second driver must not seal the run again"
    );
    assert_eq!(fs::read_to_string(&marker)?.lines().count(), 1);
    Ok(())
}
