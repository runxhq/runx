use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn skill_author_journey_discovers_harnesses_runs_verifies_and_reads_history() -> TestResult {
    let root = crate::support::temp_root("runx-operator-author-journey");
    let skills_dir = root.join("skills");
    let skill_dir = skills_dir.join("digest-note");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        crate::new_skill_authoring::digest_note_manual(),
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        crate::new_skill_authoring::digest_note_manifest(),
    )?;

    let list = command(&root)
        .args(["list", "skills", "--ok-only", "--json"])
        .output()?;
    let list = assert_json(&list, 0)?;
    let listed = list["items"]
        .as_array()
        .ok_or("list items must be an array")?
        .iter()
        .find(|item| item["name"] == "digest-note")
        .ok_or("authored skill was not discoverable")?;
    assert_eq!(listed["status"], "ok");

    let inspect = command(&root)
        .arg("skill")
        .arg("inspect")
        .arg(&skill_dir)
        .arg("--json")
        .output()?;
    let inspect = assert_json(&inspect, 0)?;
    assert_eq!(inspect["status"], "ok");
    assert_eq!(inspect["readiness"]["status"], "ready");
    assert_eq!(inspect["capabilities"]["execution"], "read");
    assert_eq!(inspect["capabilities"]["completion"], "runtime_receipt");
    assert!(
        inspect["runner"]["inputs"]
            .as_array()
            .is_some_and(|inputs| inputs.iter().any(|input| input["name"] == "note"))
    );

    let harness = command(&root)
        .arg("harness")
        .arg(&skill_dir)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let harness = assert_json(&harness, 0)?;
    assert_eq!(harness["status"], "passed");
    assert_eq!(harness["case_count"], 1);

    let run = command(&root)
        .arg("skill")
        .arg(&skill_dir)
        .args(["--input", "note=hello"])
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json"])
        .output()?;
    let run = assert_json(&run, 0)?;
    assert_eq!(run["status"], "sealed");
    let receipt_id = json_string(&run, "receipt_id")?;
    let run_id = json_string(&run, "run_id")?;

    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    assert_run_sealed_with_local_receipt(&root, &receipt_dir, run_id, receipt_id)?;

    Ok(())
}

#[test]
fn agent_handoff_journey_pauses_recovers_resumes_verifies_and_clears_history() -> TestResult {
    let root = crate::support::temp_root("runx-operator-agent-handoff-journey");
    let skill_dir = crate::support::write_agent_task_skill(&root.join("skills"))?;
    let receipt_dir = root.join(".runx/receipts");

    let pause = command(&root)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["-i", "thread_title=Docs bug"])
        .output()?;
    assert_exit(&pause, 2)?;
    let pause_text = String::from_utf8(pause.stdout)?;
    assert!(pause_text.contains("status: needs_agent"));
    assert!(pause_text.contains("pending_requests: 1"));
    assert!(pause_text.contains("intake"));
    assert!(!pause_text.contains("<answers.json>"));
    assert!(!pause_text.trim_start().starts_with('{'));
    let run_id = pause_text
        .lines()
        .find_map(|line| line.strip_prefix("run_id: "))
        .ok_or("pending run omitted run_id")?
        .to_owned();
    assert!(run_id.starts_with("run_code-intake_"), "{run_id}");
    assert!(pause_text.contains(&format!("runx resume {run_id} -")));

    assert!(run_is_paused(&run_read(&root, &run_id)?));

    let malformed_answers = root.join("malformed-answers.json");
    fs::write(
        &malformed_answers,
        crate::support::bind_resume(
            &run_read(&root, &run_id)?,
            serde_json::json!({
                "answers": {
                    "intake": {
                        "intake_report": "not-an-object"
                    }
                }
            }),
        )?
        .to_string(),
    )?;
    let malformed = command(&root)
        .arg("resume")
        .arg(&run_id)
        .arg(&malformed_answers)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let malformed = assert_json(&malformed, 1)?;
    assert_eq!(malformed["status"], "failure");

    let answers = root.join("answers.json");
    fs::write(
        &answers,
        crate::support::bind_resume(
            &run_read(&root, &run_id)?,
            serde_json::json!({
                "answers": {
                    "intake": {
                        "intake_report": {
                            "summary": "Docs bug is bounded."
                        }
                    }
                }
            }),
        )?
        .to_string(),
    )?;
    let resume = command(&root)
        .arg("resume")
        .arg(&run_id)
        .arg(&answers)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let resume = assert_json(&resume, 0)?;
    assert_eq!(resume["status"], "sealed");
    assert_eq!(resume["schema"], "runx.skill_run.v1");
    assert_eq!(resume["run_id"], run_id);
    assert_eq!(resume["closure"]["disposition"], "closed");
    assert!(resume.get("receipt").is_none());
    assert!(resume.get("execution").is_none());
    let receipt_id = json_string(&resume, "receipt_id")?;

    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    assert_run_sealed_with_local_receipt(&root, &receipt_dir, &run_id, receipt_id)?;

    Ok(())
}

