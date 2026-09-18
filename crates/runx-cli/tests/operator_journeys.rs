use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

mod complex_flows;

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
        skill_dir.join("X.yaml"),
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
    assert_eq!(inspect["runner"]["type"], "graph");
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

    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    let history = assert_history_contains_local_receipt(&root, &receipt_dir, receipt_id)?;
    let short_history = command(&root)
        .args(["history", receipt_id, "-j"])
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .output()?;
    assert_eq!(assert_json(&short_history, 0)?, history);
    let short_detail = command(&root)
        .args(["history", "-j", receipt_id, "--detail"])
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .output()?;
    let long_detail = command(&root)
        .args(["history", receipt_id, "--detail", "--json"])
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .output()?;
    assert_eq!(
        assert_json(&short_detail, 0)?,
        assert_json(&long_detail, 0)?
    );

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
        .args(["--thread-title", "Docs bug"])
        .output()?;
    assert_exit(&pause, 2)?;
    let pause_text = String::from_utf8(pause.stdout)?;
    assert!(pause_text.contains("status: needs_agent"));
    assert!(pause_text.contains("pending_requests: 1"));
    assert!(pause_text.contains("agent_task.issue-intake.output"));
    assert!(!pause_text.contains("<answers.json>"));
    assert!(!pause_text.trim_start().starts_with('{'));
    let run_id = pause_text
        .lines()
        .find_map(|line| line.strip_prefix("run_id: "))
        .ok_or("pending run omitted run_id")?
        .to_owned();
    assert!(run_id.starts_with("run_agent_task-issue-intake-output_"));
    assert!(pause_text.contains(&format!("runx resume {run_id} -")));

    let pending = history_json(&root, &receipt_dir, &run_id)?;
    assert!(
        pending["pendingRuns"]
            .as_array()
            .is_some_and(|runs| runs.iter().any(|run| run["id"] == run_id))
    );

    let malformed_answers = root.join("malformed-answers.json");
    fs::write(
        &malformed_answers,
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": "not-an-object"
                }
            }
        })
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
        serde_json::json!({
            "answers": {
                "agent_task.issue-intake.output": {
                    "intake_report": {
                        "summary": "Docs bug is bounded."
                    }
                }
            }
        })
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
    let history = assert_history_contains_local_receipt(&root, &receipt_dir, receipt_id)?;
    assert!(
        history["pendingRuns"]
            .as_array()
            .is_some_and(|runs| runs.iter().all(|run| run["id"] != run_id))
    );

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
            })
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
        assert_receipt_verifies(&case_root, &receipt_dir, receipt_id)?;
        assert_history_contains_local_receipt(&case_root, &receipt_dir, receipt_id)?;
    }

    Ok(())
}

#[test]
fn standalone_business_ops_journey_routes_verifies_and_reads_receipt_tree() -> TestResult {
    let root = crate::support::temp_root("runx-operator-business-ops-journey");
    let skill_dir = crate::support::repo_root()?.join("skills/business-ops");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&root)?;

    let run = command(&root)
        .arg("skill")
        .arg(&skill_dir)
        .args([
            "--input",
            "signal=Launch readiness for API v2 with docs, release, customer comms, and spend checks.",
            "--input",
            "operator_context=Live sends route through send-as; payment movement requires a spend gate and provider readback.",
        ])
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json", "--diagnostics"])
        .output()?;
    let run = assert_json(&run, 0)?;

    assert_eq!(run["status"], "sealed");
    assert_eq!(run["closure"]["disposition"], "closed");
    assert_eq!(run["trace"]["graph"], "business-ops-route");
    assert_eq!(run["trace"]["status"], "succeeded");
    assert_eq!(
        run["trace"]["steps"].as_array().map(|steps| steps
            .iter()
            .map(|step| step["step_id"].clone())
            .collect::<Vec<_>>()),
        Some(["route"].into_iter().map(serde_json::Value::from).collect())
    );
    assert_eq!(run["result"]["lane_packet"]["data"]["lane"], "classify");
    assert_eq!(
        run["result"]["lane_packet"]["data"]["handoff"]["lane_ref"],
        "business-ops"
    );
    assert!(run.get("receipt").is_none());
    assert!(run.get("execution").is_none());
    let receipt_id = json_string(&run, "receipt_id")?;

    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    assert_history_contains_local_receipt(&root, &receipt_dir, receipt_id)?;

    Ok(())
}

