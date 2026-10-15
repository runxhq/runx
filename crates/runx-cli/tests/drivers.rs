//! A run has one driver however it was named: a run started without a
//! pinned id holds its driver lock while it runs, so a resume of it sent
//! meanwhile is refused before it runs anything.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

fn runx(root: &Path) -> Command {
    let mut command = crate::support::signed_runx_command("drivers-test-key");
    command
        .current_dir(root)
        .env("RUNX_CWD", root)
        .env("RUNX_HOME", root.join("home"));
    command
}

fn json(command: &mut Command) -> TestResult<(bool, serde_json::Value)> {
    let output = command.output()?;
    Ok((
        output.status.success(),
        serde_json::from_slice(&output.stdout)?,
    ))
}

#[test]
fn a_run_started_without_an_id_refuses_a_second_driver_while_it_runs() -> TestResult {
    let root = crate::support::temp_root("runx-drivers");
    fs::create_dir_all(&root)?;
    let root = root.as_path();
    let effects = root.join("effects.txt");
    let skill = root.join("skills").join("slow");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: slow\ndescription: Performs one effect, slowly.\n---\n# slow\n\nRun it.\n",
    )?;
    let script = format!(
        "echo performed >> '{}'; sleep 3; printf '{{\"done\":true}}'",
        effects.display()
    );
    fs::write(
        skill.join("run.yaml"),
        format!(
            "skill: slow\nrunners:\n  default:\n    default: true\n    steps:\n      - id: slow\n        tool: {}\n        outputs: {{done: boolean}}\n",
            serde_json::json!({"command": "sh", "args": ["-c", script]})
        ),
    )?;

    let original = runx(root)
        .args(["skill", &skill.to_string_lossy(), "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !effects.exists() {
        if Instant::now() > deadline {
            return Err("the run never reached its effect".into());
        }
        thread::sleep(Duration::from_millis(30));
    }

    let (_, listing) = json(runx(root).args(["history", "--json"]))?;
    let run_id = listing["runs"][0]["run_id"]
        .as_str()
        .ok_or("a listed run")?
        .to_owned();
    let (_, read) = json(runx(root).args(["history", &run_id, "--json"]))?;
    assert_eq!(read["run"]["driven"], true, "{read}");

    // A resume acknowledging the step's retry, sent while the step runs.
    let mut answers = serde_json::Map::new();
    for waited in read["waiting"].as_array().into_iter().flatten() {
        if let (Some(address), Some(digest)) = (
            waited["address"].as_str(),
            waited["request_digest"].as_str(),
        ) {
            answers.insert(
                address.to_owned(),
                serde_json::json!({"request_digest": digest, "value": {"retry": true}}),
            );
        }
    }
    let mut resume = runx(root)
        .args(["resume", &run_id, "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    resume.stdin.take().ok_or("the resume's stdin")?.write_all(
        serde_json::json!({"approvals": answers})
            .to_string()
            .as_bytes(),
    )?;
    let refused = resume.wait_with_output()?;
    assert!(!refused.status.success(), "a second driver is refused");
    let refusal = String::from_utf8_lossy(&refused.stdout);
    assert!(refusal.contains("driven"), "{refusal}");

    let finished = original.wait_with_output()?;
    let sealed: serde_json::Value = serde_json::from_slice(&finished.stdout)?;
    assert!(finished.status.success(), "{sealed}");
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    assert_eq!(
        fs::read_to_string(&effects)?.lines().count(),
        1,
        "the effect ran once"
    );
    let (_, after) = json(runx(root).args(["history", &run_id, "--json"]))?;
    assert_eq!(after["run"]["driven"], false, "{after}");
    Ok(())
}
