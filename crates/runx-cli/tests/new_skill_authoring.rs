use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[test]
fn new_skill_authoring_waits_for_agent_then_applies_one_validated_package() -> TestResult {
    let repo = crate::support::repo_root()?;
    let root = crate::support::isolated_target_temp_root("new-skill-authoring")?;
    let target = root.join("digest-note");
    let receipts = root.join("receipts");

    let pending = command(&repo, &root)
        .arg("new")
        .arg("digest-note")
        .args([
            "--objective",
            "Create a bounded digest-note skill using the native digest capability",
            "--project-context",
            "Keep the package declarative and useful to a cold operator",
            "--directory",
        ])
        .arg(&target)
        .arg("--receipt-dir")
        .arg(&receipts)
        .output()?;
    assert_exit(&pending, 2)?;
    let pending_text = String::from_utf8(pending.stdout)?;
    let run_id = line_value(&pending_text, "run_id: ")?;

    assert!(pending_text.contains("status: needs_agent"));
    assert!(pending_text.contains("architect"));
    assert!(pending_text.contains(&format!(
        "runx resume {run_id} - --receipt-dir {}",
        receipts.display()
    )));
    assert!(
        !target.exists(),
        "runx new wrote before authoring completed"
    );

    // Each act is answered when the run asks it: the architect, then the
    // author its decision leads to.
    let answers = root.join("answers.json");
    let mut completed = Value::Null;
    for (step, exit) in [("architect", 2), ("author", 0)] {
        let read = command(&repo, &root)
            .args(["history", run_id, "--json"])
            .output()?;
        let answer = serde_json::json!({ "answers": { step: authoring_answers()[step].clone() } });
        fs::write(
            &answers,
            crate::support::bind_resume(&assert_json(&read, 0)?, answer)?.to_string(),
        )?;
        let resumed = command(&repo, &root)
            .arg("resume")
            .arg(run_id)
            .arg(&answers)
            .arg("--receipt-dir")
            .arg(&receipts)
            .args(["--json", "--diagnostics"])
            .output()?;
        completed = assert_json(&resumed, exit)?;
    }

    assert_eq!(completed["status"], "sealed");
    assert_eq!(completed["outcome"], "completed");
    assert_eq!(completed["trace"]["graph"], "skill-build");
    assert_eq!(fs::read_dir(&target)?.count(), 2);
    assert!(target.join("SKILL.md").is_file());
    assert!(target.join("run.yaml").is_file());
    assert!(package_modules(&target)?.is_empty());

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn new_skill_authoring_rejects_outside_workspace_without_writing() -> TestResult {
    let repo = crate::support::repo_root()?;
    let root = crate::support::isolated_target_temp_root("new-skill-containment")?;
    let outside = std::env::temp_dir().join(format!("runx-new-outside-{}", std::process::id()));
    fs::remove_dir_all(&outside).ok();

    let output = command(&repo, &root)
        .arg("new")
        .arg("outside-skill")
        .args(["--objective", "Create a bounded skill", "--directory"])
        .arg(&outside)
        .arg("--json")
        .output()?;
    let output = assert_json(&output, 1)?;

    assert_eq!(output["status"], "failure");
    assert_eq!(output["error"]["code"], "invalid_target");
    assert!(!outside.exists());

    fs::remove_dir_all(root)?;
    Ok(())
}

fn command(repo: &Path, runx_home: &Path) -> Command {
    let mut command = crate::support::unsigned_runx_command_at(repo);
    command.env("RUNX_HOME", runx_home.join("home"));
    // The bundled skill-build is added to the project's skills directory, so
    // the project is the test's own, never the repository.
    command.env("RUNX_CWD", runx_home);
    crate::support::isolate_project(&mut command);
    command
}

fn package_modules(target: &Path) -> TestResult<Vec<PathBuf>> {
    Ok(fs::read_dir(target)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("mjs"))
        .collect())
}

fn line_value<'a>(text: &'a str, prefix: &str) -> TestResult<&'a str> {
    text.lines()
        .find_map(|line| line.strip_prefix(prefix))
        .ok_or_else(|| format!("output is missing {prefix:?}").into())
}

