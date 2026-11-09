use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::JsonValue;
use runx_receipts::ReceiptTreeConfig;
use runx_runtime::{
    LocalOrchestrator, LocalReceiptStore, RUNX_RECEIPT_DIR_ENV, RunResult, SkillRunRequest,
};
use runx_runtime::{
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_KID_ENV,
};
use tempfile::tempdir;

use crate::registry_fixture::{LoopbackRegistry, Publication};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

fn registry_child_profile_document() -> String {
    r#"
minimum_runx: 0.12.0
methods:
  child-cli:
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
            inputs_via: stdin
          }
        outputs:
          nested: {type: object}
"#
    .to_owned()
}

/// A loopback registry publishing `acme/registry-child@1.0.0`.
fn registry_child() -> Result<LoopbackRegistry, Box<dyn std::error::Error>> {
    let registry = LoopbackRegistry::serve()?;
    let profile = registry_child_profile_document();
    registry.publish(Publication::new(
        "acme",
        "1.0.0",
        "---\nname: registry-child\ndescription: Registry-backed nested child.\n---\n# Registry Child\n",
        Some(&profile),
    ))?;
    Ok(registry)
}

/// The environment of a project at `root` that reaches `registry` and
/// trusts its key.
fn registry_env(
    registry: &LoopbackRegistry,
    root: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut env = registry.env()?;
    env.insert(
        runx_runtime::RUNX_CWD_ENV.to_owned(),
        root.to_string_lossy().into_owned(),
    );
    env.insert(
        "RUNX_HOME".to_owned(),
        root.join("home").to_string_lossy().into_owned(),
    );
    Ok(env)
}

/// Fetch every registry skill the package at `skill_dir` names from
/// `registry` into the project at `root`, as `runx skill` does before a run,
/// and return the environment a run of it takes.
fn fetch_registry_refs(
    registry: &LoopbackRegistry,
    root: &Path,
    skill_dir: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let env = registry_env(registry, root)?;
    runx_runtime::registry::resolve::resolve_skill_ref_details(
        skill_dir,
        root,
        runx_runtime::registry::resolve::SkillResolverOptions {
            env: &env,
            registry: Some(&registry.url),
            expected_digest: None,
            absent_pins: runx_runtime::registry::resolve::AbsentPins::Fail,
            holder: runx_runtime::registry::resolve::Holder::Project,
        },
    )?;
    Ok(env)
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
fn native_skill_run_pauses_with_turn_request() -> Result<(), Box<dyn std::error::Error>> {
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
    assert_eq!(string_field(request, "kind"), Some("turn"));
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
minimum_runx: 0.12.0
methods:
  inspect:
    steps:
      - id: inspect
        tool: {command: sh, args: [-c, "printf '{\"receipt_dir\":\"%s\",\"verify_kid\":\"%s\",\"verify_key\":\"%s\"}\\n' \"$RUNX_RECEIPT_DIR\" \"$RUNX_RECEIPT_VERIFY_KID\" \"$RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64\""]}
        outputs:
          receipt_dir: {type: string}
          verify_kid: {type: string}
          verify_key: {type: string}
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
fn native_workflow_runs_a_javascript_module_without_manifest_process_plumbing()
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
minimum_runx: 0.12.0
methods:
  main:
    inputs:
      value:
        type: string
    result_from:
      - transform
    steps:
      - id: transform
        inputs:
          value: $input.value
        tool: {module: domain.mjs, export: transform}
        outputs:
          transformed: {type: object, packet: acme/javascript-module-skill.transformed.v1}
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
        .ok_or("missing workflow steps")?;
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
fn native_javascript_method_rejects_a_packet_schema_violation()
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
minimum_runx: 0.12.0
methods:
  main:
    steps:
      - id: main
        tool: {module: domain.mjs}
        outputs:
          result: {type: object, packet: acme/invalid-javascript-packet.result.v1}
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

    // The step's packet violation fails the step; the workflow seals that
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
fn native_workflow_skill_run_pauses_and_resumes_agent_task()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_agent_task_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Workflow bug".to_owned()),
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

    let output = object(&initial.output, "workflow skill run result")?;
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
        Some(&JsonValue::String("Workflow bug".to_owned()))
    );

    // The paused run's record binds its package, closure and method.
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
                    "summary": "Workflow fix authored."
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

    let output = object(&resumed.output, "resumed workflow skill run result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let declared_result =
        object_field(public_result, "result").ok_or("missing declared result output")?;
    assert_eq!(
        string_field(declared_result, "summary"),
        Some("Workflow fix authored.")
    );
    let record = runx_runtime::run_record::read_run(&BTreeMap::new(), temp.path(), run_id, None)?;
    assert_eq!(record.run.state, runx_contracts::RunState::Sealed);
    assert_eq!(
        record.run.outcome,
        Some(runx_contracts::RunOutcome::Completed),
        "the run record must agree with the sealed workflow receipt"
    );

    Ok(())
}

#[test]
fn native_workflow_transition_gate_allows_declared_agent_output()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir =
        write_workflow_gated_agent_task_skill_with_field(temp.path(), "decide.approved")?;
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
    let output = object(&initial.output, "gated workflow result")?;
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

    let output = object(&resumed.output, "resumed gated workflow result")?;
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
fn native_workflow_resume_keeps_a_non_default_input() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = temp.path().join("skills/continuation");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: continuation\ndescription: Resume with a non-default input.\n---\n\nPause for approval.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"version: "0.1.0"
minimum_runx: 0.12.0
methods:
  default:
    inputs:
      repo_root:
        type: string
        default: "."
        optional: true
    result_from:
      - approve
    steps:
      - id: approve
        ask: {gate: continuation, reason: Approve the continuation.}
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
fn native_workflow_guard_rejects_skill_claim_as_fact() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_gated_agent_task_skill(temp.path())?;
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
    let output = object(&initial.output, "gated workflow result")?;
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
    let output = object(&blocked.output, "blocked workflow result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    assert_eq!(string_field(output, "outcome"), Some("blocked"));
    assert_eq!(
        string_field(output, "seal_reason"),
        Some("workflow_blocked")
    );
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
fn native_workflow_skill_run_pauses_and_resumes_nested_agent_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_nested_agent_skill(temp.path())?;
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

    let output = object(&initial.output, "nested agent workflow result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let run_id = string_field(output, "run_id").ok_or("missing run_id")?;
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    assert_eq!(requests.len(), 1);
    let request = object(&requests[0], "request")?;
    // The act waits in the child run of step `nested`, and is answered there.
    assert_eq!(string_field(request, "id"), Some("nested#author"));
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
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

    let output = object(&resumed.output, "resumed nested agent workflow result")?;
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
fn workflow_agent_task_injects_registry_skill_as_current_context()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = LoopbackRegistry::serve()?;
    registry.publish(Publication::new(
        "acme",
        "1.0.0",
        r#"---
name: taste-profile
description: Portable taste guidance for downstream agents.
---
# Taste Profile

## Agent task: apply taste judgement

Prefer clear product taste over ornamental flourish. Flag incoherent hierarchy,
weak contrast, and interaction states that feel bolted on.
"#,
        Some(
            r#"minimum_runx: 0.12.0
methods:
  main:
    steps:
      - id: main
        agent: apply taste judgement
"#,
        ),
    ))?;
    let skill_dir = write_workflow_agent_task_with_context_skill(
        temp.path(),
        "registry:acme/taste-profile@1.0.0",
    )?;
    let env = fetch_registry_refs(&registry, temp.path(), &skill_dir)?;

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

    let output = object(&result.output, "registry context workflow result")?;
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
    assert_eq!(string_field(data, "skill_id"), Some("acme/taste-profile"));
    assert_eq!(string_field(data, "version"), Some("1.0.0"));
    // Its identity is its signature's; its tier is a statement apart, read
    // only when a decision needs more than community (D30).
    assert!(!data.contains_key("trust_tier"));
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
    assert_eq!(
        string_field(context_entry, "schema"),
        Some("runx.context_entry.v1")
    );
    // A skill's instructions are a procedure, from the skill they name.
    let labels = object_field(context_entry, "labels").ok_or("missing context labels")?;
    assert_eq!(string_field(labels, "purpose"), Some("procedure"));
    assert_eq!(string_field(labels, "trust"), Some("skill_instructions"));
    let sources = array_field(context_entry, "sources").ok_or("missing context sources")?;
    assert_eq!(sources.len(), 1);
    let origin = object_field(context_entry, "origin").ok_or("missing context origin")?;
    assert!(string_field(origin, "digest").is_some_and(|digest| digest.starts_with("sha256:")));

    Ok(())
}

/// A package whose `recall` step returns `entries` from `recall.mjs`, and
/// whose `answer` turn is given them through `context.entries`.
fn write_context_entries_skill(
    root: &Path,
    recall: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("context-entries");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: context-entries\ndescription: An answer given what an earlier step recalled.\n---\n# Context entries\n\n## Agent task: answer\n\nAnswer from what was recalled.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  main:
    steps:
      - id: recall
        tool: {module: recall.mjs, export: recall}
        outputs:
          entries: {type: array}
      - id: answer
        agent: answer
        context:
          entries: [$steps.recall.entries]
        outputs:
          answer: {type: string}
"#,
    )?;
    fs::write(skill_dir.join("recall.mjs"), recall)?;
    Ok(skill_dir)
}

fn run_context_entries_skill(
    root: &Path,
    skill_dir: PathBuf,
) -> Result<RunResult, Box<dyn std::error::Error>> {
    run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(root.join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: path_env(),
        cwd: root.to_path_buf(),
        managed_agent: Default::default(),
    })
}

