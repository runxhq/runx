//! A kill at every boundary of a script's call, and the run's recovery when
//! its start is invoked again under the same run id (plan §9's control-flow
//! row). Each crash is a real abort of the `runx` process at the named point,
//! so recovery reads exactly what a kill there leaves on disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support::unsigned_runx_command_at;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// `confirm-child` digests its value in two steps; `script-demo` makes the
/// calls `script` makes, under `calls` that admit both.
fn write_skills(root: &Path, script: &str, outputs: &str) -> TestResult<PathBuf> {
    let child = root.join("confirm-child");
    fs::create_dir_all(&child)?;
    fs::write(
        child.join("SKILL.md"),
        "---\nname: confirm-child\ndescription: Digests a value twice.\n---\n# Confirm child\n\nDigest the value.\n",
    )?;
    fs::write(
        child.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  confirm:
    inputs:
      value: {type: string}
    steps:
      - id: first
        tool: data.digest
        inputs: {value: $input.value, encoding: utf8_text}
      - id: second
        tool: data.digest
        inputs: {value: $input.value, encoding: utf8_text}
"#,
    )?;
    let skill = root.join("script-demo");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: script-demo\ndescription: Digests a value and has it confirmed.\n---\n# Script demo\n\nDigest, then confirm.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        format!(
            r#"minimum_runx: 0.12.0
methods:
  prepare:
    inputs:
      value: {{type: string}}
    steps:
      - id: prepare
        script: {{module: prepare.mjs}}
        inputs:
          value: $input.value
        calls:
          tools: [data.digest]
          skills: [../confirm-child]
        max_calls: 4
        concurrency: 2
        outputs:
{outputs}"#
        ),
    )?;
    fs::write(skill.join("prepare.mjs"), script)?;
    Ok(skill)
}

const TWO_DIGESTS: &str = r#"export default async function (ctx, { value }) {
  const first = await ctx.tool("data.digest", { value, encoding: "utf8_text" });
  const second = await ctx.tool("data.digest", { value: value + "!", encoding: "utf8_text" });
  return { digested: [Object.keys(first).length, Object.keys(second).length] };
}
"#;

const DIGEST_THEN_CONFIRM: &str = r#"export default async function (ctx, { value }) {
  const digested = await ctx.tool("data.digest", { value, encoding: "utf8_text" });
  const confirmed = await ctx.skill("../confirm-child", { value });
  return { digested: Object.keys(digested).length, confirmed: Object.keys(confirmed).length };
}
"#;

const RUN_ID: &str = "crash_points_run";

/// Run `skill` under the pinned run id, crashing at `crash` when given.
fn run(root: &Path, skill: &Path, crash: Option<&str>) -> TestResult<Output> {
    let mut command = unsigned_runx_command_at(root);
    command.args([
        "skill",
        skill.to_str().ok_or("non-utf8 skill dir")?,
        "-i",
        "value=hello",
        "--run-id",
        RUN_ID,
        "--receipt-dir",
        root.join("receipts")
            .to_str()
            .ok_or("non-utf8 receipt dir")?,
        "--json",
    ]);
    if let Some(crash) = crash {
        command.env("RUNX_CRASH_AT", crash);
    }
    Ok(command.output()?)
}

/// The run as a host reads it.
fn history(root: &Path) -> TestResult<serde_json::Value> {
    let read = unsigned_runx_command_at(root)
        .args(["history", RUN_ID, "--json"])
        .output()?;
    assert!(
        read.status.success(),
        "{}",
        String::from_utf8_lossy(&read.stderr)
    );
    Ok(serde_json::from_slice(&read.stdout)?)
}