#[test]
fn provider_failure_is_not_success_and_readiness_is_actionable() -> TestResult {
    let root = crate::support::temp_root("runx-operator-provider-readiness-journey");
    let skill_dir = crate::support::repo_root()?.join("skills/google-analytics");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&root)?;

    let incomplete = command(&root)
        .env("RUNX_PROVIDER_PERMISSION_GRANT_ID", "grant_google_read")
        .env(
            "RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES",
            r#"["properties.read","reports.read"]"#,
        )
        .arg("skill")
        .arg("inspect")
        .arg(&skill_dir)
        .arg("properties")
        .arg("--json")
        .output()?;
    let incomplete = assert_json(&incomplete, 0)?;
    assert_ne!(incomplete["provider"]["status"], "ready");

    let inspect = command(&root)
        .env("RUNX_PROVIDER_PERMISSION_GRANT_ID", "grant_google_read")
        .env(
            "RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES",
            r#"["properties.read","reports.read"]"#,
        )
        .env(
            "RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF",
            "runx:principal:operator:test",
        )
        .arg("skill")
        .arg("inspect")
        .arg(&skill_dir)
        .arg("properties")
        .arg("--json")
        .output()?;
    let inspect = assert_json(&inspect, 0)?;
    assert_eq!(inspect["readiness"]["status"], "ready");
    assert_eq!(inspect["provider"]["status"], "ready");
    assert_eq!(
        inspect["provider"]["requirements"][0]["provider"],
        "google-analytics"
    );
    assert_eq!(
        inspect["provider"]["requirements"][0]["operation"],
        "properties.list"
    );
    assert_eq!(
        inspect["provider"]["requirements"][0]["grant_ref"],
        "runx:grant:grant_google_read"
    );

    let denied = command(&root)
        .env(
            "RUNX_PROVIDER_PERMISSION_GRANT_ID",
            "grant_google_wrong_scope",
        )
        .env(
            "RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES",
            r#"["reports.read"]"#,
        )
        .env(
            "RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF",
            "runx:principal:operator:test",
        )
        .arg("skill")
        .arg(&skill_dir)
        .arg("properties")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json", "--diagnostics"])
        .output()?;
    let denied = assert_json(&denied, 1)?;
    assert_eq!(denied["status"], "sealed");
    assert_eq!(denied["outcome"], "blocked");
    assert_eq!(denied["closure"]["disposition"], "blocked");
    assert_eq!(denied["closure"]["reason_code"], "authority_denied");
    assert_eq!(denied["trace"]["status"], "blocked");
    let receipt_id = json_string(&denied, "receipt_id")?;
    assert_receipt_verifies(&root, &receipt_dir, receipt_id)?;
    assert_history_contains_local_receipt(&root, &receipt_dir, receipt_id)?;

    Ok(())
}

#[cfg(unix)]
#[test]
fn issue_442_local_github_replay_resolves_checkout_before_transport_and_skips_hosted_grant()
-> TestResult {
    let root = crate::support::temp_root("runx-issue-442-local-github-replay");
    let skill_dir = root.join("skills/github-issue-inspect");
    let fake_bin = root.join("fake-bin");
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&skill_dir)?;
    fs::create_dir_all(&fake_bin)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: github-issue-inspect\ndescription: Inspect one GitHub issue from the owning checkout.\n---\n# GitHub Issue Inspect\n\nReturn the requested issue as bounded provider evidence.\n",
    )?;
    fs::write(
        skill_dir.join("X.yaml"),
        r#"skill: github-issue-inspect
version: "0.1.0"
runners:
  inspect:
    default: true
    type: graph
    graph:
      name: github-issue-inspect
      result_from: [read-issue]
      steps:
        - id: read-issue
          tool: provider.read
          scopes: [repo.read]
          policy:
            provider_permission:
              verb: read
          inputs:
            expected_provider: github
            operation: issue.read
            target: .
            expected_result:
              repository: nitrosend/nitrosend
            result_fields: [repository, number, title, state, body, url, labels]
            input:
              issue_number: 442
"#,
    )?;
    let git = |args: &[&str]| -> TestResult {
        let output = Command::new("git").current_dir(&root).args(args).output()?;
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    git(&["init", "--quiet"])?;
    git(&[
        "remote",
        "add",
        "origin",
        "git@github.com:nitrosend/nitrosend.git",
    ])?;

    let gh = fake_bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$dir/gh.log"
case "$*" in
  *graphql*)
    /bin/cat >/dev/null
    printf '%s\n' '{"data":{"viewer":{"id":"U_operator","login":"operator"},"repository":{"nameWithOwner":"nitrosend/nitrosend","viewerPermission":"WRITE"}}}'
    ;;
  *"repos/nitrosend/nitrosend/issues/442"*)
    printf '%s\n' '{"number":442,"title":"Use local GitHub transport","state":"open","body":"Operator regression","html_url":"https://github.com/nitrosend/nitrosend/issues/442","labels":[]}'
    ;;
  *)
    printf '%s\n' '{"message":"unexpected gh invocation"}' >&2
    exit 2
    ;;