#[test]
fn an_agent_step_is_given_an_earlier_steps_context_entries_with_their_own_labels()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_context_entries_skill(
        temp.path(),
        r#"export const recall = () => ({ entries: [{
  schema: "runx.context_entry.v1",
  type: "memory.said",
  summary: "Kam's launch date",
  data: { text: "We launch on the third of November." },
  labels: { purpose: "confirmed_memory", trust: "person_confirmed", sharing: "configured_model" },
  sources: ["conversation:2026-10-20"],
  origin: {
    artifact_id: "memory:said:personal#launch@1",
    run_id: "rx_recall",
    step_id: null,
    producer: { skill: "runx/memory", method: "index" },
    created_at: "2026-10-20T00:00:00Z",
    digest: "sha256:launch",
    size_bytes: 36
  }
}] });
"#,
    )?;

    let result = run_context_entries_skill(temp.path(), skill_dir)?;

    let output = object(&result.output, "context entries result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    let request = object(&requests[0], "request")?;
    let invocation = object_field(request, "invocation").ok_or("missing invocation")?;
    let envelope = object_field(invocation, "envelope").ok_or("missing envelope")?;
    let current_context =
        array_field(envelope, "current_context").ok_or("missing current_context")?;
    assert_eq!(current_context.len(), 1, "{current_context:?}");
    let entry = object(&current_context[0], "recalled entry")?;
    assert_eq!(string_field(entry, "type"), Some("memory.said"));
    // The entry keeps the labels its producer gave it: the runtime neither
    // raises its trust nor widens where it may go.
    let labels = object_field(entry, "labels").ok_or("missing labels")?;
    assert_eq!(string_field(labels, "purpose"), Some("confirmed_memory"));
    assert_eq!(string_field(labels, "trust"), Some("person_confirmed"));
    assert_eq!(string_field(labels, "sharing"), Some("configured_model"));
    Ok(())
}

#[test]
fn a_turn_whose_host_may_run_its_model_elsewhere_is_given_no_local_only_entry()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let entry = |summary: &str, sharing: &str| {
        format!(
            r#"{{
  schema: "runx.context_entry.v1",
  type: "memory.said",
  summary: "{summary}",
  data: {{ text: "{summary}" }},
  labels: {{ purpose: "confirmed_memory", trust: "person_confirmed", sharing: "{sharing}" }},
  sources: ["conversation:2026-10-20"],
  origin: {{
    artifact_id: "memory:{sharing}",
    run_id: "rx_recall",
    step_id: null,
    producer: {{ skill: "runx/memory", method: "index" }},
    created_at: "2026-10-20T00:00:00Z",
    digest: "sha256:{sharing}",
    size_bytes: 12
  }}
}}"#
        )
    };
    let skill_dir = write_context_entries_skill(
        temp.path(),
        &format!(
            "export const recall = () => ({{ entries: [{}, {}] }});\n",
            entry("Shared with the configured model", "configured_model"),
            entry("Kept on this machine", "local_only"),
        ),
    )?;

    // No managed agent works this run, so its host may run its model
    // anywhere.
    let result = run_context_entries_skill(temp.path(), skill_dir)?;

    let output = object(&result.output, "context entries result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let rendered = serde_json::to_string(&result.output)?;
    assert!(
        rendered.contains("Shared with the configured model"),
        "{rendered}"
    );
    // Neither the entry nor any of its text reaches the host.
    assert!(!rendered.contains("Kept on this machine"), "{rendered}");
    assert!(!rendered.contains("memory:local_only"), "{rendered}");
    Ok(())
}

/// A skill whose first step makes an artifact labelled `sharing`, and whose
/// agent step is given it as an input.
fn write_artifact_input_skill(
    root: &Path,
    sharing: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("artifact-input");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: artifact-input\ndescription: An answer given an earlier step's artifact.\n---\n# Artifact input\n\n## Agent task: answer\n\nAnswer from the artifact.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  main:
    steps:
      - id: make
        tool: {module: make.mjs, export: make}
        outputs:
          artifact: {type: object}
      - id: answer
        agent: answer
        inputs:
          source: $steps.make.artifact
        outputs:
          answer: {type: string}
"#,
    )?;
    let digest = format!("sha256:{}", "c".repeat(64));
    fs::write(
        skill_dir.join("make.mjs"),
        format!(
            r#"export const make = () => ({{ artifact: {{
  schema: "runx.artifact.v1",
  ref: "runx:project-artifact:{digest}",
  digest: "{digest}",
  size_bytes: 24,
  media_type: "application/json",
  summary: "The {sharing} notes",
  labels: {{ sharing: "{sharing}" }}
}} }});
"#
        ),
    )?;
    Ok(skill_dir)
}

#[test]
fn a_turn_whose_host_may_run_its_model_elsewhere_is_refused_a_local_only_artifact()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_artifact_input_skill(temp.path(), "local_only")?;

    // No managed agent works this run, so its host may run its model
    // anywhere.
    let result = run_context_entries_skill(temp.path(), skill_dir)?;

    let output = object(&result.output, "artifact input result")?;
    assert_ne!(string_field(output, "status"), Some("needs_agent"));
    let rendered = serde_json::to_string(&result.output)?;
    assert!(
        rendered.contains("a local_only artifact is among its inputs"),
        "{rendered}"
    );
    // No turn is asked, so the host is given nothing of the artifact.
    assert!(
        array_field(output, "requests").is_none_or(|requests| requests.is_empty()),
        "{rendered}"
    );
    Ok(())
}

