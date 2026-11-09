use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
#[cfg(unix)]
use std::time::{Duration, Instant};

use serde_json::json;

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
    let profile_path = skill_dir.join("run.yaml");
    let profile = fs::read_to_string(&profile_path)?;
    fs::write(
        &profile_path,
        profile
            .replace(
                "      thread_title:\n        type: string\n        optional: true\n",
                "      thread_title:\n        type: string\n        optional: true\n      severity:\n        type: string\n        optional: true\n",
            )
            .replace(
                "          thread_title: $input.thread_title\n",
                "          thread_title: $input.thread_title\n          severity: $input.severity\n",
            ),
    )?;
    let receipt_dir = root.join("receipts");

    let output = runx_command()
        .current_dir(&root)
        .args([
            "skill",
            "code-intake",
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--input",
            "thread_title=Docs bug",
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
    assert!(stderr.contains("Full context: add --diagnostics"));
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
            "--diagnostics",
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
fn workflow_action_approval_yields_one_operator_resolution()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-explicit-approval");
    fs::create_dir_all(&root)?;
    let receipt_dir = root.join(".runx/receipts");
    let approval_skill = write_approval_workflow_skill(&root)?;

    let workflow_run = crate::support::unsigned_runx_command_at(&root)
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
        workflow_run.status.code(),
        Some(2),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&workflow_run.stdout),
        String::from_utf8_lossy(&workflow_run.stderr)
    );
    let workflow_json = serde_json::from_slice::<serde_json::Value>(&workflow_run.stdout)?;
    assert_eq!(workflow_json["status"], "needs_approval");
    assert_eq!(workflow_json["requests"][0]["kind"], "approval");

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

    let run_id = workflow_json["run_id"].as_str().ok_or("missing run id")?;
    let request_id = workflow_json["requests"][0]["id"]
        .as_str()
        .ok_or("missing request id")?;
    let answers = root.join("decline.json");
    let read = crate::support::unsigned_runx_command_at(&root)
        .args(["history", run_id, "--json"])
        .output()?;
    fs::write(
        &answers,
        crate::support::bind_resume(
            &serde_json::from_slice(&read.stdout)?,
            serde_json::json!({"approvals": { request_id: false }}),
        )?
        .to_string(),
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
    assert_ne!(declined["outcome"], "completed");
    assert!(
        runx_runtime::run_record::paused_run(&std::collections::BTreeMap::new(), &root, run_id)?
            .is_none()
    );
    let receipt_id = declined["receipt_id"]
        .as_str()
        .ok_or("missing receipt id")?;
    let receipt_path =
        runx_runtime::LocalReceiptStore::new(&receipt_dir).receipt_path(receipt_id)?;
    fs::remove_file(&receipt_path)?;
    assert!(!receipt_path.exists());
    let replay = runx_runtime::orchestrator::LocalOrchestrator::default().run_skill_with_binding(
        &runx_runtime::orchestrator::SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: approval_skill,
            receipt_dir: Some(receipt_dir),
            run_id: Some(run_id.to_owned()),
            answers: None,
            inputs: std::collections::BTreeMap::new(),
            env: std::collections::BTreeMap::from([(
                "RUNX_HOME".to_owned(),
                root.join("home").to_string_lossy().into_owned(),
            )]),
            cwd: root,
            managed_agent: runx_runtime::ManagedAgentPolicy::HostDriven,
        },
        Some("approval-workflow"),
        None,
        None,
    )?;
    assert!(!replay.succeeded());
    assert_eq!(
        replay.receipt_ids.first().map(String::as_str),
        declined["receipt_id"].as_str()
    );
    assert!(receipt_path.exists());

    Ok(())
}