esac
"#,
    )?;
    let mut permissions = fs::metadata(&gh)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&gh, permissions)?;

    let run = command(&root)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("RUNX_PUBLIC_API_BASE_URL", "https://wrong-hosted.invalid")
        .env("RUNX_PUBLIC_API_TOKEN", "wrong-hosted-grant")
        .arg("skill")
        .arg(&skill_dir)
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let run = assert_json(&run, 0)?;
    assert_eq!(run["status"], "sealed");
    assert_eq!(run["outcome"], "completed");
    let operation = run["result"]["provider_operation"]["data"]
        .as_object()
        .or_else(|| run["result"]["provider_operation"].as_object())
        .ok_or("provider operation result missing")?;
    assert_eq!(
        operation.get("transport").and_then(Value::as_str),
        Some("local_github")
    );
    assert_eq!(
        operation
            .get("result")
            .and_then(Value::as_object)
            .and_then(|result| result.get("number"))
            .and_then(Value::as_str),
        Some("442")
    );
    let log = fs::read_to_string(fake_bin.join("gh.log"))?;
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("repos/nitrosend/nitrosend/issues/442"))
            .count(),
        1
    );
    assert!(log.lines().all(|line| !line.contains("sourcey/catalog")));
    Ok(())
}

#[cfg(unix)]
#[test]
fn reference_skill_journeys_execute_or_block_truthfully_without_repeating_phase_producers()
-> TestResult {
    let root = crate::support::temp_root("runx-reference-skill-journeys");
    let repo_root = crate::support::repo_root()?;
    let receipt_dir = root.join(".runx/receipts");
    fs::create_dir_all(&root)?;

    for (skill, expected_runner) in [
        ("send-as", "send"),
        ("github-sync", "github-sync"),
        ("spend", "spend"),
    ] {
        let inspect = command(&root)
            .arg("skill")
            .arg("inspect")
            .arg(repo_root.join("skills").join(skill))
            .arg("--json")
            .output()?;
        let inspect = assert_json(&inspect, 0)?;
        assert_eq!(inspect["runner"]["name"], expected_runner);
        assert_eq!(
            inspect["semantic_report"]["diagnostics"],
            serde_json::json!([])
        );
    }

    let send_inputs = root.join("send-apply.json");
    fs::write(
        &send_inputs,
        serde_json::json!({
            "send_plan": {
                "decision": "ready",
                "action_family": "send-as",
                "principal": { "type": "account", "ref": "account:release-bot" },
                "provider": { "name": "slack", "account_ref": "workspace:demo", "runtime_path": "channel.post" },
                "send_class": "status",
                "channel": "chat",
                "audience": { "type": "channel", "ref": "slack://T123/C456", "requires_reconfirmation": false },
                "content": {
                    "draft_ref": "release:notice",
                    "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "subject_or_title": "Release notice"
                },
                "gates": { "preflight_required": true, "human_approval_required": true, "approval_ref": "send-as.live" },
                "blockers": [],
                "provider_actions": ["channel.post"],
                "evidence_refs": ["release:notice"],
                "success_checkpoint": {
                    "milestone": "provider_delivery_required",
                    "description": "Apply must deliver and preserve provider evidence."
                }
            },
            "connector": {
                "provider": "slack",
                "target": "slack://T123/C456"
            }
        })
        .to_string(),
    )?;
    let send = command(&root)
        .env("RUNX_PROVIDER_PERMISSION_GRANT_ID", "grant_slack_send")
        .env(
            "RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES",
            r#"["message.send","message.read"]"#,
        )
        .env(
            "RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF",
            "runx:principal:operator:test",
        )
        .env("RUNX_PUBLIC_API_TOKEN", "not-used-before-approval")
        .env("RUNX_PUBLIC_API_BASE_URL", "https://wrong-hosted.invalid")
        .arg("skill")
        .arg(repo_root.join("skills/send-as"))
        .arg("apply")
        .arg("--inputs")
        .arg("send-apply.json")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json", "--diagnostics"])
        .output()?;
    let send = assert_json(&send, 2)?;
    assert_eq!(send["status"], "needs_approval", "{send}");
    assert_eq!(send["outcome"], "deferred");
    assert_eq!(send["requests"].as_array().map(Vec::len), Some(1));
    assert_eq!(send["requests"][0]["kind"], "approval");
    assert!(
        send["requests"][0]["id"]
            .as_str()
            .is_some_and(|request| request.starts_with("provider-effect:"))
    );

    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    let git = |args: &[&str]| -> TestResult {
        let output = Command::new("git").current_dir(&root).args(args).output()?;
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    git(&["init", "--quiet"])?;
    git(&[
        "remote",
        "add",
        "origin",
        "git@github.com:nitrosend/nitrosend.git",
    ])?;
    let gh = fake_bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$dir/gh.log"
case "$*" in
  *graphql*)
    /bin/cat >/dev/null
    printf '%s\n' '{"data":{"viewer":{"id":"U_operator","login":"operator"},"repository":{"nameWithOwner":"nitrosend/nitrosend","viewerPermission":"WRITE"}}}'
    ;;
  *"repos/nitrosend/nitrosend/issues?"*)
    printf '%s\n' '[{"number":442,"title":"Use local GitHub transport","state":"open","body":"Operator regression","html_url":"https://github.com/nitrosend/nitrosend/issues/442","labels":[]}]'
    ;;
  *)
    printf '%s\n' '{"message":"unexpected gh invocation"}' >&2
    exit 2
    ;;