#[test]
fn declined_and_superseded_agent_runs_preserve_receipts_and_exit_nonzero() -> TestResult {
    let root = crate::support::temp_root("runx-operator-terminal-disposition-journey");

    for disposition in ["declined", "superseded"] {
        let case_root = root.join(disposition);
        let skill_dir = crate::support::write_agent_task_skill(&case_root.join("skills"))?;
        let receipt_dir = case_root.join(".runx/receipts");

        let pause = command(&case_root)
            .arg("skill")
            .arg(&skill_dir)
            .arg("--receipt-dir")
            .arg(&receipt_dir)
            .args(["--json"])
            .output()?;
        let pause = assert_json(&pause, 2)?;
        let run_id = json_string(&pause, "run_id")?;
        let request_id = pause["requests"]
            .as_array()
            .and_then(|requests| requests.first())
            .and_then(|request| request["id"].as_str())
            .ok_or("pending run omitted its request id")?;
        let answers = case_root.join("answers.json");
        fs::write(
            &answers,
            crate::support::bind_resume(
                &run_read(&case_root, run_id)?,
                serde_json::json!({
                    "answers": {
                        request_id: {
                            "intake_report": {
                                "summary": format!("Operator chose {disposition}.")
                            },
                            "closure": {
                                "disposition": disposition
                            }
                        }
                    }
                }),
            )?
            .to_string(),
        )?;

        let resume = command(&case_root)
            .arg("resume")
            .arg(run_id)
            .arg(&answers)
            .arg("--receipt-dir")
            .arg(&receipt_dir)
            .arg("--json")
            .output()?;
        let resume = assert_json(&resume, 1)?;
        assert_eq!(resume["status"], "sealed");
        assert_eq!(resume["closure"]["disposition"], disposition);
        assert!(resume.get("receipt").is_none());
        let receipt_id = json_string(&resume, "receipt_id")?;
        let run_id = json_string(&resume, "run_id")?;
        assert_receipt_verifies(&case_root, &receipt_dir, receipt_id)?;
        assert_run_sealed_with_local_receipt(&case_root, &receipt_dir, run_id, receipt_id)?;
    }

    Ok(())
}

#[test]
fn stdin_resume_reuses_run_and_rejects_digest_drift() -> TestResult {
    let root = crate::support::temp_root("runx-operator-stdin-resume-journey");
    let skill_dir = crate::support::write_agent_task_skill(&root.join("skills"))?;
    let receipt_dir = root.join(".runx/receipts");

    let pause = command(&root)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json"])
        .output()?;
    let pause = assert_json(&pause, 2)?;
    assert_eq!(pause["status"], "needs_agent");
    assert_eq!(pause["outcome"], "deferred");
    assert!(pause.get("context").is_none());
    let run_id = json_string(&pause, "run_id")?;
    let request = pause["requests"]
        .as_array()
        .and_then(|requests| requests.first())
        .ok_or("pending run omitted request summary")?;
    let request_id = json_string(request, "id")?;
    // The summary names the request artifact by its content digest.
    assert_eq!(
        json_string(request, "request_digest")?,
        request["artifact_ref"]["digest"]
            .as_str()
            .ok_or("request summary omitted artifact digest")?
    );
    let request_path = request["artifact_ref"]["path"]
        .as_str()
        .ok_or("request summary omitted artifact path")?;
    let request_path = if Path::new(request_path).is_absolute() {
        Path::new(request_path).to_path_buf()
    } else {
        root.join(request_path)
    };
    assert!(
        request_path
            .canonicalize()?
            .starts_with(root.join(".runx").canonicalize()?)
    );
    assert!(request_path.is_file());

    for (flag, value) in [
        ("--package-digest", "sha256:wrong-package"),
        ("--execution-closure-digest", "sha256:wrong-closure"),
    ] {
        let rejected = command(&root)
            .arg("resume")
            .arg(run_id)
            .arg("-")
            .arg(flag)
            .arg(value)
            .arg("--receipt-dir")
            .arg(&receipt_dir)
            .arg("--json")
            .output()?;
        let rejected = assert_json(&rejected, 1)?;
        assert_eq!(rejected["status"], "failure");
    }

    assert!(run_is_paused(&run_read(&root, run_id)?));
    let answer = crate::support::bind_resume(
        &run_read(&root, run_id)?,
        serde_json::json!({
            "answers": {
                (request_id): {
                    "intake_report": { "summary": "Docs bug is bounded." }
                }
            }
        }),
    )?
    .to_string();

    let mut resume = command(&root);
    resume
        .arg("resume")
        .arg(run_id)
        .arg("-")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json");
    let resume = output_with_stdin(&mut resume, &answer)?;
    let resume = assert_json(&resume, 0)?;
    assert_eq!(resume["status"], "sealed");
    assert_eq!(resume["outcome"], "completed");
    assert_eq!(resume["run_id"], run_id);
    // Answers read from stdin go to the run, never to a file.
    assert!(!root.join(".runx/continuations").exists());

    Ok(())
}