#[test]
fn a_turn_is_given_an_artifact_its_configured_model_may_see()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_artifact_input_skill(temp.path(), "configured_model")?;

    let result = run_context_entries_skill(temp.path(), skill_dir)?;

    let output = object(&result.output, "artifact input result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    let rendered = serde_json::to_string(&result.output)?;
    assert!(
        rendered.contains("The configured_model notes"),
        "{rendered}"
    );
    Ok(())
}

#[test]
fn an_agent_step_refuses_an_earlier_output_that_isnt_context_entries()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    // `scope` was a context entry's labels before 0.12; it isn't one now.
    let skill_dir = write_context_entries_skill(
        temp.path(),
        r#"export const recall = () => ({ entries: [{ type: "memory.said", scope: { purpose: "confirmed_memory" } }] });
"#,
    )?;

    let result = run_context_entries_skill(temp.path(), skill_dir)?;

    let output = object(&result.output, "context entries result")?;
    assert_ne!(string_field(output, "status"), Some("needs_agent"));
    let rendered = serde_json::to_string(&result.output)?;
    assert!(
        rendered.contains(
            "context.entries reads $steps.recall.entries, which isn't a list of context entries"
        ),
        "{rendered}"
    );
    Ok(())
}

/// A package whose `read` step returns `entry` from `read.mjs`, an output that
/// names the core packet `runx.context_entry.v1`.
fn write_context_entry_packet_skill(
    root: &Path,
    read: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("entry-packet");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: entry-packet\ndescription: One context entry, named by its packet.\n---\n# Entry packet\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: read\n        tool: {module: read.mjs, export: read}\n        outputs:\n          entry: {type: object, packet: runx.context_entry.v1}\n",
    )?;
    fs::write(skill_dir.join("read.mjs"), read)?;
    Ok(skill_dir)
}

#[test]
fn an_output_naming_the_context_entry_packet_is_checked_against_it()
-> Result<(), Box<dyn std::error::Error>> {
    let entry = |trust: &str| {
        format!(
            r#"export const read = () => ({{ entry: {{
  schema: "runx.context_entry.v1",
  type: "memory.said",
  summary: "2026-11-08 I write in Australian English.",
  data: {{ handle: "memory:said:personal#k@1" }},
  labels: {{ purpose: "confirmed_memory", trust: "{trust}", sharing: "configured_model" }},
  sources: ["conversation"],
  origin: {{
    artifact_id: "memory:said:personal#k@1",
    step_id: null,
    producer: {{ skill: "runx/memory", method: "read" }},
    created_at: "2026-11-08T00:00:00Z",
    digest: "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    size_bytes: 30
  }}
}} }});
"#
        )
    };
    let run = |root: &Path, skill_dir: PathBuf| {
        run_skill(SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill_dir,
            receipt_dir: Some(root.join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env: path_env(),
            cwd: root.to_path_buf(),
            managed_agent: Default::default(),
        })
    };

    // An entry made in no run is a context entry, and its packet holds.
    let temp = tempdir()?;
    let kept = run(
        temp.path(),
        write_context_entry_packet_skill(temp.path(), &entry("person_confirmed"))?,
    )?;
    let output = object(&kept.output, "entry result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"), "{output:?}");

    // A label the packet doesn't have fails the step.
    let temp = tempdir()?;
    let refused = run(
        temp.path(),
        write_context_entry_packet_skill(temp.path(), &entry("operator_confirmed"))?,
    )?;
    let output = object(&refused.output, "entry result")?;
    assert_eq!(
        string_field(output, "outcome"),
        Some("failed"),
        "{output:?}"
    );
    assert!(
        serde_json::to_string(&refused.output)?.contains("runx.context_entry.v1"),
        "{output:?}"
    );
    Ok(())
}

/// A package whose `change` step says what a gate decides, and whose `gate`
/// step shows that as its reason.
fn write_bound_reason_skill(
    root: &Path,
    gate: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("bound-reason");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: bound-reason\ndescription: A gate that shows the exact change it decides.\n---\n# Bound reason\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: change\n        tool: {{module: change.mjs, export: change}}\n        outputs:\n          reason: {{type: string}}\n{gate}"
        ),
    )?;
    fs::write(
        skill_dir.join("change.mjs"),
        "export const change = () => ({ reason: \"Keep these exact words as the person's: I write in Australian English.\" });\n",
    )?;
    Ok(skill_dir)
}

/// The reason of the one approval `skill_dir`'s run waits on.
fn waiting_reason(root: &Path, skill_dir: PathBuf) -> Result<String, Box<dyn std::error::Error>> {
    let pending = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(root.join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: path_env(),
        cwd: root.to_path_buf(),
        managed_agent: Default::default(),
    })?;
    let output = object(&pending.output, "pending result")?;
    assert_eq!(
        string_field(output, "status"),
        Some("needs_approval"),
        "{output:?}"
    );
    let requests = array_field(output, "requests").ok_or("missing requests")?;
    let [request] = requests.as_slice() else {
        return Err(format!("one request: {requests:?}").into());
    };
    let gate = object_field(object(request, "the request")?, "gate").ok_or("a request's gate")?;
    Ok(string_field(gate, "reason")
        .ok_or("a gate's reason")?
        .to_owned())
}

#[test]
fn an_asks_reason_bound_from_an_earlier_step_is_what_its_gate_shows()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_bound_reason_skill(
        temp.path(),
        "      - id: confirm\n        ask: {gate: words.keep, reason: $steps.change.reason, type: person_words}\n        inputs: {quote: I write in Australian English.}\n",
    )?;
    assert_eq!(
        waiting_reason(temp.path(), skill_dir)?,
        "Keep these exact words as the person's: I write in Australian English."
    );
    Ok(())
}

/// A package at `root/skill` whose one step echoes `$run.workspace_id`.
fn write_workspace_id_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workspace-echo\ndescription: Echo the run's workspace id.\n---\n# Workspace echo\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  main:\n    steps:\n      - id: echo\n        tool: {module: echo.mjs, export: echo}\n        inputs: {workspace: $run.workspace_id}\n        outputs:\n          workspace: {}\n",
    )?;
    fs::write(
        skill_dir.join("echo.mjs"),
        "export const echo = ({ workspace }) => ({ workspace });\n",
    )?;
    Ok(skill_dir)
}

/// Run the workspace echo from `cwd` under `env`, returning the run's output.
fn run_workspace_echo(
    root: &Path,
    cwd: &Path,
    env: BTreeMap<String, String>,
) -> Result<JsonValue, Box<dyn std::error::Error>> {
    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: write_workspace_id_skill(root)?,
        receipt_dir: Some(root.join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: cwd.to_path_buf(),
        managed_agent: Default::default(),
    })?;
    Ok(result.output)
}

fn echoed_workspace(output: &JsonValue) -> Option<&JsonValue> {
    output
        .as_object()
        .and_then(|output| output.get("result"))
        .and_then(JsonValue::as_object)
        .and_then(|result| result.get("workspace"))
}