esac
"#,
    )?;
    let mut permissions = fs::metadata(&gh)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&gh, permissions)?;

    let github = command(&root)
        .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
        .env("RUNX_PUBLIC_API_BASE_URL", "https://wrong-hosted.invalid")
        .env("RUNX_PUBLIC_API_TOKEN", "wrong-hosted-grant")
        .arg("skill")
        .arg(repo_root.join("skills/github-sync"))
        .args([
            "--input",
            "repo=nitrosend/nitrosend",
            "--input",
            "direction=pull",
            "--input",
            "scope=read",
            "--input",
            r#"resources={"kind":"issues","filters":{"state":"open","limit":1}}"#,
            "--receipt-dir",
        ])
        .arg(&receipt_dir)
        .arg("--json")
        .output()?;
    let github = assert_json(&github, 0)?;
    assert_eq!(github["status"], "sealed");
    assert_eq!(github["outcome"], "completed");
    assert_eq!(
        github["result"]["provider_operation"]["data"]["transport"],
        "local_github"
    );
    let gh_log = fs::read_to_string(fake_bin.join("gh.log"))?;
    assert_eq!(
        gh_log
            .lines()
            .filter(|line| line.contains("/issues?"))
            .count(),
        1
    );

    let spend_inputs = root.join("spend-no-rail.json");
    fs::write(
        &spend_inputs,
        serde_json::json!({
            "payment_signal": {
                "signal_type": "effect_required",
                "challenge_id": "ch_payment_default_no_rail",
                "amount_minor": 125,
                "currency": "USD",
                "counterparty": "merchant:demo",
                "operation": "search.paid",
                "realm": "test"
            },
            "parent_payment_authority": { "term_id": "authority-term:payment:default-no-rail" },
            "rail_profile_ref": "rail-profile:hosted:test",
            "realm": "test",
            "idempotency_seed": "default-no-rail"
        })
        .to_string(),
    )?;
    let spend = command(&root)
        .arg("skill")
        .arg(repo_root.join("skills/spend"))
        .arg("--inputs")
        .arg("spend-no-rail.json")
        .arg("--json")
        .output()?;
    assert_eq!(spend.status.code(), Some(1));
    let spend: Value = serde_json::from_slice(&spend.stdout)?;
    assert_eq!(spend["status"], "failure");
    assert!(
        spend["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("rail"))
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn issue_to_pr_journey_publishes_once_with_readback_and_no_secondary_work() -> TestResult {
    let root = crate::support::temp_root("runx-issue-to-pr-journey");
    let repo_root = crate::support::repo_root()?;
    let receipt_dir = root.join(".runx/receipts");
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin)?;

    let git = |args: &[&str]| -> TestResult {
        let output = Command::new("git").current_dir(&root).args(args).output()?;
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    git(&["init", "--quiet"])?;
    git(&["remote", "add", "origin", "git@github.com:runxhq/runx.git"])?;

    let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let contract_digest = "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    fs::write(
        root.join("scafld-receipt.json"),
        serde_json::json!({
            "body": {
                "task_id": "issue-to-pr-integration",
                "verdict": "pass",
                "head_commit": commit,
                "spec_fingerprint": contract_digest.trim_start_matches("sha256:"),
                "open_blockers": []
            }
        })
        .to_string(),
    )?;

    let fake_git = fake_bin.join("git");
    fs::write(
        &fake_git,
        r#"#!/bin/sh
set -eu
dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd)
commit=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
printf '%s\n' "$*" >> "$dir/git.log"
case "$*" in
  "-c core.fsmonitor=false remote get-url origin"|"config --get remote.origin.url")
    printf '%s\n' 'git@github.com:runxhq/runx.git'
    ;;
  "rev-parse --verify ${commit}^{commit}")
    printf '%s\n' "$commit"
    ;;
  "ls-remote --refs origin refs/heads/fix/442-operator-first")
    if [ -f "$dir/pushed-ref" ]; then
      printf '%s\t%s\n' "$commit" 'refs/heads/fix/442-operator-first'
    fi
    ;;
  "push --porcelain origin ${commit}:refs/heads/fix/442-operator-first")
    : > "$dir/pushed-ref"
    printf '%s\n' 'To git@github.com:runxhq/runx.git'
    ;;
  *)
    printf '%s\n' "unexpected git invocation: $*" >&2
    exit 2
    ;;