#[test]
fn nested_graph_resume_preserves_completed_effects() -> TestResult {
    for topology in ["nested", "deep"] {
        let root = crate::support::temp_root(&format!("runx-{topology}-resume-effects"));
        let skill_dir = root.join("skills/nested-resume");
        let receipt_dir = root.join(".runx/receipts");
        fs::create_dir_all(&skill_dir)?;
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: nested-resume\ndescription: Preserve completed child steps across host turns.\n---\n# Nested resume\nReturn a bounded decision for each review.\n",
        )?;
        let mut manifest = r#"skill: nested-resume
runners:
  run:
    default: true
    result_from: [child]
    steps:
      - id: child
        skill: .
        runner: child
  child:
    result_from: [second-review]
    steps:
      - id: first-write
        tool: fs.write
        scopes: [fs.write]
        inputs:
          path: first.txt
          contents: executed once
      - id: first-review
        agent: { role: reviewer, task: first-review }
        outputs: { decision: object }
        inputs:
          written: $steps.first-write.file_write
      - id: second-write
        tool: fs.write
        scopes: [fs.write]
        inputs:
          path: second.txt
          contents: executed once
      - id: second-review
        agent: { role: reviewer, task: second-review }
        outputs: { decision: object }
        inputs:
          written: $steps.second-write.file_write
          prior: $steps.first-review.decision
"#
        .to_owned();
        if topology == "deep" {
            manifest = manifest.replacen("        runner: child", "        runner: middle", 1);
            manifest.push_str(
                r#"  middle:
    result_from: [inner]
    steps:
      - id: inner
        skill: .
        runner: child
"#,
            );
        }
        fs::write(skill_dir.join("run.yaml"), manifest)?;
        let initial = command(&root)
            .arg("skill")
            .arg(&skill_dir)
            .arg("--receipt-dir")
            .arg(&receipt_dir)
            .arg("--json")
            .output()?;
        let mut paused = assert_json(&initial, 2)?;
        let run_id = json_string(&paused, "run_id")?.to_owned();

        for (index, marker) in ["first.txt", "second.txt"].into_iter().enumerate() {
            assert_eq!(paused["status"], "needs_agent");
            assert_eq!(fs::read_to_string(root.join(marker))?, "executed once");
            // A replay would overwrite this external observation of the first effect.
            fs::write(root.join(marker), "observed after pause")?;
            let request = &paused["requests"][0];
            let request_id = json_string(request, "id")?;
            let answers = crate::support::bind_resume(
                &run_read(&root, &run_id)?,
                serde_json::json!({
                    "answers": { (request_id): { "decision": { "accepted": true } } }
                }),
            )?;
            let mut resume = command(&root);
            resume
                .arg("resume")
                .arg(&run_id)
                .arg("-")
                .arg("--receipt-dir")
                .arg(&receipt_dir)
                .arg("--json");
            let resumed = output_with_stdin(&mut resume, &answers.to_string())?;
            paused = assert_json(&resumed, if index == 0 { 2 } else { 0 })?;
            assert_eq!(paused["run_id"], run_id);
            assert_eq!(
                fs::read_to_string(root.join(marker))?,
                "observed after pause"
            );
            assert_eq!(
                fs::read_to_string(root.join("first.txt"))?,
                "observed after pause"
            );
        }
        assert_eq!(paused["status"], "sealed");
        assert_eq!(paused["outcome"], "completed");
        assert_eq!(paused["result"]["decision"]["accepted"], true);
        assert_receipt_verifies(&root, &receipt_dir, json_string(&paused, "receipt_id")?)?;
    }
    Ok(())
}