#[test]
fn a_method_reference_selects_a_non_default_method() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-method-ref");
    let skill_dir = write_multi_method_skill(&root)?;
    let receipt_dir = root.join("receipts");
    let skill = skill_dir.to_str().ok_or("non-utf8 skill dir")?;

    for reference in [
        format!("{skill}#second"),
        format!("{skill}/SKILL.md#second"),
    ] {
        let output = runx_command()
            .args([
                "skill",
                &reference,
                "--receipt-dir",
                receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
                "--json",
            ])
            .output()?;
        let output_json = assert_json(&output, Some(2))?;
        assert_eq!(output_json["requests"][0]["id"], "second", "{reference}");
    }

    // With no `#method`, the package's default_method runs.
    let output = runx_command()
        .args([
            "skill",
            skill,
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(assert_json(&output, Some(2))?["requests"][0]["id"], "first");

    // A method is named one way: a positional one is refused before anything runs.
    let output = runx_command()
        .args(["skill", skill, "second", "--json"])
        .output()?;
    let refused = assert_json(&output, Some(64))?;
    assert!(
        refused["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("<ref>#<method>")),
        "{refused}"
    );
    Ok(())
}

/// A package directory named with `#method` is that directory: its method
/// runs there and no registry is asked, even one holding a skill by the
/// directory's name. The run reaches nothing but loopback, and the registry
/// and API there record every request, so a lookup would show.
#[test]
fn a_package_directory_with_a_method_never_reaches_a_registry()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-method-ref-local");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    registry.root("multi-method", "acme/echo");
    let project = root.join("project");
    let skill_dir = write_multi_method_skill(&project)?;
    let run = || -> Result<std::process::Output, Box<dyn std::error::Error>> {
        let mut command = crate::support::unsigned_runx_command_with_inherited_cwd();
        crate::support::isolate_project(&mut command);
        crate::support::apply_fixture_signing(&mut command, "skill-test-key");
        Ok(command
            .current_dir(&project)
            .env("RUNX_HOME", root.join("home"))
            .env("RUNX_CWD", &project)
            .env("RUNX_REGISTRY_URL", &registry.url)
            .env("RUNX_PUBLIC_API_BASE_URL", &registry.url)
            .envs(registry.env()?)
            .args(["skill", "multi-method#second", "--json"])
            .output()?)
    };

    let output_json = assert_json(&run()?, Some(2))?;
    assert_eq!(output_json["requests"][0]["id"], "second");
    assert_eq!(
        needs_agent_skill_directory(&output_json)?.canonicalize()?,
        skill_dir.canonicalize()?
    );
    assert_eq!(registry.asked(), Vec::<String>::new());

    // Without the directory, the same reference is a name, and the registry
    // is asked for it.
    fs::remove_dir_all(&skill_dir)?;
    let output = run()?;
    assert!(!output.status.success());
    assert!(!registry.asked().is_empty());
    Ok(())
}

/// An inspection binds a pinned registry child the project holds, and
/// reports one it does not: unresolved, then installed, then fully bound.
#[test]
fn native_skill_inspect_binds_pinned_registry_dependencies_once_installed()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-inspect-registry-closure");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    let skill_dir = root.join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: parent fixture.\n---\n# Parent\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  default:
    result_from: [child]
    steps:
      - id: child
        skill: registry:acme/echo@1.0.0#default
"#,
    )?;
    let inspect = || -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let output = registry_runx_command(&root, &registry)?
            .current_dir(&root)
            .args([
                "skill",
                "inspect",
                skill_dir.to_str().ok_or("non-utf8 skill dir")?,
                "--json",
            ])
            .output()?;
        Ok(assert_json(&output, Some(0))?["execution_closure"].clone())
    };

    let unheld = inspect()?;
    assert_eq!(unheld["fully_bound"], false);
    assert_eq!(
        unheld["unresolved_skill_edges"],
        json!(["registry:acme/echo@1.0.0#default"])
    );

    let install = registry_runx_command(&root, &registry)?
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_json(&install, Some(0))?;

    let held = inspect()?;
    assert_eq!(held["fully_bound"], true);
    assert_eq!(held["unresolved_skill_edges"], json!([]));
    assert_eq!(
        held["package_bindings"]
            .as_array()
            .ok_or("package bindings are missing")?
            .len(),
        2,
    );
    Ok(())
}

#[test]
fn native_skill_inspect_reports_a_pin_the_registry_lacks_and_a_run_refuses_it()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-inspect-registry-absent-pin");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    let skill_dir = root.join("parent");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: parent\ndescription: parent fixture.\n---\n# Parent\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  default:
    result_from: [child]
    steps:
      - id: child
        skill: registry:acme/echo@2.0.0#default