#[test]
fn a_run_reads_the_id_its_workspace_was_minted_in_the_runx_home()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(workspace.join(".git"))?;
    let home = temp.path().join("home");
    let mut env = path_env();
    env.insert("RUNX_HOME".to_owned(), home.display().to_string());

    let first = run_workspace_echo(&workspace, &workspace, env.clone())?;
    let minted = echoed_workspace(&first)
        .and_then(JsonValue::as_str)
        .ok_or_else(|| format!("no workspace id: {first:?}"))?
        .to_owned();
    assert!(minted.starts_with("ws_"), "{minted}");
    // A later run there reads the same id, which only the home holds.
    let second = run_workspace_echo(&workspace, &workspace, env)?;
    assert_eq!(
        echoed_workspace(&second).and_then(JsonValue::as_str),
        Some(minted.as_str())
    );
    assert!(home.join("workspaces.json").is_file());
    assert!(!workspace.join(".runx/workspaces.json").exists());
    Ok(())
}

#[test]
fn a_run_where_there_is_no_workspace_reads_a_null_workspace_id()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let plain = temp.path().join("plain");
    fs::create_dir_all(&plain)?;
    let mut env = path_env();
    env.insert(
        "RUNX_HOME".to_owned(),
        temp.path().join("home").display().to_string(),
    );

    let output = run_workspace_echo(temp.path(), &plain, env)?;
    assert_eq!(
        echoed_workspace(&output),
        Some(&JsonValue::Null),
        "{output:?}"
    );
    Ok(())
}

#[test]
fn a_run_whose_runx_home_fell_back_refuses_to_read_a_workspace_id()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(workspace.join(".git"))?;

    // No RUNX_HOME and no HOME: the home is the workspace's own `.runx`,
    // which a repository could ship. So the run is made without the test
    // helper, which names a home.
    let mut env = path_env();
    crate::support::insert_test_signing_env(&mut env);
    let refused = match LocalOrchestrator::default().run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: write_workspace_id_skill(&workspace)?,
            receipt_dir: Some(temp.path().join("receipts")),
            run_id: None,
            answers: None,
            inputs: BTreeMap::new(),
            env,
            cwd: workspace.clone(),
            managed_agent: Default::default(),
        },
        None,
        None,
        None,
    ) {
        Ok(result) => serde_json::to_string(&result.output)?,
        Err(error) => error.to_string(),
    };
    assert!(
        refused.contains("reads $run.workspace_id, and the runx home fell back to the workspace"),
        "{refused}"
    );
    assert!(!workspace.join(".runx/workspaces.json").exists());
    Ok(())
}

#[test]
fn workflow_agent_task_rejects_parent_path_context_skill() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let context_dir = temp.path().join("taste-profile");
    fs::create_dir_all(&context_dir)?;
    fs::write(
        context_dir.join("SKILL.md"),
        "---\nname: taste-profile\ndescription: taste-profile fixture.\n---\n# Taste Profile\n",
    )?;
    let skill_dir = write_workflow_agent_task_with_context_skill(temp.path(), "../taste-profile")?;

    let error = context_skill_refusal(temp.path(), skill_dir)?;
    assert!(
        error.contains("context skill ref \"../taste-profile\" must not traverse the package"),
        "unexpected error: {error}"
    );

    Ok(())
}

/// A stage in a skill's `workflow/` folder runs only inside its workflow, so
/// no step loads one as context.
#[test]
fn workflow_agent_task_rejects_a_context_skill_in_its_workflow_stages()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_agent_task_with_context_skill(temp.path(), "workflow/plan")?;
    let stage = skill_dir.join("workflow/plan");
    fs::create_dir_all(&stage)?;
    fs::write(
        stage.join("SKILL.md"),
        "---\nname: plan\ndescription: plan stage fixture.\n---\n# Plan\n",
    )?;

    let error = context_skill_refusal(temp.path(), skill_dir)?;
    assert!(
        error.contains(
            "context skill ref \"workflow/plan\" must not target an internal workflow stage"
        ),
        "unexpected error: {error}"
    );

    Ok(())
}

/// The error a run of `skill_dir` fails with, refusing its context skill.
fn context_skill_refusal(
    root: &Path,
    skill_dir: PathBuf,
) -> Result<String, Box<dyn std::error::Error>> {
    match run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill_dir,
        receipt_dir: Some(root.join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env: BTreeMap::new(),
        cwd: root.to_path_buf(),
        managed_agent: Default::default(),
    }) {
        Ok(_) => Err("a refused context skill ran".into()),
        Err(error) => Ok(error.to_string()),
    }
}

#[test]
fn native_workflow_skill_run_executes_local_tool_step() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_tool_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Workflow tool bug".to_owned()),
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

    let output = object(&result.output, "workflow tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    assert_eq!(string_field(echo, "message"), Some("Workflow tool bug"));
    // The envelope's trace names the workflow the run ran under `workflow`,
    // and under no older key.
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    assert_eq!(string_field(trace, "workflow"), Some("workflow-tool"));
    assert!(!trace.contains_key("graph"), "{trace:?}");

    Ok(())
}

#[test]
fn configured_model_credentials_do_not_enable_workflow_agent_without_run_consent()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_agent_task_skill(temp.path())?;
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

    let output = object(&result.output, "configured workflow no-consent result")?;
    assert_eq!(string_field(output, "status"), Some("needs_agent"));
    Ok(())
}