#[test]
fn compact_skill_output_externalizes_oversized_intermediate_context() -> TestResult {
    let root = crate::support::temp_root("runx-operator-compact-output-journey");
    let skill_dir = root.join("skills/compact-output");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: compact-output\ndescription: Proves compact graph output.\n---\n# Compact Output\n\nProduce one small terminal packet after a large intermediate value.\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"skill: compact-output
version: "0.1.0"
runners:
  run:
    default: true
    inputs:
      payload:
        type: string
        required: true
    result_from: [finish]
    steps:
      - id: expand
        inputs:
          payload: "$input.payload"
        tool: { module: compact-output.mjs, export: expand }
        outputs:
          intermediate: object
      - id: finish
        inputs:
          intermediate: $steps.expand.intermediate
        tool: { module: compact-output.mjs, export: finish }
        outputs:
          result: object
"#,
    )?;
    fs::write(
        skill_dir.join("compact-output.mjs"),
        r#"export function expand(inputs) {
  return { intermediate: { body: String(inputs.payload || "") } };
}
export function finish() {
  return { result: { schema: "runx.compact_output.result.v1", summary: "complete" } };
}
"#,
    )?;
    let inputs_path = root.join("inputs.json");
    fs::write(
        &inputs_path,
        serde_json::json!({ "payload": "x".repeat(96 * 1024) }).to_string(),
    )?;

    let compact = command(&root)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--inputs")
        .arg("inputs.json")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    assert_exit(&compact, 0)?;
    assert!(compact.stdout.len() <= 16 * 1024);
    let compact: Value = serde_json::from_slice(&compact.stdout)?;
    assert_eq!(compact["status"], "sealed");
    assert_eq!(compact["outcome"], "completed");
    assert!(compact.get("context").is_none());
    assert!(compact.get("trace").is_none());
    let diagnostics_path = compact["diagnostics_ref"]["path"]
        .as_str()
        .ok_or("compact output omitted diagnostic artifact path")?;
    let diagnostics_path = if Path::new(diagnostics_path).is_absolute() {
        Path::new(diagnostics_path).to_path_buf()
    } else {
        root.join(diagnostics_path)
    };
    assert!(diagnostics_path.is_file());

    let diagnostic = command(&root)
        .arg("skill")
        .arg(&skill_dir)
        .arg("--inputs")
        .arg("inputs.json")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json", "--diagnostics"])
        .output()?;
    let diagnostic = assert_json(&diagnostic, 0)?;
    assert!(diagnostic.get("context").is_some());
    assert!(diagnostic.get("trace").is_some());

    Ok(())
}

/// The hosted grants a loopback Connect lists for `fixture/records`: one
/// holding every scope the `records` skill needs, and one short of them.
const RECORDS_GRANTS: &[(&str, &str, &[&str])] = &[
    (
        "grant_records_read",
        "fixture/records",
        &["records.read", "records.summary.read"],
    ),
    (
        "grant_records_wrong_scope",
        "fixture/records",
        &["records.summary.read"],
    ),
];

/// Makes `root` the hosted-records plugin: one hosted connector,
/// `fixture/records`, and the `records` skill that reads it.
fn hosted_records(root: &Path) -> TestResult<std::path::PathBuf> {
    crate::support::copy_dir(
        &crate::support::repo_root()?.join("fixtures/journeys/hosted-records"),
        root,
    )?;
    Ok(root.join("skills/records"))
}

/// The check and detail of an inspection's first failing readiness reason.
fn failed_readiness(inspection: &Value) -> TestResult<(String, String)> {
    let reason = inspection["connector_readiness"][0]["reasons"]
        .as_array()
        .and_then(|reasons| reasons.iter().find(|reason| reason["outcome"] == "fail"))
        .ok_or("a failing readiness reason")?;
    Ok((
        json_string(reason, "check")?.to_owned(),
        json_string(reason, "detail")?.to_owned(),
    ))
}