esac
"#,
    )?;
    let mut permissions = fs::metadata(&fake_git)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&fake_git, permissions)?;

    let scafld = fake_bin.join("scafld");
    fs::write(
        &scafld,
        r#"#!/bin/sh
set -eu
dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$dir/scafld.log"
case "$*" in
  verify*" --target aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --root "*" --json")
    printf '%s\n' '{"ok":true,"command":"verify"}'
    ;;
  *)
    printf '%s\n' "unexpected scafld invocation: $*" >&2
    exit 2
    ;;
esac
"#,
    )?;
    let mut permissions = fs::metadata(&scafld)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&scafld, permissions)?;

    let gh = fake_bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$dir/gh.log"
case "$*" in
  *graphql*)
    /bin/cat >/dev/null
    printf '%s\n' '{"data":{"viewer":{"id":"U_operator","login":"operator"},"repository":{"nameWithOwner":"runxhq/runx","viewerPermission":"WRITE"}}}'
    ;;
  *"--method POST repos/runxhq/runx/pulls"*)
    /bin/cat > "$dir/approved-pr-body.json"
    : > "$dir/published-pr"
    printf '%s\n' '{"number":77,"title":"Make issue-to-PR operator-first","state":"open","body":"Closes #442.","html_url":"https://github.com/runxhq/runx/pull/77","head":{"ref":"fix/442-operator-first","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"base":{"ref":"main"},"draft":false}'
    ;;
  *"--method GET repos/runxhq/runx/pulls?"*)
    if [ -f "$dir/published-pr" ]; then
      printf '%s\n' '[{"number":77,"title":"Make issue-to-PR operator-first","state":"open","body":"Closes #442.","html_url":"https://github.com/runxhq/runx/pull/77","head":{"ref":"fix/442-operator-first","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"base":{"ref":"main"},"draft":false}]'
    else
      printf '%s\n' '[]'
    fi
    ;;
  *"--method GET repos/runxhq/runx/pulls/77"*)
    printf '%s\n' '{"number":77,"title":"Make issue-to-PR operator-first","state":"open","body":"Closes #442.","html_url":"https://github.com/runxhq/runx/pull/77","head":{"ref":"fix/442-operator-first","sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"base":{"ref":"main"},"draft":false}'
    ;;
  *)
    printf '%s\n' "unexpected gh invocation: $*" >&2
    exit 2
    ;;
