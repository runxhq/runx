use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

use base64::Engine;
use ring::signature::KeyPair;
use serde_json::json;

const TEST_MANIFEST_KEY_ID: &str = "runx-registry-skill-test-key";
const TEST_MANIFEST_SIGNER_ID: &str = "runx-registry-skill-test-signer";
const TEST_MANIFEST_SEED: [u8; 32] = [7; 32];

#[test]
fn input_document_supports_file_and_stdin_without_a_second_input_map()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-input-document");
    let skill_dir = crate::support::write_agent_task_skill(&root.join("skills"))?;
    fs::write(root.join("inputs.json"), r#"{"thread_title":"from-file"}"#)?;
    let workspace = runx_runtime::WorkspaceEnv::load_process(root.clone())?;
    let args = [
        "skill",
        skill_dir.to_str().ok_or("skill path was not UTF-8")?,
        "--inputs",
        "inputs.json",
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .collect::<Vec<_>>();

    let plan = runx_cli::skill::parse_skill_plan_with_workspace(&args, &workspace)?;
    assert!(plan.inputs.is_empty());
    assert_eq!(
        plan.input_document,
        Some(runx_cli::document_input::DocumentInputSource::Path(
            PathBuf::from("inputs.json")
        ))
    );

    let mixed = [
        "skill",
        skill_dir.to_str().ok_or("skill path was not UTF-8")?,
        "--inputs",
        "inputs.json",
        "--input",
        "thread_title=inline",
    ]
    .into_iter()
    .map(std::ffi::OsString::from)
    .collect::<Vec<_>>();
    let error = runx_cli::skill::parse_skill_plan_with_workspace(&mixed, &workspace)
        .err()
        .ok_or_else(|| std::io::Error::other("mixed document and per-key inputs should fail"))?;
    assert!(error.contains("cannot be combined"));

    let output = runx_command()
        .current_dir(&root)
        .env("RUNX_CWD", &root)
        .args([
            "skill",
            skill_dir.to_str().ok_or("skill path was not UTF-8")?,
            "--inputs",
            "inputs.json",
            "--json",
        ])
        .output()?;
    let value = assert_json(&output, Some(2))?;
    let request = pending_request_artifact(&value)?;
    assert_eq!(
        request["invocation"]["envelope"]["inputs"]["thread_title"],
        "from-file"
    );

    let mut child = runx_command()
        .current_dir(&root)
        .env("RUNX_CWD", &root)
        .args([
            "skill",
            skill_dir.to_str().ok_or("skill path was not UTF-8")?,
            "--inputs",
            "-",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("skill stdin was not piped")?
        .write_all(br#"{"thread_title":"from-stdin"}"#)?;
    let output = child.wait_with_output()?;
    let value = assert_json(&output, Some(2))?;
    let request = pending_request_artifact(&value)?;
    assert_eq!(
        request["invocation"]["envelope"]["inputs"]["thread_title"],
        "from-stdin"
    );
    Ok(())
}

#[test]
fn input_document_rejects_paths_outside_the_invocation_workspace()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-input-containment");
    let skill_dir = crate::support::write_agent_task_skill(&root.join("skills"))?;
    let outside = crate::support::temp_root("runx-skill-input-outside").join("inputs.json");
    fs::create_dir_all(outside.parent().ok_or("outside fixture has no parent")?)?;
    fs::write(&outside, r#"{"thread_title":"outside"}"#)?;

    for path in [
        outside.to_string_lossy().into_owned(),
        "../inputs.json".to_owned(),
    ] {
        let output = runx_command()
            .current_dir(&root)
            .env("RUNX_CWD", &root)
            .args([
                "skill",
                skill_dir.to_str().ok_or("skill path was not UTF-8")?,
                "--inputs",
                &path,
                "--json",
            ])
            .output()?;

        assert!(!output.status.success());
        assert!(
            String::from_utf8(output.stdout)?
                .contains("workspace file path must be a non-empty relative path")
        );
    }
    Ok(())
}

#[test]
fn native_skill_resolves_bare_local_skill_and_documented_input_flags()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-bare-ref");
    let skills_root = root.join("skills");
    fs::create_dir_all(&skills_root)?;
    let skill_dir = crate::support::write_agent_task_skill(&skills_root)?;
    let profile_path = skill_dir.join("X.yaml");
    let profile = fs::read_to_string(&profile_path)?;
    fs::write(
        &profile_path,
        profile.replace(
            "      thread_title:\n        type: string\n        required: false\n",
            "      thread_title:\n        type: string\n        required: false\n      severity:\n        type: string\n        required: false\n",
        ),
    )?;
    let receipt_dir = root.join("receipts");

    let output = runx_command()
        .current_dir(&root)
        .args([
            "skill",
            "issue-intake",
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--input",
            "thread-title=Docs bug",
            "--input",
            "severity",
            "low",
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(2))?;
    let request = pending_request_artifact(&output_json)?;
    let inputs = &request["invocation"]["envelope"]["inputs"];
    assert_eq!(inputs["thread_title"], "Docs bug");
    assert_eq!(inputs["severity"], "low");
    let actual_skill_dir = PathBuf::from(
        request["invocation"]["envelope"]["execution_location"]["skill_directory"]
            .as_str()
            .ok_or("missing skill directory")?,
    );
    assert_eq!(actual_skill_dir.canonicalize()?, skill_dir.canonicalize()?);

    Ok(())
}

#[test]
fn native_skill_prints_operator_context_and_admits_safe_run_by_default()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-operator-context");
    let skill_dir = write_operator_context_skill(&root)?;

    let output = runx_command()
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;

    assert_eq!(
        output.status.code(),
        Some(2),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8(output.stderr)?;
    assert!(stderr.contains("Prepared run"));
    assert!(stderr.contains("Steps:"));
    assert!(stderr.contains("Tools:"));
    assert!(stderr.contains("Boundaries:"));
    assert!(stderr.contains("trusted_host_process"));
    assert!(stderr.contains("remote_provider"));
    assert!(stderr.contains("Full context: add --full-operator-context"));
    assert!(!stderr.contains("--- root skill ---"));
    assert!(!stderr.contains("# Operator Context Fixture"));
    let stdout = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(stdout["status"], "needs_agent");
    assert!(stdout.get("approval_flag").is_none());
    let request = pending_request_artifact(&stdout)?;
    let instructions = request["invocation"]["envelope"]["instructions"]
        .as_str()
        .ok_or("missing nested skill instructions")?;
    assert!(instructions.contains("# Nested Review Skill"));
    assert!(instructions.contains("Judge the work against the supplied review-rubric"));
    assert!(!instructions.contains("# Operator Context Fixture"));

    let full = runx_command()
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
            "--full-operator-context",
        ])
        .output()?;
    assert_eq!(full.status.code(), Some(2));
    let full_stderr = String::from_utf8(full.stderr)?;
    assert!(full_stderr.contains("Full operator context"));
    assert!(full_stderr.contains("--- root skill ---"));
    assert!(full_stderr.contains("# Operator Context Fixture"));
    assert!(full_stderr.contains("--- skill node: entry.review ---"));
    assert!(full_stderr.contains("execution_boundary: remote_provider"));
    assert!(full_stderr.contains("execution_boundary: trusted_host_process"));
    assert!(full_stderr.contains("context skill: ./context/review-rubric"));
    assert!(full_stderr.contains("production bar from context skill"));
    assert!(full_stderr.contains("tool manifest: example.record at entry.review"));
    let full_stdout = serde_json::from_slice::<serde_json::Value>(&full.stdout)?;
    assert_eq!(full_stdout["status"], "needs_agent");

    Ok(())
}

#[test]
fn graph_action_approval_yields_one_operator_resolution() -> Result<(), Box<dyn std::error::Error>>
{
    let root = crate::support::temp_root("runx-explicit-approval");
    fs::create_dir_all(&root)?;
    let receipt_dir = root.join(".runx/receipts");
    let approval_skill = write_approval_graph_skill(&root)?;

    let graph_run = crate::support::unsigned_runx_command_at(&root)
        .args([
            "skill",
            approval_skill
                .to_str()
                .ok_or("non-utf8 approval skill dir")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(
        graph_run.status.code(),
        Some(2),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&graph_run.stdout),
        String::from_utf8_lossy(&graph_run.stderr)
    );
    let graph_json = serde_json::from_slice::<serde_json::Value>(&graph_run.stdout)?;
    assert_eq!(graph_json["status"], "needs_approval");
    assert_eq!(graph_json["requests"][0]["kind"], "approval");

    let signed_run = runx_command()
        .current_dir(&root)
        .env("RUNX_HOME", root.join("home"))
        .args([
            "skill",
            approval_skill
                .to_str()
                .ok_or("non-utf8 approval skill dir")?,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(signed_run.status.code(), Some(2));
    let signed_json = serde_json::from_slice::<serde_json::Value>(&signed_run.stdout)?;
    assert_eq!(signed_json["status"], "needs_approval");
    assert_eq!(signed_json["requests"][0]["kind"], "approval");

    let run_id = graph_json["run_id"].as_str().ok_or("missing run id")?;
    let request_id = graph_json["requests"][0]["id"]
        .as_str()
        .ok_or("missing request id")?;
    let answers = root.join("decline.json");
    fs::write(
        &answers,
        serde_json::json!({"approvals": { request_id: false }}).to_string(),
    )?;
    let declined = crate::support::unsigned_runx_command_at(&root)
        .arg("resume")
        .arg(run_id)
        .arg(&answers)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let declined = serde_json::from_slice::<serde_json::Value>(&declined.stdout)?;
    assert_eq!(declined["status"], "sealed");
    assert_ne!(declined["closure"]["disposition"], "closed");
    assert!(runx_runtime::journal::find_paused_run(&receipt_dir, run_id)?.is_none());
    let receipt_id = declined["receipt_id"]
        .as_str()
        .ok_or("missing receipt id")?;
    let receipt_path =
        runx_runtime::LocalReceiptStore::new(&receipt_dir).receipt_path(receipt_id)?;
    fs::remove_file(&receipt_path)?;
    assert!(!receipt_path.exists());
    let replay = runx_runtime::orchestrator::LocalOrchestrator::default().run_skill_with_runner(
        &runx_runtime::orchestrator::SkillRunRequest {
            skill_path: approval_skill,
            receipt_dir: Some(receipt_dir),
            run_id: Some(run_id.to_owned()),
            answers_path: None,
            inputs: std::collections::BTreeMap::new(),
            env: std::collections::BTreeMap::from([(
                "RUNX_HOME".to_owned(),
                root.join("home").to_string_lossy().into_owned(),
            )]),
            cwd: root,
            managed_agent: runx_runtime::ManagedAgentPolicy::HostDriven,
            local_credential: None,
        },
        "approval-graph",
    )?;
    assert!(!replay.succeeded());
    assert_eq!(
        replay.receipt_refs.first().map(String::as_str),
        declined["receipt_id"].as_str()
    );
    assert!(receipt_path.exists());

    Ok(())
}

#[test]
fn native_skill_positional_runner_selects_non_default_runner()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-positional-runner");
    let skill_dir = write_multi_runner_skill(&root)?;
    let receipt_dir = root.join("receipts");

    let output = runx_command()
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "second",
            "--run",
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(2))?;
    assert_eq!(
        output_json["requests"][0]["id"],
        "agent_task.second-task.output"
    );

    Ok(())
}

#[test]
fn native_skill_inspect_reports_declared_credential_readiness()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-credential-inspect");
    let skill_dir = write_credential_skill(&root)?;

    let missing = runx_command()
        .current_dir(&root)
        .env_remove("EXAMPLE_API_KEY")
        .args([
            "skill",
            "inspect",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;
    let missing_json = assert_json(&missing, Some(0))?;
    assert_eq!(missing_json["readiness"]["status"], "needs_credential");
    assert_eq!(missing_json["credential"]["provider"], "example");
    assert_eq!(missing_json["credential"]["status"], "missing");
    assert_eq!(
        missing_json["credential"]["setup"][0],
        "runx credential set example --from-stdin"
    );

    let ready = runx_command()
        .current_dir(&root)
        .env("EXAMPLE_API_KEY", "inspect-secret-sentinel")
        .args([
            "skill",
            "inspect",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;
    let ready_json = assert_json(&ready, Some(0))?;
    assert_eq!(ready_json["readiness"]["status"], "ready");
    assert_eq!(ready_json["credential"]["status"], "ready");
    assert!(!String::from_utf8(ready.stdout)?.contains("inspect-secret-sentinel"));
    Ok(())
}

#[test]
fn native_skill_inspect_binds_pinned_local_registry_dependencies()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-inspect-registry-closure");
    let registry_dir = publish_registry_echo_version(&root, "1.0.0", "# Echo\n", true)?;
    let skill_dir = root.join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\n---\n# Parent\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"skill: parent
runners:
  default:
    default: true
    type: graph
    graph:
      name: parent
      result_from: [child]
      steps:
        - id: child
          skill: registry:acme/echo@1.0.0
          runner: default
"#,
    )?;

    let output = trusted_registry_runx_command(&root)?
        .current_dir(&root)
        .env("RUNX_REGISTRY_DIR", &registry_dir)
        .args([
            "skill",
            "inspect",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;
    let inspected = assert_json(&output, Some(0))?;
    let closure = &inspected["execution_closure"];
    assert_eq!(closure["fully_bound"], true);
    assert_eq!(closure["unresolved_skill_edges"], json!([]));
    assert_eq!(
        closure["package_bindings"]
            .as_array()
            .ok_or("package bindings are missing")?
            .len(),
        2,
    );
    Ok(())
}

#[test]
fn native_skill_exported_shim_resolves_to_source_skill() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-exported-shim");
    let source_dir = crate::support::write_agent_task_skill(&root.join("source with spaces"))?;
    let shim_dir = root.join("claude").join("issue-intake");
    fs::create_dir_all(&shim_dir)?;
    fs::write(
        shim_dir.join("SKILL.md"),
        format!(
            "---\nname: issue-intake\n---\n# issue-intake\n<!-- runx-export:claude source={} - generated, do not edit -->\n",
            source_dir.display()
        ),
    )?;

    let output = runx_command()
        .args([
            "skill",
            shim_dir.to_str().ok_or("non-utf8 shim dir")?,
            "--thread-title",
            "Docs bug",
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(2))?;
    let request = pending_request_artifact(&output_json)?;
    let actual_source_dir = PathBuf::from(
        request["invocation"]["envelope"]["execution_location"]["skill_directory"]
            .as_str()
            .ok_or("missing skill directory")?,
    );
    assert_eq!(
        actual_source_dir.canonicalize()?,
        source_dir.canonicalize()?
    );

    Ok(())
}

#[test]
fn native_skill_resolves_trusted_registry_ref() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-registry-ref");
    let registry_dir = publish_registry_echo_version(&root, "1.0.0", "# Echo\n", true)?;
    let output = trusted_registry_runx_command(&root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(2))?;
    let skill_dir = needs_agent_skill_directory(&output_json)?;
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("X.yaml").exists());
    assert!(skill_dir.to_string_lossy().contains("registry-skills"));
    assert!(skill_dir.to_string_lossy().contains("1.0.0"));

    Ok(())
}

#[test]
fn native_skill_registry_run_reports_provenance() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-registry-provenance");
    let registry_dir = publish_registry_echo_version(&root, "1.0.0", "# Echo\n", true)?;

    let json_output = trusted_registry_runx_command(&root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&json_output, Some(2))?;
    let provenance = output_json["registry_provenance"]
        .as_object()
        .ok_or("missing registry provenance")?;
    assert_eq!(provenance["skill_id"], "acme/echo");
    assert_eq!(provenance["version"], "1.0.0");
    assert_eq!(provenance["trust_tier"], "community");
    assert_eq!(provenance["registry_key_id"], TEST_MANIFEST_KEY_ID);
    assert_eq!(provenance["trust_state"], "trusted");
    assert_eq!(
        provenance["registry_source"],
        format!("local {}", registry_dir.display())
    );
    assert!(
        provenance["digest"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:"))
    );
    assert!(
        provenance["profile_digest"]
            .as_str()
            .is_some_and(|value| value.starts_with("sha256:"))
    );
    assert!(
        provenance["registry_source_fingerprint"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );

    let text_output = trusted_registry_runx_command(&root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
        ])
        .output()?;
    assert_eq!(text_output.status.code(), Some(2));
    let stdout = String::from_utf8(text_output.stdout)?;
    assert!(stdout.contains("registry:"));
    assert!(stdout.contains("  skill_id: acme/echo"));
    assert!(stdout.contains("  version: 1.0.0"));
    assert!(stdout.contains(&format!(
        "  registry_source: local {}",
        registry_dir.display()
    )));
    assert!(stdout.contains("  trust_tier: community"));
    assert!(stdout.contains("  registry_key_id: runx-registry-skill-test-key"));

    Ok(())
}

#[test]
fn native_skill_registry_run_reports_provenance_on_execution_error()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-registry-error-provenance");
    let registry_dir = publish_registry_echo_version(&root, "1.0.0", "# Echo\n", true)?;

    let json_output = trusted_registry_runx_command(&root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "missing-runner",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&json_output, Some(1))?;
    assert_eq!(output_json["status"], "failure");
    let provenance = output_json["registry_provenance"]
        .as_object()
        .ok_or("missing registry provenance")?;
    assert_eq!(provenance["skill_id"], "acme/echo");
    assert_eq!(provenance["version"], "1.0.0");
    assert_eq!(provenance["trust_state"], "trusted");

    Ok(())
}

#[test]
fn native_skill_resolves_registry_versions_side_by_side() -> Result<(), Box<dyn std::error::Error>>
{
    let root = crate::support::temp_root("runx-skill-registry-versions");
    let registry_dir = root.join("registry");
    publish_registry_echo_version_into(&root, &registry_dir, "1.0.0", "# Echo\n", true)?;
    publish_registry_echo_version_into(
        &root,
        &registry_dir,
        "1.1.0",
        "# Echo\n\nVersion two.\n",
        true,
    )?;

    let v1 = trusted_registry_runx_command(&root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    let v1_json = assert_json(&v1, Some(2))?;
    let v1_dir = needs_agent_skill_directory(&v1_json)?;

    let v2 = trusted_registry_runx_command(&root)?
        .args([
            "skill",
            "acme/echo@1.1.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    let v2_json = assert_json(&v2, Some(2))?;
    let v2_dir = needs_agent_skill_directory(&v2_json)?;

    assert_ne!(v1_dir, v2_dir);
    assert!(v1_dir.to_string_lossy().contains("1.0.0"));
    assert!(v2_dir.to_string_lossy().contains("1.1.0"));
    assert_eq!(
        fs::read_to_string(v1_dir.join("SKILL.md"))?,
        "---\nname: echo\n---\n# Echo\n"
    );
    assert_eq!(
        fs::read_to_string(v2_dir.join("SKILL.md"))?,
        "---\nname: echo\n---\n# Echo\n\nVersion two.\n"
    );

    Ok(())
}

#[test]
fn native_skill_rejects_untrusted_registry_refs() -> Result<(), Box<dyn std::error::Error>> {
    let unsigned_root = crate::support::temp_root("runx-skill-registry-unsigned");
    let unsigned_registry =
        publish_registry_echo_version(&unsigned_root, "1.0.0", "# Echo\n", false)?;
    let unsigned = trusted_registry_runx_command(&unsigned_root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            unsigned_registry.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    let unsigned_json = assert_json(&unsigned, Some(1))?;
    assert_eq!(unsigned_json["status"], "failure");
    assert_eq!(unsigned_json["error"]["code"], "skill_error");
    assert!(
        unsigned_json["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("registry signed manifest is required"))
    );
    assert!(!unsigned_root.join("home").join("registry-skills").exists());

    let mismatch_root = crate::support::temp_root("runx-skill-registry-digest-mismatch");
    let mismatch_registry =
        publish_registry_echo_version(&mismatch_root, "1.0.0", "# Echo\n", true)?;
    let mismatch = trusted_registry_runx_command(&mismatch_root)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            mismatch_registry.to_str().ok_or("non-utf8 registry dir")?,
            "--digest",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "--json",
        ])
        .output()?;
    let mismatch_json = assert_json(&mismatch, Some(1))?;
    assert_eq!(mismatch_json["status"], "failure");
    assert_eq!(mismatch_json["error"]["code"], "skill_error");
    assert!(
        mismatch_json["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("digest mismatch"))
    );
    assert!(!mismatch_root.join("home").join("registry-skills").exists());

    Ok(())
}

#[test]
fn native_skill_json_parse_failure_uses_failure_envelope() -> Result<(), Box<dyn std::error::Error>>
{
    let output = runx_command().args(["skill", "--json"]).output()?;

    let value = assert_json(&output, Some(64))?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "invalid_args");
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("runx skill requires a skill package path"))
    );

    Ok(())
}

#[test]
fn native_skill_rejects_legacy_answers_flag() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-reject-answers");
    let skill_dir = crate::support::write_agent_task_skill(&root)?;
    let answers_path = root.join("answers.json");
    fs::write(&answers_path, "{}")?;
    let output = runx_command()
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--answers",
            answers_path.to_str().ok_or("non-utf8 answers path")?,
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(64));
    assert!(
        String::from_utf8(output.stderr)?.contains("use `runx resume <run-id> <answers.json|->`")
    );
    assert_eq!(String::from_utf8(output.stdout)?, "");

    Ok(())
}

#[test]
fn native_skill_rejects_legacy_run_id_flag() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-reject-run-id");
    let skill_dir = crate::support::write_agent_task_skill(&root)?;
    let output = runx_command()
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--run-id",
            "issue-intake-run",
        ])
        .output()?;

    assert_eq!(output.status.code(), Some(64));
    assert!(
        String::from_utf8(output.stderr)?.contains("use `runx resume <run-id> <answers.json|->`")
    );
    assert_eq!(String::from_utf8(output.stdout)?, "");

    Ok(())
}