"#,
    )?;
    let skill = skill_dir.to_str().ok_or("non-utf8 skill dir")?;

    // An inspection reports the pin the project does not hold.
    let output = registry_runx_command(&root, &registry)?
        .current_dir(&root)
        .args(["skill", "inspect", skill, "--json"])
        .output()?;
    let inspected = assert_json(&output, Some(0))?;
    let closure = &inspected["execution_closure"];
    assert_eq!(closure["fully_bound"], false);
    assert_eq!(
        closure["unresolved_skill_edges"],
        json!(["registry:acme/echo@2.0.0#default"])
    );

    // A run cannot proceed without it, and the registry does not publish it.
    let output = registry_runx_command(&root, &registry)?
        .current_dir(&root)
        .args(["skill", skill, "--registry", &registry.url, "--json"])
        .output()?;
    let failed = assert_json(&output, Some(1))?;
    let message = failed["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("fetching registry:acme/echo@2.0.0: registry skill not found"),
        "{message}"
    );
    Ok(())
}

#[test]
fn native_skill_exported_shim_resolves_to_source_skill() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-exported-shim");
    let source_dir = crate::support::write_agent_task_skill(&root.join("source with spaces"))?;
    let package_digest = runx_runtime::load_validated_skill_package(&source_dir)?
        .package
        .package_digest;
    let shim_dir = root.join("claude").join("code-intake");
    fs::create_dir_all(&shim_dir)?;
    fs::write(
        shim_dir.join("SKILL.md"),
        format!(
            "---\nname: code-intake\ndescription: code-intake fixture.\n---\n# code-intake\n<!-- runx-export:claude source={} package-digest={} - generated, do not edit -->\n",
            source_dir.display(),
            package_digest
        ),
    )?;

    let output = runx_command()
        .args([
            "skill",
            shim_dir.to_str().ok_or("non-utf8 shim dir")?,
            "-i",
            "thread_title=Docs bug",
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
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    let output = registry_runx_command(&root, &registry)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(2))?;
    let skill_dir = needs_agent_skill_directory(&output_json)?;
    assert!(skill_dir.join("SKILL.md").exists());
    assert!(skill_dir.join("run.yaml").exists());
    // runx adds it to the project directory, never beside the source.
    assert!(skill_dir.ends_with(".runx/added-skills/acme/echo/1.0.0"));

    Ok(())
}

#[test]
fn native_skill_registry_run_reports_provenance() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-registry-provenance");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;

    let json_output = registry_runx_command(&root, &registry)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&json_output, Some(2))?;
    let provenance = output_json["entry_provenance"]
        .as_object()
        .ok_or("missing registry provenance")?;
    assert_eq!(provenance["skill_id"], "acme/echo");
    assert_eq!(provenance["version"], "1.0.0");
    assert_eq!(provenance["trust_tier"], "community");
    assert_eq!(
        provenance["registry_key_id"],
        crate::registry_fixture::KEY_ID
    );
    assert_eq!(provenance["trust_state"], "trusted");
    assert_eq!(
        provenance["registry_source"],
        format!("remote {}", registry.url)
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
        provenance["registry_source_digest"]
            .as_str()
            .is_some_and(|value| value.len() == 16)
    );

    let text_output = registry_runx_command(&root, &registry)?
        .args(["skill", "acme/echo@1.0.0", "--registry", &registry.url])
        .output()?;
    assert_eq!(text_output.status.code(), Some(2));
    let stdout = String::from_utf8(text_output.stdout)?;
    assert!(stdout.contains("entry:\n  kind: registry\n"), "{stdout}");
    assert!(stdout.contains("  skill_id: acme/echo"));
    assert!(stdout.contains("  version: 1.0.0"));
    assert!(stdout.contains(&format!("  registry_source: remote {}", registry.url)));
    assert!(stdout.contains("  trust_tier: community"));
    assert!(stdout.contains(&format!(
        "  registry_key_id: {}",
        crate::registry_fixture::KEY_ID
    )));

    Ok(())
}