#[test]
fn native_workflow_skill_run_resolves_agent_task_tool_output_context()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_agent_artifact_context_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers = serde_json::json!({
        "answers": {
            "author": {
                "fix_bundle": {
                    "message": "Workflow tool bug"
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
    let pending_output = object(&pending.output, "pending workflow agent artifact result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending workflow agent artifact result missing run_id")?
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

    let output = object(&result.output, "workflow agent artifact result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    assert_eq!(string_field(echo, "message"), Some("Workflow tool bug"));

    Ok(())
}

#[test]
fn native_workflow_skill_resume_preserves_initial_inputs_for_later_tool_steps()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_agent_then_input_tool_skill(temp.path())?;
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
            JsonValue::String("Workflow tool bug".to_owned()),
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
    let pending_output = object(&pending.output, "pending workflow input resume result")?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending workflow input resume result missing run_id")?
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

    let output = object(&result.output, "workflow input resume result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    assert_eq!(string_field(echo, "message"), Some("/evidence/non-default"));

    Ok(())
}

// Transport-envelope probing is intentionally forbidden: an agent claim must
// return the declared contract directly, so an `output`-wrapped answer fails
// with the typed missing-output error instead of being silently unwrapped.
#[test]
fn native_workflow_skill_run_refuses_agent_output_envelope_claim()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_agent_artifact_context_skill(temp.path())?;
    write_echo_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let answers = serde_json::json!({
        "answers": {
            "author": {
                "output": {
                    "fix_bundle": {
                        "message": "Workflow tool bug"
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
        "pending workflow agent artifact envelope result",
    )?;
    assert_eq!(string_field(pending_output, "status"), Some("needs_agent"));
    let run_id = string_field(pending_output, "run_id")
        .ok_or("pending workflow agent artifact envelope result missing run_id")?
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
        message.contains("method output contract violation at $.output"),
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
fn native_workflow_skill_run_omits_missing_optional_workflow_input_references()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_optional_json_tool_skill(temp.path())?;
    write_optional_json_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let tool_root = temp.path().join("tools");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Workflow optional JSON bug".to_owned()),
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

    let output = object(&result.output, "workflow optional JSON tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    assert_eq!(
        string_field(echo, "message"),
        Some("Workflow optional JSON bug")
    );

    Ok(())
}

#[test]
fn native_workflow_skill_run_requires_declared_workflow_inputs()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_required_input_skill(temp.path())?;
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
        .run_skill_with_binding(&request, None, None, None)
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
            .contains("method input 'lead' is required")
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
fn native_workflow_skill_resume_applies_approval_before_completing_step()
-> Result<(), Box<dyn std::error::Error>> {
    let case = "approvals-field";
    let answers = serde_json::json!({
        "approvals": { "approval-resume.approve": true }
    });
    let temp = tempdir()?;
    let skill_dir = write_workflow_approval_resume_skill(temp.path())?;
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
fn native_workflow_skill_resume_approves_only_what_the_run_waits_on()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_approval_resume_skill(temp.path())?;
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
    let request = runx_runtime::run_record::read_run(&BTreeMap::new(), temp.path(), &run_id, None)?
        .waiting
        .pop()
        .ok_or("the paused run's approval request is missing")?;
    // A person approves only what the run waits on: an approval of any other
    // request, such as one the resume would ask next, is refused.
    let mut resume = start;
    resume.answers = Some(serde_json::from_value(serde_json::json!({
        "approvals": {
            request.address.as_str(): { "request_digest": request.request_digest.as_str(), "approved": true },
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
            request.address.as_str(): { "request_digest": request.request_digest.as_str(), "approved": true }
        }
    }))?);
    let resumed = run_skill(resume)?;
    assert_eq!(resumed.status, runx_runtime::RunStatus::Sealed);
    assert!(resumed.succeeded());
    Ok(())
}

#[test]
fn native_workflow_skill_resume_rejects_agent_answer_for_approval()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_approval_resume_skill(temp.path())?;
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
fn native_workflow_skill_run_uses_canonical_tool_root() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_tool_skill_under_skills(temp.path())?;
    write_echo_tool_at(&temp.path().join("tools/test/echo"), "root tools")?;
    write_echo_tool_at(
        &temp.path().join("packages/cli/tools/test/echo"),
        "stale copy",
    )?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Workflow tool bug".to_owned()),
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

    let output = object(&result.output, "workflow tool result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing echo")?;
    assert_eq!(string_field(echo, "message"), Some("root tools"));

    Ok(())
}

#[test]
fn native_workflow_skill_run_merges_imported_workflow_skill_tool_roots()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_importing_workflow_with_bundled_tool(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Workflow tool bug".to_owned()),
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

    let output = object(&result.output, "nested workflow tool root result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let echo = object_field(public_result, "echo").ok_or("missing nested echo output")?;
    assert_eq!(
        string_field(echo, "message"),
        Some("Nested workflow tool root bug")
    );
    let root_receipt_id = string_field(output, "receipt_id").ok_or("missing receipt id")?;
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    let steps = array_field(trace, "steps").ok_or("missing workflow steps")?;
    let nested_step = object(&steps[0], "nested workflow step")?;
    let nested_step_id =
        string_field(nested_step, "receipt_id").ok_or("missing nested step receipt id")?;
    let root_receipt = crate::support::read_test_signed_receipt(&receipt_dir, root_receipt_id)?;
    let nested_step_receipt =
        crate::support::read_test_signed_receipt(&receipt_dir, nested_step_id)?;
    let nested_workflow_ref = nested_step_receipt
        .lineage
        .children
        .first()
        .ok_or("nested step receipt missing child workflow reference")?;
    let nested_workflow_id = nested_workflow_ref
        .uri
        .as_str()
        .strip_prefix("runx:receipt:")
        .ok_or("nested workflow receipt reference is malformed")?;
    let nested_workflow_receipt =
        crate::support::read_test_signed_receipt(&receipt_dir, nested_workflow_id)?;
    assert_eq!(
        nested_workflow_ref.locator.as_deref(),
        Some(nested_workflow_receipt.digest.as_str())
    );
    let nested_child_refs = &nested_workflow_receipt.lineage.children;
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
            nested_workflow_receipt,
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
fn native_workflow_skill_run_executes_nested_process_tool_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_nested_cli_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Nested workflow bug".to_owned()),
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

    let output = object(&result.output, "nested workflow skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let declared_nested =
        object_field(public_result, "nested").ok_or("missing exposed nested output")?;
    assert_eq!(
        string_field(declared_nested, "message"),
        Some("Nested workflow bug")
    );
    let context = object_field(output, "context").ok_or("missing workflow context")?;
    let step_outputs =
        object_field(context, "step_outputs").ok_or("missing declared step context")?;
    let nested_context =
        object_field(step_outputs, "nested").ok_or("missing nested step context")?;
    assert_eq!(
        object_field(nested_context, "nested").and_then(|nested| string_field(nested, "message")),
        Some("Nested workflow bug")
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
    let steps = array_field(trace, "steps").ok_or("missing workflow steps")?;
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
fn native_workflow_skill_run_executes_nested_registry_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = registry_child()?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;
    let env = fetch_registry_refs(&registry, temp.path(), &skill_dir)?;

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

    let output = object(&result.output, "nested registry skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("registry child"));

    Ok(())
}

/// An inspection reports a registry child the project does not hold, and
/// binds it once it is installed.
#[test]
fn skill_inspection_reports_registry_refs_the_project_does_not_hold()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = registry_child()?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;
    let env = registry_env(&registry, temp.path())?;
    let closure_of = |inspection: &runx_contracts::SkillInspection| -> Result<JsonValue, Box<dyn std::error::Error>> {
        Ok(JsonValue::Object(
            inspection
                .method_inspections
                .first()
                .ok_or("no method inspection")?
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

    fetch_registry_refs(&registry, temp.path(), &skill_dir)?;
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

/// No registry but the official one serves runx's own skills: a child
/// another registry signs under `runx/` is refused before it is installed.
#[test]
fn a_nested_runx_skill_from_another_registry_is_refused() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let registry = LoopbackRegistry::serve()?;
    registry.publish(Publication::new(
        "runx",
        "1.0.0",
        "---\nname: registry-child\ndescription: Official registry-backed nested child.\n---\n# Registry Child\n",
        Some(&registry_child_profile_document()),
    ))?;
    let skill_dir = write_workflow_nested_registry_skill_with_ref(
        temp.path(),
        "registry:runx/registry-child@1.0.0",
    )?;

    let error = fetch_registry_refs(&registry, temp.path(), &skill_dir)
        .err()
        .ok_or("a runx skill from another registry was installed")?;
    assert!(
        error.to_string().contains("may not sign runx/*"),
        "unexpected error: {error}"
    );
    assert!(!temp.path().join(".runx/added-skills/runx").exists());

    Ok(())
}

#[test]
fn native_workflow_skill_run_rejects_unsigned_nested_registry_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = registry_child()?;
    registry.tamper("acme/registry-child", "1.0.0", |record| {
        if let Some(record) = record.as_object_mut() {
            record.remove("signed_manifest");
        }
    })?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;

    let error = fetch_registry_refs(&registry, temp.path(), &skill_dir)
        .err()
        .ok_or("an unsigned nested registry skill was installed")?;
    assert!(
        error.to_string().contains("signed manifest is required"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[test]
fn native_workflow_skill_run_rejects_tampered_nested_registry_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = registry_child()?;
    registry.tamper("acme/registry-child", "1.0.0", |record| {
        let tampered = record["markdown"]
            .as_str()
            .map(|markdown| markdown.replace("Registry", "Tampered"));
        record["markdown"] = serde_json::json!(tampered);
    })?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;

    let error = fetch_registry_refs(&registry, temp.path(), &skill_dir)
        .err()
        .ok_or("a tampered nested registry skill was installed")?;
    assert!(
        error.to_string().contains("digest mismatch"),
        "unexpected error: {error}"
    );

    Ok(())
}

#[test]
fn native_workflow_skill_run_rejects_a_nested_registry_skill_the_project_does_not_hold()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;
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
fn native_workflow_skill_run_does_not_rerun_final_step() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_nested_cli_counter_skill(temp.path())?;
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

    let output = object(&result.output, "counter workflow skill result")?;
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
        .ok_or("pinned workflow identity must reject input drift")?;
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
        "SELECT data FROM kernel_events WHERE stream = 'run' AND key = 'run_pinned_counter' AND version = 2",
        [],
        |row| row.get(0),
    )?;
    database.execute(
        "UPDATE kernel_events SET data = ?1 WHERE stream = 'run' AND key = 'run_pinned_counter' AND version = 2",
        [b"{}".to_vec()],
    )?;
    let error = run_skill(request.clone())
        .err()
        .ok_or("a rewritten run record must hold")?;
    assert!(error.to_string().contains("integrity chain"), "{error}");
    assert_eq!(fs::read_to_string(&count_file)?, "1");
    database.execute(
        "UPDATE kernel_events SET data = ?1 WHERE stream = 'run' AND key = 'run_pinned_counter' AND version = 2",
        [original_event],
    )?;
    drop(database);

    let receipt_id = result
        .receipt_ids
        .first()
        .ok_or("missing workflow receipt")?;
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
fn native_workflow_skill_run_executes_workflow_stage_process_tool_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_stage_cli_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Stage workflow bug".to_owned()),
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

    let output = object(&result.output, "stage workflow skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("Stage workflow bug"));
    let trace = object_field(output, "trace").ok_or("missing trace")?;
    let steps = array_field(trace, "steps").ok_or("missing workflow steps")?;
    let nested_step_summary = object(&steps[0], "nested step summary")?;
    assert_eq!(
        string_field(nested_step_summary, "skill"),
        Some("child-echo")
    );

    Ok(())
}

#[test]
fn native_workflow_skill_run_executes_nested_x_yaml_method_skill()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_nested_x_yaml_cli_skill(temp.path())?;
    let receipt_dir = temp.path().join("receipts");
    let inputs = [(
        "thread_title".to_owned(),
        JsonValue::String("Method manifest bug".to_owned()),
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

    let output = object(&result.output, "nested run.yaml workflow skill result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    let public_result = object_field(output, "result").ok_or("missing result")?;
    let nested = object_field(public_result, "nested").ok_or("missing nested output")?;
    assert_eq!(string_field(nested, "message"), Some("Method manifest bug"));

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
    let pending =
        LocalOrchestrator::default().run_skill_with_binding(&request, None, None, None)?;
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
        .run_skill_with_binding(&request, None, None, None)
        .map_err(Into::into)
}

fn run_skill(request: SkillRunRequest) -> Result<RunResult, Box<dyn std::error::Error>> {
    let request = with_test_signing_env(request);
    LocalOrchestrator::default()
        .run_skill_with_binding(&request, None, None, None)
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
        "---\nname: code-intake\ndescription: code-intake fixture.\n---\n# Issue Intake\n\n## Agent task: code-intake\n\nCode-intake.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  intake:
    inputs:
      thread_title:
        type: string
        optional: true
    steps:
      - id: intake
        agent: code-intake
        inputs:
          thread_title: $input.thread_title
        outputs:
          intake_report: {type: object}
          claimed_proof:
            type: object
            optional: true
          verification:
            type: object
            optional: true
          signal:
            type: object
            optional: true
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_workflow_agent_task_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-issue-to-pr");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-issue-to-pr\ndescription: workflow-issue-to-pr fixture.\n---\n# Graph Issue To PR\n\n## Agent task: workflow-decide\n\nUse the full issue context.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
        optional: true
    result_from:
      - decide
    steps:
      - id: decide
        agent: workflow-decide
        inputs:
          thread_title: $input.thread_title
        outputs:
          result: {type: object}
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_workflow_agent_task_with_context_skill(
    root: &Path,
    context_skill: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-agent-context-skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-agent-context-skill\ndescription: workflow-agent-context-skill fixture.\n---\n# Graph Agent Context Skill\n\n## Agent task: apply taste guidance\n\nApply taste guidance.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
minimum_runx: 0.12.0
methods:
  graph:
    result_from:
      - apply_taste
    steps:
      - id: apply_taste
        agent: apply taste guidance
        outputs:
          summary: {{type: string}}
        context:
          skills:
            - "{context_skill}"
"#
        ),
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_workflow_gated_agent_task_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    write_workflow_gated_agent_task_skill_with_field(root, "decide.skill_claim.approved")
}

fn write_workflow_gated_agent_task_skill_with_field(
    root: &Path,
    gate_field: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-gated-agent-task");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-gated-agent-task\ndescription: workflow-gated-agent-task fixture.\n---\n# Graph Gated Agent Step\n\n## Agent task: gated-decide\n\nGated-decide.\n\n## Agent task: gated-followup\n\nGated-followup.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
minimum_runx: 0.12.0
methods:
  graph:
    result_from:
      - gated
    steps:
      - id: decide
        agent: gated-decide
        outputs:
          approved: {{type: boolean}}
      - id: gated
        guard:
          - {{value: $steps.{gate_field}, equals: true}}
        agent: gated-followup
        outputs:
          result: {{type: object}}
            "#
        ),
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_workflow_nested_agent_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_name = "child-agent";
    let child_dir = root.join(child_name);
    let method = r#"    inputs:
      thread_title:
        type: string
        optional: true
    steps:
      - id: author
        agent: child-agent
        inputs:
          thread_title: $input.thread_title
        outputs:
          result: {type: object}
"#;
    crate::support::write_test_skill_package(
        &child_dir,
        format!(
            r#"---
name: {child_name}
description: {child_name} fixture.
---
# {child_name}

## Agent task: {child_name}

Author the result.
"#
        )
        .as_str(),
        format!(
            r#"minimum_runx: 0.12.0
methods:
  {child_name}:
{method}"#
        )
        .as_str(),
    )?;

    let skill_dir = root.join("workflow-nested-agent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-nested-agent\ndescription: workflow-nested-agent fixture.\n---\n# Graph Nested agent\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
        optional: true
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

fn write_workflow_tool_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-tool");
    write_workflow_tool_skill_at(&skill_dir)
}

fn write_workflow_tool_skill_under_skills(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("skills/workflow-tool");
    write_workflow_tool_skill_at(&skill_dir)
}

fn write_workflow_importing_workflow_with_bundled_tool(
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
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
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
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      message:
        type: string
    result_from:
      - echo
    steps:
      - id: echo
        tool: test.echo
        inputs:
          message: $input.message
"#,
    )?;
    write_echo_tool_at(
        &child_dir.join("tools/test/echo"),
        "Nested workflow tool root bug",
    )?;
    Ok(parent_dir)
}

fn write_workflow_tool_skill_at(skill_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    fs::create_dir_all(skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-tool\ndescription: workflow-tool fixture.\n---\n# Graph Tool\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
    result_from:
      - echo
    steps:
      - id: echo
        tool: test.echo
        inputs:
          message: $input.thread_title
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

fn write_workflow_agent_artifact_context_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-agent-artifact-context");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-agent-artifact-context\ndescription: workflow-agent-artifact-context fixture.\n---\n# Graph Agent Artifact Context\n\n## Agent task: workflow-author\n\nWorkflow-author.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    result_from:
      - echo
    steps:
      - id: author
        agent: workflow-author
        outputs:
          fix_bundle: {type: object}
      - id: echo
        tool: test.echo
        inputs:
          message: $steps.author.fix_bundle.message
"#,
    )?;
    Ok(skill_dir)
}

fn write_workflow_agent_then_input_tool_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-agent-then-input-tool");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-agent-then-input-tool\ndescription: workflow-agent-then-input-tool fixture.\n---\n# Graph Agent Then Input Tool\n\n## Agent task: workflow-author\n\nWorkflow-author.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
      repo_root:
        type: string
        default: .
        optional: true
    result_from:
      - echo
    steps:
      - id: author
        agent: workflow-author
        outputs:
          result: {type: object}
      - id: echo
        tool: test.echo
        inputs:
          message: $input.repo_root
"#,
    )?;
    Ok(skill_dir)
}

fn write_workflow_optional_json_tool_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-optional-json-tool");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-optional-json-tool\ndescription: workflow-optional-json-tool fixture.\n---\n# Graph Optional JSON Tool\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
      test:
        optional: true
    result_from:
      - echo
    steps:
      - id: echo
        tool: test.optional-json
        inputs:
          message: $input.thread_title
          test: $input.test
"#,
    )?;
    Ok(skill_dir)
}

fn write_workflow_required_input_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-required-input");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-required-input\ndescription: workflow-required-input fixture.\n---\n# Graph Required Input\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      lead:
        description: Lead packet to route.
    result_from:
      - approve
    steps:
      - id: approve
        ask: {gate: workflow-required-input.approve, reason: approve the graph}
"#,
    )?;
    Ok(skill_dir)
}

fn write_workflow_approval_resume_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("approval-resume");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: approval-resume\ndescription: approval-resume fixture.\n---\n# Approval Resume\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    result_from:
      - approve
    steps:
      - id: approve
        ask: {gate: approval-resume.approve, reason: approve the test graph, type: test.claim-bound}
"#,
    )?;
    Ok(skill_dir)
}

fn write_echo_tool(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    write_echo_tool_at(&root.join("tools/test/echo"), "Workflow tool bug")
}

fn write_echo_tool_at(tool_dir: &Path, message: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(tool_dir)?;
    fs::write(
        tool_dir.join("manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "test.echo",
  "process": {
    "command": "sh",
    "args": ["./run.sh"],
    "inputs_via": "stdin"
  },
  "inputs": {
    "message": { "type": "string" }
  },
  "outputs": {
    "echo": { "optional": true }
  }
}
"#,
    )?;
    fs::write(
        tool_dir.join("run.sh"),
        format!(
            r#"raw="$(cat)"
case "$raw" in
  *"Workflow tool bug"*|*"{}"*) printf '%s\n' '{{"echo":{{"message":"{}"}}}}' ;;
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
  "process": {
    "command": "sh",
    "args": ["./run.sh"],
    "inputs_via": "stdin"
  },
  "inputs": {
    "message": { "type": "string" },
    "test": { "optional": true }
  },
  "outputs": {
    "echo": { "optional": true }
  }
}
"#,
    )?;
    fs::write(
        tool_dir.join("run.sh"),
        r#"raw="$(cat)"
case "$raw" in
  *'$input.test'*)
    printf '%s\n' '{"error":"unresolved test reference reached tool input"}'
    exit 2
    ;;
  *'"test"'*)
    printf '%s\n' '{"error":"optional test should be omitted when absent"}'
    exit 3
    ;;
  *"Workflow optional JSON bug"*)
    printf '%s\n' '{"echo":{"message":"Workflow optional JSON bug"}}'
    ;;
  *)
    printf '%s\n' '{"echo":{"message":"unexpected"}}'
    ;;
esac
"#,
    )?;
    Ok(())
}

fn write_workflow_nested_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-echo");
    crate::support::write_test_skill_package(
        &child_dir,
        r#"---
name: child-echo
description: child-echo fixture.
---
# Child Echo
"#,
        r#"minimum_runx: 0.12.0
methods:
  child-echo:
    inputs:
      message:
        type: string
    steps:
      - id: child-echo
        tool: {command: node, args: [run.mjs], inputs_via: stdin}
        inputs:
          message: $input.message
        outputs:
          nested: {type: object}
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

    let skill_dir = root.join("workflow-nested-cli");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-nested-cli\ndescription: workflow-nested-cli fixture.\n---\n# Graph Nested CLI\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
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

fn write_workflow_nested_registry_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    write_workflow_nested_registry_skill_with_ref(root, "registry:acme/registry-child@1.0.0")
}

fn write_workflow_nested_registry_skill_with_ref(
    root: &Path,
    skill_ref: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-nested-registry");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-nested-registry\ndescription: workflow-nested-registry fixture.\n---\n# Graph Nested Registry\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        format!(
            r#"
minimum_runx: 0.12.0
methods:
  graph:
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

fn write_workflow_stage_cli_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-stage-cli");
    let stage_dir = skill_dir.join("workflow/child-echo");
    crate::support::write_test_skill_package(
        &stage_dir,
        r#"---
name: child-echo
description: child-echo fixture.
---
# Child Echo
"#,
        r#"minimum_runx: 0.12.0
methods:
  child-echo:
    inputs:
      message:
        type: string
    steps:
      - id: child-echo
        tool: {command: node, args: [run.mjs], inputs_via: stdin}
        inputs:
          message: $input.message
        outputs:
          nested: {type: object}
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
        "---\nname: workflow-stage-cli\ndescription: workflow-stage-cli fixture.\n---\n# Graph Stage CLI\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
    result_from:
      - nested
    steps:
      - id: nested
        skill: workflow/child-echo
        inputs:
          message: $input.thread_title
"#,
    )?;
    Ok(skill_dir)
}

fn write_workflow_nested_cli_counter_skill(
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
        r#"minimum_runx: 0.12.0
methods:
  child-counter:
    inputs:
      count_file:
        type: string
    steps:
      - id: child-counter
        tool: {command: node, args: [run.mjs], inputs_via: stdin}
        inputs:
          count_file: $input.count_file
        outputs:
          counted: {type: object}
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

    let skill_dir = root.join("workflow-nested-cli-counter");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-nested-cli-counter\ndescription: workflow-nested-cli-counter fixture.\n---\n# Graph Nested CLI Counter\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      count_file:
        type: string
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

fn write_workflow_nested_x_yaml_cli_skill(
    root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let child_dir = root.join("child-x-cli");
    fs::create_dir_all(&child_dir)?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: child-x-cli\ndescription: child-x-cli fixture.\n---\n# Child X CLI\n",
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  child-cli:
    inputs:
      message:
        type: string
    steps:
      - id: child-cli
        tool: {command: node, args: [run.mjs], inputs_via: stdin}
        inputs:
          message: $input.message
        outputs:
          nested: {type: object}
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

    let skill_dir = root.join("workflow-nested-x-yaml-cli");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-nested-x-yaml-cli\ndescription: workflow-nested-x-yaml-cli fixture.\n---\n# Graph Nested X YAML CLI\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    inputs:
      thread_title:
        type: string
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

fn write_workflow_when_branch_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("workflow-when-branch");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: workflow-when-branch\ndescription: workflow-when-branch fixture.\n---\n# Graph When Branch\n\n## Agent task: when-decide\n\nWhen-decide.\n\n## Agent task: when-go\n\nWhen-go.\n\n## Agent task: when-stop\n\nWhen-stop.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  graph:
    result_from:
      - branch_go
      - branch_stop
    steps:
      - id: decide
        agent: when-decide
        outputs:
          verdict: {type: string}
      - id: branch_go
        when:
          value: $steps.decide.verdict
          equals: go
        agent: when-go
        outputs:
          result: {type: object}
      - id: branch_stop
        when:
          value: $steps.decide.verdict
          equals: stop
        agent: when-stop
        outputs:
          result: {type: object}
"#,
    )?;
    Ok(skill_dir.to_path_buf())
}

#[test]
fn native_workflow_when_skips_unselected_branch() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let skill_dir = write_workflow_when_branch_skill(temp.path())?;
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
    let output = object(&initial.output, "when workflow result")?;
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
    let branched = object(&branched.output, "branched when workflow result")?;
    assert_eq!(string_field(branched, "status"), Some("needs_agent"));
    let sealed = answer(serde_json::json!({
        "answers": { "branch_go": { "result": { "ok": true }, "closure": { "outcome": "completed" } } }
    }))?;
    let output = object(&sealed.output, "sealed when workflow result")?;
    assert_eq!(string_field(output, "status"), Some("sealed"));
    Ok(())
}

/// A publish binds a skill another publisher owns from the registry it is
/// published to: read by its record, never acquired, and installed,
/// verified, into the publish's own project, never the publisher's.
#[test]
fn a_publish_binds_another_publishers_skill_from_the_registry_it_targets()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    // The registry the package is published to holds acme's skill.
    let registry = registry_child()?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;
    let env = registry_env(&registry, temp.path())?;
    let subject = skill_dir.to_string_lossy().into_owned();
    let request = runx_runtime::registry::RegistryPublishPackageRequest {
        subject: &subject,
        env: &env,
        cwd: temp.path(),
    };
    assert_eq!(
        runx_runtime::registry::unheld_publish_registry_refs(request)?,
        ["registry:acme/registry-child@1.0.0"]
    );

    let client = runx_runtime::registry::resolve::registry_client(&registry.url, &env)?;
    let closure =
        runx_runtime::registry::materialize_publish_closure(&client, &registry.url, request)?;

    let project = closure.project();
    assert!(
        project
            .join("added-skills/acme/registry-child/1.0.0/SKILL.md")
            .is_file()
    );
    assert!(
        project
            .join("added-skills/acme/registry-child/1.0.0/signature.json")
            .is_file()
    );
    assert!(!temp.path().join(".runx/added-skills").exists());
    assert!(
        registry
            .asked()
            .iter()
            .all(|line| !line.contains("/acquire")),
        "{:?}",
        registry.asked()
    );
    let bound = runx_runtime::registry::RegistryPublishPackageRequest {
        env: &closure.env,
        ..request
    };
    assert!(runx_runtime::registry::unheld_publish_registry_refs(bound)?.is_empty());
    runx_runtime::registry::prepare_registry_publish_package(bound)?;
    Ok(())
}