esac
"#,
    )?;
    let mut permissions = fs::metadata(&gh)?.permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&gh, permissions)?;

    let inputs = root.join("issue-to-pr-inputs.json");
    fs::write(
        &inputs,
        serde_json::json!({
            "issue_evidence": {
                "schema": "runx.issue_to_pr.issue_evidence.v1",
                "repository": "runxhq/runx",
                "number": "442",
                "title": "Make issue-to-PR operator-first",
                "state": "open",
                "body": "Use normal host tools and govern only PR publication.",
                "url": "https://github.com/runxhq/runx/issues/442",
                "labels": ["operator-experience"],
                "assignees": [],
                "source": {
                    "provider": "github",
                    "transport": "local_github",
                    "principal_ref": "runx:principal:github:github.com:operator:U_operator",
                    "readback_ref": "runx:github-readback:sha256:prior-issue-evidence"
                },
                "checkout_resolved": true
            },
            "host_result": {
                "schema": "runx.issue_to_pr.host_result.v1",
                "status": "completed",
                "repository": "runxhq/runx",
                "issue_number": "442",
                "repo_root": ".",
                "branch": "fix/442-operator-first",
                "commit": commit,
                "files": ["skills/issue-to-pr/X.yaml"],
                "tests": [{
                    "command": "cargo test -p runx-cli --test integration issue_to_pr_journey",
                    "status": "passed",
                    "evidence": "receipt:test:issue-to-pr"
                }],
                "finalization": {
                    "receipt_path": "scafld-receipt.json",
                    "contract_digest": contract_digest
                },
                "publication": {
                    "decision": "ready",
                    "title": "Make issue-to-PR operator-first",
                    "body": "Closes #442.",
                    "head": "fix/442-operator-first",
                    "base": "main",
                    "draft": false,
                    "idempotency_key": "issue-to-pr-442-integration"
                },
                "errors": []
            }
        })
        .to_string(),
    )?;

    let local_env = |command: &mut Command| {
        command
            .env("PATH", format!("{}:/usr/bin:/bin", fake_bin.display()))
            .env("RUNX_SCAFLD_BIN", &scafld)
            .env("RUNX_PROVIDER_PERMISSION_TRANSPORT", "local:github")
            .env_remove("RUNX_PROVIDER_PERMISSION_GRANT_ID")
            .env_remove("RUNX_PROVIDER_PERMISSION_GRANTED_SCOPES")
            .env_remove("RUNX_PROVIDER_PERMISSION_PRINCIPAL_REF")
            .env_remove("RUNX_PUBLIC_API_BASE_URL")
            .env_remove("RUNX_PUBLIC_API_TOKEN");
    };

    let mut start = command(&root);
    local_env(&mut start);
    let start = start
        .arg("skill")
        .arg(repo_root.join("skills/issue-to-pr"))
        .arg("resume")
        .arg("--inputs")
        .arg("issue-to-pr-inputs.json")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .args(["--json", "--diagnostics"])
        .output()?;
    let result = assert_json(&start, 0)?;
    assert_eq!(result["status"], "sealed", "{result}");
    assert_eq!(result["outcome"], "completed");
    assert_eq!(
        result["result"]["issue_to_pr_result"]["data"]["publication"]["status"],
        "published"
    );
    assert_eq!(
        result["result"]["issue_to_pr_result"]["data"]["publication"]["pr_number"],
        "77"
    );

    let gh_log = fs::read_to_string(fake_bin.join("gh.log"))?;
    assert_eq!(
        gh_log
            .lines()
            .filter(|line| line.contains("--method POST repos/runxhq/runx/pulls"))
            .count(),
        1,
        "PR publication must occur exactly once: {gh_log}"
    );
    assert_eq!(
        gh_log
            .lines()
            .filter(|line| line.contains("--method GET repos/runxhq/runx/pulls/77"))
            .count(),
        1,
        "PR readback must occur exactly once: {gh_log}"
    );
    assert_eq!(
        gh_log
            .lines()
            .filter(|line| line.contains("graphql"))
            .count(),
        1,
        "the scoped write must validate GitHub identity exactly once: {gh_log}"
    );
    assert!(
        gh_log.lines().all(|line| !line.contains("/issues/442")),
        "publication must not repeat issue discovery: {gh_log}"
    );
    let approved_body: Value =
        serde_json::from_slice(&fs::read(fake_bin.join("approved-pr-body.json"))?)?;
    assert_eq!(approved_body["title"], "Make issue-to-PR operator-first");
    assert_eq!(approved_body["head"], "fix/442-operator-first");
    assert_eq!(approved_body["base"], "main");

    let git_log = fs::read_to_string(fake_bin.join("git.log"))?;
    assert_eq!(
        git_log
            .lines()
            .filter(|line| line.starts_with("push --porcelain"))
            .count(),
        1,
        "the exact tested commit must be pushed once: {git_log}"
    );
    assert_eq!(
        git_log
            .lines()
            .filter(|line| line.starts_with("rev-parse --verify"))
            .count(),
        1,
        "the exact commit must be verified once before publication: {git_log}"
    );
    let scafld_log = fs::read_to_string(fake_bin.join("scafld.log"))?;
    assert_eq!(
        scafld_log.lines().count(),
        1,
        "finalization must verify once"
    );
    assert!(scafld_log.contains(" --target aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa "));

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
    let request_digest = json_string(request, "request_digest")?;
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

    let answer = |digest: &str| {
        serde_json::json!({
            "request_digests": { (request_id): digest },
            "answers": {
                (request_id): {
                    "intake_report": { "summary": "Docs bug is bounded." }
                }
            }
        })
        .to_string()
    };
    let mut mismatch = command(&root);
    mismatch
        .arg("resume")
        .arg(run_id)
        .arg("-")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json");
    let mismatch = output_with_stdin(&mut mismatch, &answer("sha256:wrong-request"))?;
    let mismatch = assert_json(&mismatch, 1)?;
    assert_eq!(mismatch["status"], "failure");
    assert!(
        mismatch["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("request digest mismatch"))
    );
    let pending = history_json(&root, &receipt_dir, run_id)?;
    assert!(
        pending["pendingRuns"]
            .as_array()
            .is_some_and(|runs| runs.iter().any(|run| run["id"] == run_id))
    );

    let mut resume = command(&root);
    resume
        .arg("resume")
        .arg(run_id)
        .arg("-")
        .arg("--receipt-dir")
        .arg(&receipt_dir)
        .arg("--json");
    let resume = output_with_stdin(&mut resume, &answer(request_digest))?;
    let resume = assert_json(&resume, 0)?;
    assert_eq!(resume["status"], "sealed");
    assert_eq!(resume["outcome"], "completed");
    assert_eq!(resume["run_id"], run_id);
    assert!(root.join(".runx/continuations").is_dir());

    Ok(())
}