#[test]
fn native_skill_registry_run_reports_provenance_on_execution_error()
-> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-registry-error-provenance");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;

    let json_output = registry_runx_command(&root, &registry)?
        .args([
            "skill",
            "acme/echo@1.0.0#missing-method",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&json_output, Some(1))?;
    assert_eq!(output_json["status"], "failure");
    let provenance = output_json["entry_provenance"]
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
    let registry = publish_echo(&[("1.0.0", "# Echo\n"), ("1.1.0", "# Echo\n\nVersion two.\n")])?;
    let run = |reference: &str| -> Result<PathBuf, Box<dyn std::error::Error>> {
        let output = registry_runx_command(&root, &registry)?
            .args(["skill", reference, "--registry", &registry.url, "--json"])
            .output()?;
        needs_agent_skill_directory(&assert_json(&output, Some(2))?)
    };

    let v1_dir = run("acme/echo@1.0.0")?;
    let v2_dir = run("acme/echo@1.1.0")?;

    assert_ne!(v1_dir, v2_dir);
    assert!(v1_dir.to_string_lossy().contains("1.0.0"));
    assert!(v2_dir.to_string_lossy().contains("1.1.0"));
    assert_eq!(
        fs::read_to_string(v1_dir.join("SKILL.md"))?,
        format!("---\nname: echo\ndescription: echo fixture.\n---\n# Echo\n{ECHO_TASK}")
    );
    assert_eq!(
        fs::read_to_string(v2_dir.join("SKILL.md"))?,
        format!(
            "---\nname: echo\ndescription: echo fixture.\n---\n# Echo\n\nVersion two.\n{ECHO_TASK}"
        )
    );

    Ok(())
}

/// A graph's registry step arrives when the graph runs: `runx skill` fetches
/// it as it fetches a top-level ref, into the project's skills, and the run
/// resolves the step from there.
#[test]
fn a_workflow_fetches_its_registry_steps_before_it_runs() -> Result<(), Box<dyn std::error::Error>>
{
    let root = crate::support::temp_root("runx-skill-registry-nested");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    let graph = root.join("project").join("nested");
    fs::create_dir_all(&graph)?;
    fs::write(
        graph.join("SKILL.md"),
        "---\nname: nested\ndescription: Run the echo registry skill as a step.\n---\n\nRun echo as a step.\n",
    )?;
    fs::write(
        graph.join("run.yaml"),
        "version: \"0.1.0\"\nminimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: echo\n        skill: registry:acme/echo@1.0.0\n",
    )?;

    let run = || -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let output = registry_runx_command(&root, &registry)?
            .args([
                "skill",
                graph.to_str().ok_or("non-utf8 workflow dir")?,
                "--registry",
                &registry.url,
                "--json",
            ])
            .output()?;
        assert_json(&output, Some(2))
    };
    let installed = crate::support::added_skills().join("acme/echo/1.0.0");
    let first = run()?;
    assert_eq!(
        needs_agent_skill_directory(&first)?.canonicalize()?,
        installed.canonicalize()?
    );

    // Held, it is not fetched again: the run resolves the installed version
    // and asks the registry nothing.
    let asked = registry.asked().len();
    let second = run()?;
    assert_eq!(
        needs_agent_skill_directory(&second)?.canonicalize()?,
        installed.canonicalize()?
    );
    assert_eq!(registry.asked().len(), asked, "{:?}", registry.asked());
    Ok(())
}