#[test]
fn provider_failure_is_not_success_and_readiness_is_actionable() -> TestResult {
    let root = crate::support::temp_root("runx-operator-provider-readiness-journey");
    let skill_dir = hosted_records(&root)?;
    let receipt_dir = root.join(".runx/receipts");
    let connect = crate::support::MockConnect::serve(&root, "operator-1", RECORDS_GRANTS)?;
    let operator = |root: &Path| {
        let mut command = command(root);
        connect.apply(&mut command);
        command
    };
    let inspect = || -> TestResult<Value> {
        let output = operator(&root)
            .arg("skill")
            .arg("inspect")
            .arg(&skill_dir)
            .arg("list")
            .arg("--json")
            .output()?;
        assert_json(&output, 0)
    };
    let confirm = |grant: &str| {
        operator(&root)
            .args(["connector", "use", "fixture/records", grant, "--json"])
            .output()
    };

    // With no confirmed grant, readiness names the setup the run needs.
    let unset = inspect()?;
    assert_eq!(unset["readiness"]["status"], "needs_connector_setup");
    assert_eq!(failed_readiness(&unset)?.0, "instance");

    assert_json(&confirm("grant_records_read")?, 0)?;
    let ready = inspect()?;
    assert_eq!(ready["readiness"]["status"], "ready");
    let readiness = ready["connector_readiness"]
        .as_array()
        .and_then(|operations| {
            operations
                .iter()
                .find(|operation| operation["operation"] == "records.list")
        })
        .ok_or("records.list readiness missing")?;
    assert_eq!(readiness["connector"], "fixture/records");
    assert!(
        readiness["instance_id"]
            .as_str()
            .is_some_and(|instance| instance.starts_with("hosted.")),
        "{readiness}"
    );
    assert_eq!(readiness["ready"], true);

    // A grant without the operation's scope is refused before anything is sent.
    assert_json(&confirm("grant_records_wrong_scope")?, 0)?;
    let denied = operator(&root)
        .arg("skill")
        .arg(&skill_dir)
        .arg("list")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json", "--diagnostics"])
        .output()?;
    let denied = assert_json(&denied, 1)?;
    assert_eq!(denied["status"], "sealed");
    assert_eq!(denied["outcome"], "blocked");
    assert_eq!(denied["closure"]["disposition"], "blocked");
    assert_eq!(denied["closure"]["reason"], "authority_denied");
    assert_eq!(denied["refusal"]["code"], "authority_denied", "{denied}");
    assert_eq!(denied["refusal"]["verb"], "read", "{denied}");
    assert_eq!(denied["trace"]["status"], "blocked");
    let receipt_id = json_string(&denied, "receipt_id")?;
    let run_id = json_string(&denied, "run_id")?;
    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    assert_run_sealed_with_local_receipt(&root, &receipt_dir, run_id, receipt_id)?;

    Ok(())
}