#[test]
fn native_skill_rejects_retired_receipt_options() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-reject-retired-receipt");
    let skill_dir = crate::support::write_agent_task_skill(&root)?;
    let receipt_dir = root.join("receipts");
    let retired_receipt = format!("--{}", "receipt");
    let retired_receipt_dir = format!("--{}", ["receipt", "Dir"].concat());
    let retired_receipt_dir_equals = format!(
        "{}={}",
        retired_receipt_dir,
        receipt_dir.to_str().ok_or("non-utf8 receipt dir")?
    );

    for args in [
        vec![
            "skill".to_owned(),
            skill_dir.to_str().ok_or("non-utf8 skill dir")?.to_owned(),
            retired_receipt,
            receipt_dir
                .to_str()
                .ok_or("non-utf8 receipt dir")?
                .to_owned(),
        ],
        vec![
            "skill".to_owned(),
            skill_dir.to_str().ok_or("non-utf8 skill dir")?.to_owned(),
            retired_receipt_dir,
            receipt_dir
                .to_str()
                .ok_or("non-utf8 receipt dir")?
                .to_owned(),
        ],
        vec![
            "skill".to_owned(),
            skill_dir.to_str().ok_or("non-utf8 skill dir")?.to_owned(),
            retired_receipt_dir_equals,
        ],
    ] {
        let output = runx_command().args(args).output()?;
        assert_eq!(output.status.code(), Some(64));
        assert!(String::from_utf8(output.stderr)?.contains("retired runx skill receipt option"));
        assert_eq!(String::from_utf8(output.stdout)?, "");
    }

    Ok(())
}