/// Each call event of `stream`, as `(index, status)`.
fn calls(read: &serde_json::Value, stream: &str) -> Vec<(u64, String)> {
    read["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| record["stream"] == stream && record["event"]["kind"] == "call")
        .map(|record| {
            (
                record["event"]["index"].as_u64().unwrap_or(u64::MAX),
                record["event"]["outcome"]["status"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

/// How many step events `stream` records.
fn steps(read: &serde_json::Value, stream: &str) -> usize {
    read["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| record["stream"] == stream && record["event"]["kind"] == "step")
        .count()
}

fn status(output: &Output) -> TestResult<serde_json::Value> {
    serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "{error}: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
        .into()
    })
}

fn crashed(output: &Output) {
    assert!(
        !output.status.success() && output.stdout.is_empty(),
        "the run aborted at its crash point: status={:?} stdout={}",
        output.status,
        String::from_utf8_lossy(&output.stdout)
    );
}

fn pair(index: u64, status: &str) -> (u64, String) {
    (index, status.to_owned())
}

#[test]
fn a_kill_before_a_call_is_admitted_makes_it_on_recovery_and_runs_nothing_again() -> TestResult {
    let root = crate::support::temp_root("crash-before-admission");
    let skill = write_skills(&root, TWO_DIGESTS, "          digested: {type: array}\n")?;
    crashed(&run(&root, &skill, Some("before_admission@prepare/1"))?);
    // Call 0 settled; call 1 left nothing behind.
    assert_eq!(
        calls(&history(&root)?, RUN_ID),
        [pair(0, "claimed"), pair(0, "completed")]
    );

    let recovered = status(&run(&root, &skill, None)?)?;
    assert_eq!(recovered["status"], "sealed", "{recovered}");
    assert_eq!(
        calls(&history(&root)?, RUN_ID),
        [
            pair(0, "claimed"),
            pair(0, "completed"),
            pair(1, "claimed"),
            pair(1, "completed"),
        ]
    );
    Ok(())
}

#[test]
fn a_kill_after_a_call_is_admitted_or_has_run_waits_on_its_retry() -> TestResult {
    for boundary in ["after_admission", "after_execution"] {
        let root = crate::support::temp_root(&format!("crash-{boundary}"));
        let skill = write_skills(&root, TWO_DIGESTS, "          digested: {type: array}\n")?;
        crashed(&run(&root, &skill, Some(&format!("{boundary}@prepare/0")))?);
        assert_eq!(calls(&history(&root)?, RUN_ID), [pair(0, "claimed")]);

        // The call may have run, so recovery never runs it again unseen: it
        // waits on the person's acknowledgement of its retry.
        let recovered = status(&run(&root, &skill, None)?)?;
        assert_eq!(
            recovered["requests"][0]["id"], "prepare/0/retry",
            "{recovered}"
        );
        assert_eq!(recovered["requests"][0]["kind"], "retry", "{recovered}");
        let read = history(&root)?;
        assert_eq!(read["waiting"][0]["address"], "prepare/0/retry");
        assert_eq!(calls(&read, RUN_ID), [pair(0, "claimed")]);

        // Acknowledged, it runs again with the same input, and the run goes
        // on to seal.
        let answers = root.join("retry.json");
        fs::write(
            &answers,
            crate::support::bind_resume(
                &read,
                serde_json::json!({"approvals": {"prepare/0/retry": {"value": {"retry": true}}}}),
            )?
            .to_string(),
        )?;
        let resumed = unsigned_runx_command_at(&root)
            .args(["resume", RUN_ID])
            .arg(&answers)
            .arg("--receipt-dir")
            .arg(root.join("receipts"))
            .arg("--json")
            .output()?;
        let resumed = status(&resumed)?;
        assert_eq!(resumed["status"], "sealed", "{boundary}: {resumed}");
        let settled = calls(&history(&root)?, RUN_ID);
        assert_eq!(
            settled
                .iter()
                .filter(|call| **call == pair(0, "completed"))
                .count(),
            1,
            "{boundary}: {settled:?}"
        );
    }
    Ok(())
}

#[test]
fn a_kill_after_a_child_sealed_records_it_on_recovery_without_running_it_again() -> TestResult {
    let root = crate::support::temp_root("crash-after-child");
    let skill = write_skills(
        &root,
        DIGEST_THEN_CONFIRM,
        "          digested: {type: number}\n          confirmed: {type: number}\n",
    )?;
    crashed(&run(&root, &skill, Some("after_execution@prepare/1"))?);
    let read = history(&root)?;
    let child = format!("{RUN_ID}/prepare/1");
    // The child sealed with both its steps; its parent's call still names it.
    assert_eq!(steps(&read, &child), 2);
    assert_eq!(
        calls(&read, RUN_ID),
        [pair(0, "claimed"), pair(0, "completed"), pair(1, "child")]
    );

    let recovered = status(&run(&root, &skill, None)?)?;
    assert_eq!(recovered["status"], "sealed", "{recovered}");
    let read = history(&root)?;
    assert_eq!(steps(&read, &child), 2, "the child ran nothing again");
    assert_eq!(
        calls(&read, RUN_ID),
        [
            pair(0, "claimed"),
            pair(0, "completed"),
            pair(1, "child"),
            pair(1, "completed"),
        ]
    );
    Ok(())
}

/// No build that ships has crash points: the feature is off by default, and
/// only `runx-cli`'s dev-dependency turns it on.
#[test]
fn only_a_test_build_has_crash_points() -> TestResult {
    let crates = crate::support::repo_root()?.join("crates");
    let runtime = fs::read_to_string(crates.join("runx-runtime/Cargo.toml"))?
        .parse::<toml_edit::DocumentMut>()?;
    let default = runtime["features"]["default"]
        .as_array()
        .ok_or("runx-runtime declares its default features")?;
    assert!(
        !default
            .iter()
            .any(|feature| feature.as_str() == Some("crash-points")),
        "crash points are off by default"
    );
    for entry in fs::read_dir(&crates)? {
        let manifest = entry?.path().join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let document = fs::read_to_string(&manifest)?.parse::<toml_edit::DocumentMut>()?;
        let enables = |table: &str| {
            document
                .get(table)
                .and_then(|table| table.get("runx-runtime"))
                .and_then(|dependency| dependency.get("features"))
                .and_then(toml_edit::Item::as_array)
                .is_some_and(|features| {
                    features
                        .iter()
                        .any(|feature| feature.as_str() == Some("crash-points"))
                })
        };
        assert!(
            !enables("dependencies") && !enables("build-dependencies"),
            "{} enables crash points in a build that ships",
            manifest.display()
        );
        // The runtime defines the feature; no other crate may pass it on.
        if manifest.ends_with("runx-runtime/Cargo.toml") {
            continue;
        }
        let features = document
            .get("features")
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            !features.contains("crash-points"),
            "{} exposes crash points as a feature of its own",
            manifest.display()
        );
    }
    Ok(())
}

const TWO_DIGESTS_AT_ONCE: &str = r#"export default async function (ctx, { value }) {
  const digested = await Promise.all([
    ctx.tool("data.digest", { value, encoding: "utf8_text" }),
    ctx.tool("data.digest", { value: value + "!", encoding: "utf8_text" }),
  ]);
  return { digested: digested.map((each) => Object.keys(each).length) };
}
"#;

/// Acknowledge every retry the run waits on, in one resume.
fn acknowledge_retries(root: &Path) -> TestResult<serde_json::Value> {
    let read = history(root)?;
    let mut approvals = serde_json::Map::new();
    for waited in read["waiting"].as_array().into_iter().flatten() {
        if let Some(address) = waited["address"].as_str() {
            approvals.insert(
                address.to_owned(),
                serde_json::json!({"value": {"retry": true}}),
            );
        }
    }
    let answers = root.join("retries.json");
    fs::write(
        &answers,
        crate::support::bind_resume(&read, serde_json::json!({"approvals": approvals}))?
            .to_string(),
    )?;
    let resumed = unsigned_runx_command_at(root)
        .args(["resume", RUN_ID])
        .arg(&answers)
        .arg("--receipt-dir")
        .arg(root.join("receipts"))
        .arg("--json")
        .output()?;
    status(&resumed)
}

/// Killed while two calls are in flight, each recovers by its own record: a
/// call recorded as done isn't run again, and one admitted but not recorded
/// waits on its retry.
#[test]
fn a_kill_with_calls_in_flight_recovers_each_by_its_own_record() -> TestResult {
    for crash in ["after_admission@prepare/1", "after_execution@prepare/0"] {
        let root = crate::support::temp_root("crash-in-flight");
        let skill = write_skills(
            &root,
            TWO_DIGESTS_AT_ONCE,
            "          digested: {type: array}\n",
        )?;
        crashed(&run(&root, &skill, Some(crash))?);
        let left = calls(&history(&root)?, RUN_ID);
        // Both were admitted, in position order, before either ran.
        assert_eq!(
            left[..2],
            [pair(0, "claimed"), pair(1, "claimed")],
            "{crash}"
        );
        let done_before = left
            .iter()
            .filter(|(_, status)| status == "completed")
            .map(|(index, _)| *index)
            .collect::<Vec<_>>();

        let recovered = status(&run(&root, &skill, None)?)?;
        let waiting = recovered["requests"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|request| request["id"].as_str().map(str::to_owned))
            .collect::<Vec<_>>();
        let expected = [0, 1]
            .iter()
            .filter(|index| !done_before.contains(index))
            .map(|index| format!("prepare/{index}/retry"))
            .collect::<Vec<_>>();
        assert_eq!(waiting, expected, "{crash}: {recovered}");

        let sealed = acknowledge_retries(&root)?;
        assert_eq!(sealed["status"], "sealed", "{crash}: {sealed}");
        let settled = calls(&history(&root)?, RUN_ID);
        for index in [0, 1] {
            assert_eq!(
                settled
                    .iter()
                    .filter(|call| **call == pair(index, "completed"))
                    .count(),
                1,
                "{crash}: call {index} ran once: {settled:?}"
            );
        }
    }
    Ok(())
}