/// A registry context skill arrives the same way: fetched into the project's
/// added skills before the run, and given to the turn from there.
#[test]
fn a_registry_context_skill_is_fetched_before_the_run() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-registry-context");
    let registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    let skill = root.join("project").join("with-context");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: with-context\ndescription: Write with the echo skill as context.\n---\n\n## Agent task: write\n\nWrite with context.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "version: \"0.1.0\"\nminimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: write\n        agent: write\n        outputs:\n          result: {type: object}\n        context:\n          skills: [registry:acme/echo@1.0.0]\n",
    )?;

    let output = registry_runx_command(&root, &registry)?
        .args([
            "skill",
            skill.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(2))?;
    let request = pending_request_artifact(&output_json)?;
    let context = request["invocation"]["envelope"]["current_context"]
        .as_array()
        .ok_or("the turn has no current context")?;
    let echo = context
        .iter()
        .find(|entry| entry["data"]["skill"] == "registry:acme/echo@1.0.0")
        .ok_or("the turn's context omits the registry skill")?;
    // It keeps the registry provenance its signature states; its tier is a
    // statement apart, read only when a decision needs it (D30).
    assert_eq!(echo["data"]["source"], "runx-registry", "{echo}");
    assert_eq!(echo["data"]["skill_id"], "acme/echo", "{echo}");
    assert_eq!(echo["data"]["version"], "1.0.0", "{echo}");
    assert!(echo["data"].get("trust_tier").is_none(), "{echo}");
    assert!(
        echo["data"]["digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "{echo}"
    );
    assert!(
        crate::support::added_skills()
            .join("acme/echo/1.0.0/SKILL.md")
            .is_file()
    );
    Ok(())
}

#[test]
fn native_skill_rejects_untrusted_registry_refs() -> Result<(), Box<dyn std::error::Error>> {
    let unsigned_root = crate::support::temp_root("runx-skill-registry-unsigned");
    let unsigned_registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    unsigned_registry.tamper("acme/echo", "1.0.0", |record| {
        if let Some(record) = record.as_object_mut() {
            record.remove("signed_manifest");
        }
    })?;
    let unsigned = registry_runx_command(&unsigned_root, &unsigned_registry)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            &unsigned_registry.url,
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
    assert!(!crate::support::added_skills().join("acme/echo").exists());

    let mismatch_root = crate::support::temp_root("runx-skill-registry-digest-mismatch");
    let mismatch_registry = publish_echo(&[("1.0.0", "# Echo\n")])?;
    let mismatch = registry_runx_command(&mismatch_root, &mismatch_registry)?
        .args([
            "skill",
            "acme/echo@1.0.0",
            "--registry",
            &mismatch_registry.url,
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
    assert!(!crate::support::added_skills().join("acme/echo").exists());

    Ok(())
}

/// A tool whose program is missing fails its step once: the failure names
/// the program, and the run's tree is the graph and that one step.
#[cfg(unix)]
#[test]
fn a_missing_program_fails_its_step_once_and_is_named() -> Result<(), Box<dyn std::error::Error>> {
    let root = crate::support::temp_root("runx-skill-missing-program");
    let skill_dir = root.join("missing-program");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: missing-program\ndescription: Run a program that is not installed.\n---\n\nRun a program that is not installed.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "version: \"0.1.0\"\nminimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: run\n        tool: {command: runx-test-program-not-installed}\n        outputs:\n          message: {type: string}\n",
    )?;
    let receipt_dir = root.join("receipts");

    let output = runx_command()
        .current_dir(&root)
        .args([
            "skill",
            "./missing-program",
            "--receipt-dir",
            receipt_dir.to_str().ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .output()?;
    let output_json = assert_json(&output, Some(1))?;
    assert_eq!(output_json["outcome"], "failed", "{output_json}");
    assert!(
        output_json["result"]["message"]
            .as_str()
            .is_some_and(|message| message
                .contains("runx-test-program-not-installed is not installed or not on PATH")),
        "{output_json}"
    );

    let mut receipts = Vec::new();
    for entry in fs::read_dir(&receipt_dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if name.starts_with("sha256-") && name.ends_with(".json") {
            receipts.push(serde_json::from_slice::<serde_json::Value>(&fs::read(
                &path,
            )?)?);
        }
    }
    let kinds = |kind: &str| {
        receipts
            .iter()
            .filter(|receipt| receipt["subject"]["kind"] == kind)
            .collect::<Vec<_>>()
    };
    let (graphs, steps) = (kinds("graph"), kinds("step"));
    assert_eq!((graphs.len(), steps.len()), (1, 1), "{receipts:?}");
    let step_id = steps[0]["id"].as_str().ok_or("step receipt has no id")?;
    let children = graphs[0]["lineage"]["children"]
        .as_array()
        .ok_or("workflow receipt has no children")?;
    assert_eq!(children.len(), 1, "{children:?}");
    assert_eq!(children[0]["uri"], format!("runx:receipt:{step_id}"));
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
        skill_dir.join("run.yaml"),
        r#"version: "0.1.0"
minimum_runx: 0.12.0
methods:
  default:
    inputs:
      started_path:
        type: string
      sentinel_path:
        type: string
    steps:
      - id: default
        tool: {command: sh, args: [./run.sh, "{{started_path}}", "{{sentinel_path}}"], timeout_seconds: 30}
        inputs:
          started_path: $input.started_path
          sentinel_path: $input.sentinel_path
        outputs:
          completed: {type: boolean}
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
    let started_input = format!("started_path={}", started_path.display());
    let sentinel_input = format!("sentinel_path={}", sentinel_path.display());
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
        skill_dir.join("run.yaml"),
        r#"version: "0.1.0"
minimum_runx: 0.12.0
methods:
  default:
    steps:
      - id: default
        tool: {module: main.mjs}
        outputs:
          completed: {type: boolean}
"#,
    )?;
    fs::write(
        skill_dir.join("main.mjs"),
        "export default function run() { return { completed: true }; }\n",
    )?;

    // `wc` blocks on the worker protocol pipe without producing a response.
    // The worker path is its exact location on the inherited PATH.
    let wc = crate::support::find_program("wc").ok_or("wc executable is unavailable")?;

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

/// `runx` with its home and project under `root`, trusting `registry`'s key.
fn registry_runx_command(
    root: &Path,
    registry: &crate::registry_fixture::LoopbackRegistry,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = crate::support::signed_runx_command("skill-test-key");
    command.env("RUNX_HOME", root.join("home"));
    // A fetched skill is added to the project's added skills.
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    command.env("RUNX_CWD", &project);
    command.envs(registry.env()?);
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
        "---\nname: operator-context\ndescription: operator-context fixture.\n---\n# Operator Context Fixture\n",
    )?;
    fs::write(
        child_dir.join("SKILL.md"),
        "---\nname: nested-review\ndescription: nested-review fixture.\n---\n# Nested Review Skill\n\n## Agent task: operator-context-review\n\nJudge the work against the supplied review-rubric context skill.\n",
    )?;
    fs::write(
        child_dir.join("context/review-rubric/SKILL.md"),
        "---\nname: review-rubric\ndescription: Review rubric context skill.\n---\n# Review Rubric\n\nproduction bar from context skill\n",
    )?;
    fs::write(
        child_dir.join("tools/example/record/manifest.json"),
        r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.record",
  "description": "Records a reviewed decision.",
  "source": {
    "command": "node",
    "args": ["record.mjs"],
    "inputs_via": "stdin"
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
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  review:
    result_from: [review]
    steps:
      - id: review
        skill: ./nested-review
"#,
    )?;
    fs::write(
        child_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  nested-review:
    result_from: [record]
    steps:
      - id: verdict
        agent: operator-context-review
        outputs:
          decision: {type: string}
        context:
          skills:
            - ./context/review-rubric
      - id: record
        tool: example.record
        inputs:
          decision: $steps.verdict.decision
"#,
    )?;
    Ok(skill_dir)
}

fn write_approval_workflow_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("approval-workflow");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: approval-workflow\ndescription: approval-workflow fixture.\n---\n# Approval Graph\n",
    )?;
    fs::write(
        skill_dir.join("fail.mjs"),
        "export function fail() { throw new Error('expected terminal fault'); }\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
methods:
  approval-workflow:
    result_from: [fail]
    steps:
      - id: approve
        ask: {gate: approval-workflow.local-development, reason: approve the local development fixture}
      - id: fail
        tool: {module: fail.mjs, export: fail}
        outputs: {result: {type: object}}
"#,
    )?;
    Ok(skill_dir)
}

fn write_multi_method_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("multi-method");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: multi-method\ndescription: multi-method fixture.\n---\n# Multi Method\n\n## Agent task: first-task\n\nDo the first task.\n\n## Agent task: second-task\n\nDo the second task.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
minimum_runx: 0.12.0
default_method: first
methods:
  first:
    steps:
      - id: first
        agent: first-task
        outputs:
          result: {type: object}
  second:
    steps:
      - id: second
        agent: second-task
        outputs:
          result: {type: object}
"#,
    )?;
    Ok(skill_dir)
}

/// The echo fixture's agent task, which every published manual carries.
const ECHO_TASK: &str = "\n## Agent task: echo\n\nEcho the input as the result.\n";

/// A loopback registry publishing the echo fixture as acme's at each
/// version, with each manual body.
fn publish_echo(
    versions: &[(&str, &str)],
) -> Result<crate::registry_fixture::LoopbackRegistry, Box<dyn std::error::Error>> {
    let registry = crate::registry_fixture::LoopbackRegistry::serve()?;
    for (version, body) in versions {
        let markdown =
            format!("---\nname: echo\ndescription: echo fixture.\n---\n{body}{ECHO_TASK}");
        registry.publish(crate::registry_fixture::Publication::new(
            "acme",
            version,
            &markdown,
            Some(include_str!(
                "../../../fixtures/registry/install/echo/run.yaml"
            )),
        ))?;
    }
    Ok(registry)
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
    let path = value["requests"][0]["artifact"]["path"]
        .as_str()
        .ok_or("missing pending request artifact path")?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