#[cfg(unix)]
#[test]
fn terminal_interrupt_exits_130_and_kills_the_active_skill_context()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-interrupt");
    let skill_dir = root.join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: interrupt-fixture\ndescription: Interrupt fixture.\n---\n\n# Interrupt fixture\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"skill: interrupt-fixture
version: "0.1.0"
runners:
  default:
    default: true
    type: cli-tool
    command: sh
    args:
      - ./run.sh
      - "{{started_path}}"
      - "{{sentinel_path}}"
    timeout_seconds: 30
    inputs:
      started_path:
        type: string
        required: true
      sentinel_path:
        type: string
        required: true
"#,
    )?;
    fs::write(
        skill_dir.join("run.sh"),
        r#"#!/bin/sh
set -eu
started_path=$1
sentinel_path=$2
(
  printf started > "$started_path"
  sleep 1
  printf survived > "$sentinel_path"
) &
sleep 30
"#,
    )?;
    let started_path = root.join("started");
    let sentinel_path = root.join("survived");
    fs::write(&started_path, "")?;
    fs::write(&sentinel_path, "")?;
    let started_input = format!("started-path={}", started_path.display());
    let sentinel_input = format!("sentinel-path={}", sentinel_path.display());
    let mut child = runx_command()
        .current_dir(&root)
        .env("RUNX_CWD", &root)
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--input",
            &started_input,
            "--input",
            &sentinel_input,
            "--json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let started_wait = Instant::now();
    while fs::read_to_string(&started_path).unwrap_or_default() != "started" {
        if started_wait.elapsed() >= Duration::from_secs(5) {
            let _killed = child.kill();
            return Err("skill child never reached its active context".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (status, _elapsed) = interrupt_child_and_wait(&mut child)?;
    assert_eq!(
        status.code(),
        Some(130),
        "runx exited from signal {:?}",
        std::os::unix::process::ExitStatusExt::signal(&status)
    );
    std::thread::sleep(Duration::from_millis(1_250));
    assert_eq!(
        fs::read_to_string(&sentinel_path).unwrap_or_default(),
        "",
        "active skill context survived the terminal interrupt"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn terminal_interrupt_kills_the_active_javascript_worker_before_the_watchdog()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-javascript-interrupt");
    let skill_dir = root.join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: javascript-interrupt-fixture\ndescription: JavaScript interrupt fixture.\n---\n\n# JavaScript interrupt fixture\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"skill: javascript-interrupt-fixture
version: "0.1.0"
runners:
  default:
    default: true
    type: javascript
    module: main.mjs
"#,
    )?;
    fs::write(
        skill_dir.join("main.mjs"),
        "export default function run() { return {}; }\n",
    )?;

    // `wc` blocks on the worker protocol pipe without producing a response.
    // Use an exact absolute binary path on both Linux and macOS.
    let wc = [Path::new("/usr/bin/wc"), Path::new("/bin/wc")]
        .into_iter()
        .find(|candidate| candidate.is_file())
        .ok_or("wc executable is unavailable")?;

    let mut child = runx_command()
        .current_dir(&root)
        .env("RUNX_CWD", &root)
        .env("RUNX_JS_WORKER_PATH", wc)
        .args([
            "skill",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    let child_started_at = Instant::now();
    loop {
        let process_table = Command::new("ps").args(["-axo", "ppid=,pid="]).output()?;
        let runx_pid = child.id();
        let worker_exists = String::from_utf8(process_table.stdout)?
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                Some((
                    fields.next()?.parse::<u32>().ok()?,
                    fields.next()?.parse::<u32>().ok()?,
                ))
            })
            .any(|(parent_pid, _process_id)| parent_pid == runx_pid);
        if worker_exists {
            break;
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "runx exited before the JavaScript worker reached its handshake: {status}"
            )
            .into());
        }
        if child_started_at.elapsed() >= Duration::from_secs(5) {
            let _killed = child.kill();
            return Err("the JavaScript worker did not reach its blocking handshake".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    let (status, elapsed) = interrupt_child_and_wait(&mut child)?;
    assert_eq!(
        status.code(),
        Some(130),
        "runx exited from signal {:?}",
        std::os::unix::process::ExitStatusExt::signal(&status)
    );
    assert!(
        elapsed < Duration::from_millis(1_500),
        "JavaScript context survived until the two-second interrupt watchdog"
    );
    Ok(())
}

#[cfg(unix)]
fn interrupt_child_and_wait(
    child: &mut std::process::Child,
) -> Result<(std::process::ExitStatus, Duration), Box<dyn std::error::Error>> {
    let child_pid = i32::try_from(child.id())?;
    let child_pid = rustix::process::Pid::from_raw(child_pid).ok_or("invalid runx child pid")?;
    let interrupted_at = Instant::now();
    rustix::process::kill_process(child_pid, rustix::process::Signal::INT)?;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok((status, interrupted_at.elapsed()));
        }
        if interrupted_at.elapsed() >= Duration::from_secs(5) {
            let _killed = child.kill();
            return Err("runx did not exit promptly after SIGINT".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn runx_command() -> Command {
    crate::support::isolated_runx_command_with_inherited_cwd("skill-test-key")
}

fn trusted_registry_runx_command(root: &Path) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = crate::support::signed_runx_command("skill-test-key");
    let key_pair = test_manifest_key_pair()?;
    command.env("RUNX_HOME", root.join("home"));
    command.env(
        runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV,
        base64::engine::general_purpose::STANDARD.encode(key_pair.public_key().as_ref()),
    );
    command.env(
        runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV,
        TEST_MANIFEST_KEY_ID,
    );
    command.env(
        runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV,
        "acme",
    );
    Ok(command)
}

fn assert_json(
    output: &std::process::Output,
    expected_status: Option<i32>,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    if let Some(expected_status) = expected_status {
        assert_eq!(
            output.status.code(),
            Some(expected_status),
            "stderr={}\nstdout={}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    assert!(
        output.status.success() || expected_status.is_some(),
        "status={:?}\nstderr={}\nstdout={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    crate::support::assert_json_stderr(&output.stderr)?;
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn write_operator_context_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("operator-context");
    let child_dir = skill_dir.join("nested-review");
    fs::create_dir_all(child_dir.join("context/review-rubric"))?;
    fs::create_dir_all(child_dir.join("tools/example/record"))?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: operator-context\n---\n# Operator Context Fixture\n",
    )?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: nested-review\n---\n# Nested Review Skill\n\nJudge the work against the supplied review-rubric context skill.\n",
    )?;
    fs::write(
        child_dir.join("context/review-rubric/SKILL.md"),
        "---\nname: review-rubric\nrunx:\n  category: context\n---\n# Review Rubric\n\nproduction bar from context skill\n",
    )?;
    fs::write(
        child_dir.join("tools/example/record/manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.record",
  "description": "Records a reviewed decision.",
  "source": {
    "type": "cli-tool",
    "command": "node",
    "args": ["record.mjs"],
    "input_mode": "stdin"
  },
  "inputs": {
    "decision": {
      "type": "string",
      "required": true
    }
  },
  "artifacts": {
    "named_emits": {
      "decision": "decision"
    }
  }
}
"#,
    )?;
    fs::write(
        child_dir.join("tools/example/record/record.mjs"),
        "process.stdout.write(JSON.stringify({ decision: 'recorded' }));\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: operator-context
runners:
  review:
    default: true
    type: graph
    graph:
      name: operator-context-review
      result_from: [review]
      steps:
        - id: review
          skill: ./nested-review
"#,
    )?;
    fs::write(
        child_dir.join("X.yaml"),
        r#"
skill: nested-review
runners:
  nested-review:
    default: true
    type: graph
    graph:
      name: nested-review
      result_from: [record]
      steps:
        - id: verdict
          run:
            type: agent-task
            agent: reviewer
            task: operator-context-review
            outputs:
              decision: string
          context_skills:
            - ./context/review-rubric
        - id: record
          tool: example.record
          context:
            decision: verdict.decision
"#,
    )?;
    Ok(skill_dir)
}

fn write_approval_graph_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("approval-graph");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: approval-graph\n---\n# Approval Graph\n",
    )?;
    fs::write(
        skill_dir.join("fail.mjs"),
        "export function fail() { throw new Error('expected terminal fault'); }\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: approval-graph
runners:
  approval-graph:
    default: true
    type: graph
    graph:
      name: approval-graph
      result_from: [fail]
      steps:
        - id: approve
          run:
            type: approval
          inputs:
            gate_id: approval-graph.local-development
            reason: approve the local development fixture
          artifacts:
            wrap_as: approval_decision
            packet: runx.approval.decision.v1
        - id: fail
          run:
            type: javascript
            module: fail.mjs
            export: fail
            outputs: { result: object }
"#,
    )?;
    Ok(skill_dir)
}

fn write_multi_runner_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("multi-runner");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: multi-runner\n---\n# Multi Runner\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: multi-runner
runners:
  first:
    default: true
    type: agent-task
    agent: builder
    task: first-task
    outputs:
      result: object
  second:
    type: agent-task
    agent: builder
    task: second-task
    outputs:
      result: object
"#,
    )?;
    Ok(skill_dir)
}