#[test]
fn a_host_states_the_grant_its_run_executes_under() -> TestResult {
    let root = crate::support::temp_root("runx-operator-stated-grant-journey");
    let skill_dir = hosted_records(&root)?;
    let receipt_dir = root.join(".runx/receipts");
    let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let recorded = std::sync::Arc::clone(&sent);
    // Connect records each provider call and answers none, so a call that
    // gets past admission shows the grant it was sent under.
    let connect = crate::support::MockConnect::with_operations(
        &root,
        "seller-1",
        RECORDS_GRANTS,
        Box::new(move |request| {
            recorded.lock().ok()?.push(request.clone());
            None
        }),
    )?;
    let operator = |root: &Path| {
        let mut command = command(root);
        connect.apply(&mut command);
        command
    };
    let stated = |grant: &str| format!(r#"{{"grants":{{"fixture/records":"{grant}"}}}}"#);
    let inspect = |authority: &str| -> TestResult<Value> {
        let output = operator(&root)
            .arg("skill")
            .arg("inspect")
            .arg(&skill_dir)
            .arg("list")
            .args(["--run-authority", authority, "--json"])
            .output()?;
        assert_json(&output, 0)
    };

    // The stated grant is the run's instance, read through the host: the
    // project has no setup at all.
    let ready = inspect(&stated("grant_records_read"))?;
    assert_eq!(ready["readiness"]["status"], "ready", "{ready}");
    assert!(
        ready["connector_readiness"][0]["instance_id"]
            .as_str()
            .is_some_and(|instance| instance.starts_with("hosted.")),
        "{ready}"
    );

    // A stated grant short of the operation's scopes is not ready, by name.
    let (check, detail) = failed_readiness(&inspect(&stated("grant_records_wrong_scope"))?)?;
    assert_eq!(check, "capability");
    assert!(detail.contains("lacks records.read"), "{detail}");

    // A grant the host does not hold is refused at the instance.
    let (check, detail) = failed_readiness(&inspect(&stated("grant_records_absent"))?)?;
    assert_eq!(check, "instance");
    assert!(
        detail.contains("states grant grant_records_absent for connector fixture/records"),
        "{detail}"
    );

    // A run under the stated grant is admitted and calls under that grant.
    let run = operator(&root)
        .arg("skill")
        .arg(&skill_dir)
        .arg("list")
        .args(["--run-authority", &stated("grant_records_read")])
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let calls = sent
        .lock()
        .map_err(|_| "the Connect fixture panicked")?
        .clone();
    assert!(
        !calls.is_empty(),
        "{}",
        String::from_utf8_lossy(&run.stdout)
    );
    assert!(
        calls
            .iter()
            .all(|call| call["grant_id"] == "grant_records_read"),
        "{calls:?}"
    );

    // An operator's selection of another instance conflicts with the host's
    // statement, and the run never chooses between them.
    let confirm = operator(&root)
        .args([
            "connector",
            "use",
            "fixture/records",
            "grant_records_wrong_scope",
            "--json",
        ])
        .output()?;
    assert_json(&confirm, 0)?;
    let (check, detail) = failed_readiness(&inspect(&stated("grant_records_read"))?)?;
    assert_eq!(check, "instance");
    assert!(
        detail.contains("is selected; a run never chooses between them"),
        "{detail}"
    );

    Ok(())
}

fn command(root: &Path) -> Command {
    crate::support::unsigned_runx_command_at(root)
}

fn assert_receipt_verifies(root: &Path, receipt_dir: &Path, receipt_id: &str) -> TestResult {
    let verify = command(root)
        .arg("verify")
        .arg(receipt_id)
        .arg("--receipt-dir")
        .arg(receipt_dir)
        .arg("--json")
        .output()?;
    let verify = assert_json(&verify, 0)?;
    assert_eq!(verify["valid"], true);
    assert!(verify["trees"].as_array().is_some_and(|trees| {
        trees
            .iter()
            .any(|tree| tree["root_receipt_id"] == receipt_id && tree["valid"] == true)
    }));
    Ok(())
}

/// The run sealed with this receipt, and the receipt inspects as the signed
/// record: history reads the run sealed, and its receipt id reads the receipt.
fn assert_run_sealed_with_local_receipt(
    root: &Path,
    receipt_dir: &Path,
    run_id: &str,
    receipt_id: &str,
) -> TestResult {
    let read = run_read(root, run_id)?;
    assert_eq!(read["run"]["state"], "sealed");
    assert_eq!(read["run"]["receipt_id"], receipt_id);
    let detail = command(root)
        .arg("history")
        .arg(receipt_id)
        .arg("--receipt-dir")
        .arg(receipt_dir)
        .arg("--json")
        .output()?;
    let detail = assert_json(&detail, 0)?;
    assert_eq!(detail["receipt"]["id"], receipt_id);
    // The run sealed under this home's key, so this home verifies it.
    assert_eq!(detail["receipt"]["verification"]["status"], "verified");
    assert_eq!(detail["receipt"]["issuer_type"], "local");
    Ok(())
}

/// Whether a run read shows the run paused.
fn run_is_paused(read: &Value) -> bool {
    read["run"]["state"] == "paused"
}

/// `runx history <run-id> --json`: the run as a host reads it.
fn run_read(root: &Path, run_id: &str) -> TestResult<Value> {
    let output = command(root)
        .arg("history")
        .arg(run_id)
        .arg("--json")
        .output()?;
    let read = assert_json(&output, 0)?;
    assert_eq!(read["schema"], "runx.run_read.v1");
    Ok(read)
}

fn assert_json(output: &Output, expected_exit: i32) -> TestResult<Value> {
    assert_exit(output, expected_exit)?;
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn assert_exit(output: &Output, expected_exit: i32) -> TestResult {
    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    crate::support::assert_json_stderr(&output.stderr)?;
    Ok(())
}

fn output_with_stdin(command: &mut Command, input: &str) -> TestResult<Output> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("child stdin was not piped")?
        .write_all(input.as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn json_string<'a>(value: &'a Value, field: &str) -> TestResult<&'a str> {
    value[field]
        .as_str()
        .ok_or_else(|| format!("missing string field {field}").into())
}