fn assert_exit(output: &Output, expected: i32) -> TestResult {
    if output.status.code() != Some(expected) {
        return Err(format!(
            "expected exit {expected}, got {:?}\nstdout:\n{}\nstderr:\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(())
}

fn assert_json(output: &Output, expected: i32) -> TestResult<Value> {
    assert_exit(output, expected)?;
    serde_json::from_slice(&output.stdout).map_err(Into::into)
}

/// What the architect and the author answer, by step.
fn authoring_answers() -> Value {
    serde_json::json!({
            "architect": {
                "architecture_decision": {
                    "schema": "runx.skill.architecture_decision.v1",
                    "choice": "build",
                    "identity": {
                        "proposed_name": "digest-note",
                        "action": "create",
                        "visibility": "public",
                        "rationale": "Digest note is the exact operation an agent requests."
                    },
                    "direct_use": {
                        "trigger_requests": ["Digest this note."],
                        "non_trigger_requests": ["Publish this note."],
                        "default_outcome": "Return the canonical digest for the supplied note.",
                        "routine_host_work": ["Read the supplied note."],
                        "runx_boundary": "Execute the canonical digest capability and receipt the result.",
                        "terminal_result": "A reusable digest packet.",
                        "blocker_behavior": "Block once when the note is absent.",
                        "native_escape": "Return the unchanged note for native hashing."
                    },
                    "chain_use": {
                        "accepted_inputs": ["A note or prior note artifact."],
                        "result": "A reusable digest packet.",
                        "reused_evidence": ["The supplied note artifact."],
                        "reused_effects": [],
                        "must_not_repeat": ["Do not reacquire or rewrite the supplied note."]
                    },
                    "objective": "Create a bounded digest-note skill using the native digest capability.",
                    "operator_value": "Give an operator one stable digest for a supplied note without package code.",
                    "knowledge_contract": {
                        "purpose": "Explain and perform one deterministic note-digest operation.",
                        "evidence_required": ["One note supplied by the operator."],
                        "decision_logic": ["Hash the exact supplied note through the canonical native capability."],
                        "stop_conditions": ["Stop when the note is absent."],
                        "recovery": ["Resume with a non-empty note."]
                    },
                    "required_behaviors": [
                        {
                            "id": "operating-guide",
                            "outcome": "Teach a cold operator what is hashed and what the receipt proves.",
                            "lane": "manual"
                        },
                        {
                            "id": "digest-note",
                            "outcome": "Return a canonical SHA-256 digest for the supplied note.",
                            "lane": "native_capability",
                            "reuses": "data.digest"
                        }
                    ],
                    "native_reuse": {
                        "inspected_capabilities": [
                            "runx.skill.inspect",
                            "runx.skill.plan",
                            "runx.skill.bind",
                            "runx.skill.apply",
                            "data.digest"
                        ],
                        "selected_capabilities": ["data.digest"],
                        "missing_capabilities": []
                    },
                    "effects": [{
                        "effect": "read",
                        "authority_scopes": [],
                        "approval": "none",
                        "provider_boundary": false
                    }],
                    "skill_chain": { "context_skills": [], "routes": [] },
                    "resource_budget": {
                        "max_files": 2,
                        "max_executable_lines": 0,
                        "max_process_spawns": 0,
                        "network_allowed": false
                    },
                    "preservation_obligations": ["Keep the operator manual and native runner aligned."],
                    "deletions": [],
                    "proof_plan": [
                        {
                            "name": "digest-note-selection",
                            "kind": "selection_trial",
                            "expected": "Digest requests select this skill and publish requests do not."
                        },
                        {
                            "name": "digest-note-standalone",
                            "kind": "standalone_operator_journey",
                            "expected": "A supplied note is digested without package code."
                        },
                        {
                            "name": "digest-note-composed",
                            "kind": "composed_operator_journey",
                            "expected": "A prior note artifact is reused without reacquisition."
                        }
                    ]
                }
            },
            "author": {
                "change_draft": {
                    "schema": "runx.skill.change_draft.v1",
                    "decision": "write",
                    "summary": "Create one manual and one declarative native digest graph.",
                    "non_goals": ["Do not add JavaScript, network access, or provider effects."],
                    "writes": [
                        { "path": "SKILL.md", "contents": digest_note_manual() },
                        { "path": "run.yaml", "contents": digest_note_manifest() }
                    ],
                    "deletes": [],
                    "expected_outputs": [{
                        "name": "digest_result",
                        "value_type": "object",
                        "packet": "runx.data.digest.v1"
                    }]
                }
            }
    })
}

pub(crate) fn digest_note_manual() -> &'static str {
    r#"---
name: digest-note
description: Compute a stable SHA-256 digest for one operator-supplied note.
---

# Digest Note

Use this skill when an operator needs a stable identity for an exact note and
does not need storage, publication, or a provider action. The result can bind a
later comparison to the same text, but it does not prove who authored the note
or that anyone acted on it.

Supply the complete note as `note`. The graph passes that value to Runx's
native `data.digest` capability using UTF-8 text encoding. Runx computes the
digest, seals the local execution receipt, and returns `digest_result` with the
algorithm and digest. No package JavaScript, network access, credentials, or
subprocess is involved.

An empty or missing note is not useful evidence; stop and ask for the exact
text. If a downstream operation needs to compare two values, route the sealed
value to the capability that owns that comparison instead of treating the
digest itself as authorization.

The terminal result proves only that Runx hashed the supplied bytes during this
run. It does not store the note, publish it, sign it for another party, or
claim an external effect.
"#
}

pub(crate) fn digest_note_manifest() -> &'static str {
    r#"skill: digest-note
version: "0.1.0"

catalog:
  kind: graph
  audience: public
  visibility: public
  role: context
  execution: read
  completion: runtime_receipt
  approval: none

harness:
  cases:
    - name: digest-note-ready
      runner: digest
      operator_journeys:
        - mode: standalone
          confusors: [extract, sign-receipt]
          request: Digest this exact note into one reusable content identity.
          expected_outcome: Hash the supplied note once and return its canonical digest in a sealed receipt.
        - mode: composed
          request: Digest the note already supplied by the upstream content step.
          expected_outcome: Reuse the exact supplied note bytes without reacquiring or rewriting them.
          prior_evidence:
            - The exact note bytes supplied by the upstream step.
          must_not_repeat:
            - Do not reacquire or rewrite the supplied note.
      inputs:
        note: one bounded note
      expect:
        status: sealed
        receipt:
          schema: runx.receipt.v1

runners:
  digest:
    default: true
    inputs:
      note:
        type: string
        required: true
        description: Exact UTF-8 note to digest.
    result_from:
      - digest
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: $input.note
          encoding: utf8_text
"#
}