fn write_credential_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("credential-skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: credential-skill\n---\n# Credential Skill\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"
skill: credential-skill
credentials:
  example:
    provider: example
    audience: https://api.example.com
    auth:
      api_key:
        delivery:
          env: EXAMPLE_API_KEY
runners:
  status:
    default: true
    type: cli-tool
    command: example-status
    credential: example
"#,
    )?;
    Ok(skill_dir)
}

fn publish_registry_echo_version(
    root: &Path,
    version: &str,
    markdown_body: &str,
    signed: bool,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let registry_dir = root.join("registry");
    publish_registry_echo_version_into(root, &registry_dir, version, markdown_body, signed)?;
    Ok(registry_dir)
}

fn publish_registry_echo_version_into(
    root: &Path,
    registry_dir: &Path,
    version: &str,
    markdown_body: &str,
    signed: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let skill_dir = root.join(format!("skill-{version}"));
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        format!("---\nname: echo\n---\n{markdown_body}"),
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        include_str!("../../../fixtures/registry/install/echo-X.yaml"),
    )?;
    let publish = trusted_registry_runx_command(root)?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--owner",
            "acme",
            "--version",
            version,
            "--json",
        ])
        .output()?;
    assert!(
        publish.status.success(),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&publish.stderr),
        String::from_utf8_lossy(&publish.stdout)
    );
    if signed {
        sign_registry_version(registry_dir, version)?;
    }
    Ok(())
}

fn sign_registry_version(
    registry_dir: &Path,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let version_path = registry_dir
        .join("acme")
        .join("echo")
        .join(format!("{version}.json"));
    let mut version_record =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&version_path)?)?;
    version_record["signed_manifest"] = signed_manifest(&version_record)?;
    fs::write(
        version_path,
        format!("{}\n", serde_json::to_string_pretty(&version_record)?),
    )?;
    Ok(())
}

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
    Ok(json!({
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
                base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref())
            ),
        },
    }))
}

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

fn test_manifest_key_pair() -> Result<ring::signature::Ed25519KeyPair, io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&TEST_MANIFEST_SEED).map_err(|error| {
        io::Error::other(format!("static registry manifest seed rejected: {error:?}"))
    })
}

fn needs_agent_skill_directory(
    value: &serde_json::Value,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let request = pending_request_artifact(value)?;
    Ok(PathBuf::from(
        request["invocation"]["envelope"]["execution_location"]["skill_directory"]
            .as_str()
            .ok_or("missing skill directory")?,
    ))
}

fn pending_request_artifact(
    value: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let path = value["requests"][0]["artifact_ref"]["path"]
        .as_str()
        .ok_or("missing pending request artifact path")?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