#[test]
fn nested_graph_resume_preserves_completed_effects_and_request_digests() -> TestResult {
    for topology in ["nested", "deep", "fanout"] {
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
    type: graph
    graph:
      name: parent
      result_from: [child]
      steps:
        - id: child
          skill: .
          runner: child
  child:
    type: graph
    graph:
      name: child
      result_from: [second-review]
      steps:
        - id: first-write
          tool: fs.write
          scopes: [fs.write]
          inputs:
            path: first.txt
            contents: executed once
        - id: first-review
          run:
            type: agent-task
            agent: reviewer
            task: first-review
            outputs: { decision: object }
          context: { written: first-write.file_write.data }
        - id: second-write
          tool: fs.write
          scopes: [fs.write]
          inputs:
            path: second.txt
            contents: executed once
        - id: second-review
          run:
            type: agent-task
            agent: reviewer
            task: second-review
            outputs: { decision: object }
          context:
            written: second-write.file_write.data
            prior: first-review.decision
"#
        .to_owned();
        if topology == "deep" {
            manifest = manifest.replacen("          runner: child", "          runner: middle", 1);
            manifest.push_str(
                r#"  middle:
    type: graph
    graph:
      name: middle
      result_from: [inner]
      steps:
        - id: inner
          skill: .
          runner: child
"#,
            );
        } else if topology == "fanout" {
            manifest = manifest.replacen(
                "      name: parent",
                r#"      name: parent
      fanout:
        groups:
          work:
            strategy: all
            on_branch_failure: halt"#,
                1,
            );
            manifest = manifest.replacen(
                "        - id: child",
                r#"        - id: sibling
          mode: fanout
          fanout_group: work
          tool: fs.write
          scopes: [fs.write]
          inputs: { path: sibling.txt, contents: executed once }
        - id: child
          mode: fanout
          fanout_group: work"#,
                1,
            );
        }
        fs::write(skill_dir.join("X.yaml"), manifest)?;
        let initial = command(&root)
            .arg("skill")
            .arg(&skill_dir)
            .arg("--receipt-dir")
            .arg(&receipt_dir)
            .arg("--json")
            .output()?;
        let mut paused = assert_json(&initial, 2)?;
        let run_id = json_string(&paused, "run_id")?.to_owned();

        if topology == "fanout" {
            assert_eq!(
                fs::read_to_string(root.join("sibling.txt"))?,
                "executed once"
            );
            fs::write(root.join("sibling.txt"), "observed after pause")?;
        }

        for (index, marker) in ["first.txt", "second.txt"].into_iter().enumerate() {
            assert_eq!(paused["status"], "needs_agent");
            assert_eq!(fs::read_to_string(root.join(marker))?, "executed once");
            // A replay would overwrite this external observation of the first effect.
            fs::write(root.join(marker), "observed after pause")?;
            let request = &paused["requests"][0];
            let request_id = json_string(request, "id")?;
            let digest = json_string(request, "request_digest")?;
            let answers = serde_json::json!({
                "request_digests": { (request_id): digest },
                "answers": { (request_id): { "decision": { "accepted": true } } }
            });
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
        if topology == "fanout" {
            assert_eq!(
                fs::read_to_string(root.join("sibling.txt"))?,
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
        skill_dir.join("X.yaml"),
        r#"skill: compact-output
version: "0.1.0"
runners:
  run:
    default: true
    type: graph
    inputs:
      payload:
        type: string
        required: true
    graph:
      name: compact-output
      result_from: [finish]
      steps:
        - id: expand
          inputs:
            payload: "$input.payload"
          run:
            type: javascript
            module: compact-output.mjs
            export: expand
            outputs:
              intermediate: object
        - id: finish
          context:
            intermediate: expand.intermediate
          run:
            type: javascript
            module: compact-output.mjs
            export: finish
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

fn command(root: &Path) -> Command {
    crate::support::unsigned_runx_command_at(root)
}

fn assert_receipt_verifies(root: &Path, receipt_dir: &Path, receipt_id: &str) -> TestResult {
    let verify = command(root)
        .arg("verify")
        .arg(receipt_id)
        .arg("--receipt-dir")
        .arg(receipt_dir)
        .args(["--allow-local-development-signatures", "--json"])
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

fn assert_history_contains_local_receipt(
    root: &Path,
    receipt_dir: &Path,
    receipt_id: &str,
) -> TestResult<Value> {
    let history = history_json(root, receipt_dir, receipt_id)?;
    let receipt = history["receipts"]
        .as_array()
        .and_then(|receipts| receipts.iter().find(|receipt| receipt["id"] == receipt_id))
        .ok_or("history did not return the sealed receipt")?;
    // History does not silently opt into local-development signature trust.
    // The explicit verify step above proves the receipt; passive history stays
    // fail-closed and labels the same local receipt as unverified.
    assert_eq!(receipt["verification"]["status"], "unverified");
    Ok(history)
}

fn history_json(root: &Path, receipt_dir: &Path, query: &str) -> TestResult<Value> {
    let output = command(root)
        .arg("history")
        .arg(query)
        .arg("--receipt-dir")
        .arg(receipt_dir)
        .arg("--json")
        .output()?;
    assert_json(&output, 0)
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