/// What a publish binds is what the registry signed: a record whose files
/// do not match its envelope is refused before anything is installed.
#[test]
fn a_publish_refuses_a_closure_record_its_registry_did_not_sign()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = registry_child()?;
    registry.tamper("acme/registry-child", "1.0.0", |record| {
        let tampered = record["markdown"]
            .as_str()
            .map(|markdown| markdown.replace("Registry", "Tampered"));
        record["markdown"] = serde_json::json!(tampered);
    })?;
    let skill_dir = write_workflow_nested_registry_skill(temp.path())?;
    let env = registry_env(&registry, temp.path())?;
    let subject = skill_dir.to_string_lossy().into_owned();
    let request = runx_runtime::registry::RegistryPublishPackageRequest {
        subject: &subject,
        env: &env,
        cwd: temp.path(),
    };
    let client = runx_runtime::registry::resolve::registry_client(&registry.url, &env)?;

    let error = match runx_runtime::registry::materialize_publish_closure(
        &client,
        &registry.url,
        request,
    ) {
        Ok(_) => return Err("a closure bound a record its registry did not sign".into()),
        Err(error) => error,
    };
    assert!(
        error.to_string().contains("digest mismatch"),
        "unexpected error: {error}"
    );
    Ok(())
}

/// A skill-build candidate's test reads the registry children its closure
/// names from the registry its operator selects: each installed, verified,
/// into the candidate's own project before its cases run, as a publish binds
/// its closure.
#[test]
fn a_candidates_test_reads_its_registry_children_from_the_operators_registry()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let registry = registry_child()?;
    let candidate = temp.path().join("skills/candidate");
    crate::support::write_test_skill_package(
        &candidate,
        "---\nname: candidate\ndescription: Runs acme's registry child.\n---\n# Candidate\n",
        "minimum_runx: 0.12.0\nmethods:\n  graph:\n    result_from: [nested]\n    steps:\n      - id: nested\n        skill: registry:acme/registry-child@1.0.0\n",
    )?;
    fs::create_dir_all(candidate.join("tests"))?;
    fs::write(
        candidate.join("tests/runs-the-registry-child.yaml"),
        "name: runs-the-registry-child\nmethod: graph\nexpect:\n  status: sealed\n",
    )?;
    let validator = temp.path().join("validator");
    crate::support::write_test_skill_package(
        &validator,
        "---\nname: validator\ndescription: Validates the candidate.\n---\n# Validator\n",
        "minimum_runx: 0.12.0\nmethods:\n  validate:\n    result_from: [validate]\n    steps:\n      - id: validate\n        tool: runx.skill.validate\n        inputs:\n          repo_root: .\n          skill_path: skills/candidate\n          allow_execute_test: true\n",
    )?;
    let mut env = registry_env(&registry, temp.path())?;
    env.extend(path_env());
    env.insert("RUNX_REGISTRY_URL".to_owned(), registry.url.clone());

    let result = run_skill(SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: validator,
        receipt_dir: Some(temp.path().join("receipts")),
        run_id: None,
        answers: None,
        inputs: BTreeMap::new(),
        env,
        cwd: temp.path().to_path_buf(),
        managed_agent: Default::default(),
    })?;

    let output = serde_json::to_value(&result.output)?;
    let validation = &output["result"]["skill_validation"]["data"];
    assert_eq!(validation["verdict"], "tested", "{output}");
    assert_eq!(validation["test"]["status"], "passed", "{output}");
    // Read by its record, never acquired, and nothing added to the operator's
    // project.
    assert!(
        registry
            .asked()
            .iter()
            .any(|line| line.contains("/versions/1.0.0/record")),
        "{:?}",
        registry.asked()
    );
    assert!(
        registry
            .asked()
            .iter()
            .all(|line| !line.contains("/acquire")),
        "{:?}",
        registry.asked()
    );
    assert!(!temp.path().join(".runx/added-skills/acme").exists());
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
        "minimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: write\n        tool: {command: sh, args: [./run.sh]}\n        outputs: {done: {type: boolean}}\n",
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
            .run_skill_with_binding(&first_request, None, None, None)
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
    let second = LocalOrchestrator::default().run_skill_with_binding(&request, None, None, None);
    let first = first.join().map_err(|_| "the first driver panicked")?;

    assert_eq!(first, Ok(true));
    assert!(
        second.as_ref().map_or(true, |result| !result.succeeded()),
        "the second driver must not seal the run again"
    );
    assert_eq!(fs::read_to_string(&marker)?.lines().count(), 1);
    Ok(())
}
