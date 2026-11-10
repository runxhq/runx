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
          tools: [data.digest, fs.write]
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

const WRITE_THEN_DIGEST: &str = r#"export default async function (ctx, { value }) {
  const written = await ctx.tool("fs.write", { path: "written.txt", contents: value });
  const digested = await ctx.tool("data.digest", { value, encoding: "utf8_text" });
  return { digested: [Object.keys(written).length, Object.keys(digested).length] };
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

/// Each call event of run `stream`'s steps, in order.
fn call_events<'a>(
    read: &'a serde_json::Value,
    stream: &'a str,
) -> impl Iterator<Item = &'a serde_json::Value> {
    read["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |record| {
            // A step's calls are on its own stream, `<run>/<step>@<attempt>`.
            record["stream"]
                .as_str()
                .and_then(|calls| calls.strip_prefix(stream))
                .and_then(|calls| calls.strip_prefix('/'))
                .is_some_and(|step| !step.contains('/') && step.contains('@'))
                && record["event"]["kind"] == "call"
        })
        .map(|record| &record["event"])
}

/// Each call event of run `stream`'s steps, as `(index, status)`.
fn calls(read: &serde_json::Value, stream: &str) -> Vec<(u64, String)> {
    call_events(read, stream)
        .map(|call| {
            (
                call["index"].as_u64().unwrap_or(u64::MAX),
                call["outcome"]["status"]
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

/// The retry each claimed call of the run waits on, in order.
fn claim_retries(read: &serde_json::Value) -> Vec<String> {
    call_events(read, RUN_ID)
        .filter(|call| call["outcome"]["status"] == "claimed")
        .filter_map(|call| call["outcome"]["address"].as_str().map(str::to_owned))
        .collect()
}

/// `fixed-demo` digests its value in one fixed step and writes it in the
/// next.
fn write_fixed_steps(root: &Path) -> TestResult<PathBuf> {
    let skill = root.join("fixed-demo");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: fixed-demo\ndescription: Digests a value, then writes it.\n---\n# Fixed demo\n\nDigest, then write.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    inputs:
      value: {type: string}
    steps:
      - id: read
        tool: data.digest
        inputs: {value: $input.value, encoding: utf8_text}
      - id: write
        tool: fs.write
        inputs: {path: written.txt, contents: $input.value}
"#,
    )?;
    Ok(skill)
}

/// The steps `stream` records as ended, by id, in order.
fn ended_steps(read: &serde_json::Value, stream: &str) -> Vec<String> {
    read["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|record| record["stream"] == stream && record["event"]["kind"] == "step")
        .map(|record| {
            record["event"]["step_id"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

#[test]
fn a_kill_before_a_call_is_admitted_makes_it_on_recovery_and_runs_nothing_again() -> TestResult {
    let root = crate::support::temp_root("crash-before-admission");
    let skill = write_skills(&root, TWO_DIGESTS, "          digested: {type: array}\n")?;
    crashed(&run(&root, &skill, Some("before_admission@prepare/1"))?);
    // Call 0 settled; call 1 left nothing behind.
    assert_eq!(
        calls(&history(&root)?, RUN_ID),
        [pair(0, "executing"), pair(0, "completed")]
    );

    let recovered = status(&run(&root, &skill, None)?)?;
    assert_eq!(recovered["status"], "sealed", "{recovered}");
    assert_eq!(
        calls(&history(&root)?, RUN_ID),
        [
            pair(0, "executing"),
            pair(0, "completed"),
            pair(1, "executing"),
            pair(1, "completed"),
        ]
    );
    Ok(())
}

/// A safe read, a tool or read operation that declares no effect, killed
/// after it was admitted or after it ran reruns on recovery with nobody
/// asked, and runs once to its record (D33). Changed 10 November: before
/// D33 this test held the read to a retry a person acknowledged.
#[test]
fn a_safe_read_in_flight_at_a_kill_reruns_on_recovery_with_nobody_asked() -> TestResult {
    for boundary in ["after_admission", "after_execution"] {
        let root = crate::support::temp_root(&format!("crash-read-{boundary}"));
        let skill = write_skills(&root, TWO_DIGESTS, "          digested: {type: array}\n")?;
        crashed(&run(&root, &skill, Some(&format!("{boundary}@prepare/0")))?);
        // It was executing, which waits on no retry, so a host reading the
        // run sees nothing to ask anyone.
        let read = history(&root)?;
        assert_eq!(calls(&read, RUN_ID), [pair(0, "executing")]);
        assert_eq!(read["waiting"], serde_json::json!([]), "{boundary}");

        let recovered = status(&run(&root, &skill, None)?)?;
        assert_eq!(recovered["status"], "sealed", "{boundary}: {recovered}");
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

/// A write killed after it was admitted or after it ran may have happened,
/// so recovery never runs it again unseen: it waits on the person's
/// acknowledgement of its retry, then runs again with the same input and
/// the run goes on to seal.
#[test]
fn a_write_in_flight_at_a_kill_waits_on_its_retry() -> TestResult {
    for boundary in ["after_admission", "after_execution"] {
        let root = crate::support::temp_root(&format!("crash-write-{boundary}"));
        let skill = write_skills(
            &root,
            WRITE_THEN_DIGEST,
            "          digested: {type: array}\n",
        )?;
        crashed(&run(&root, &skill, Some(&format!("{boundary}@prepare/0")))?);
        let read = history(&root)?;
        assert_eq!(calls(&read, RUN_ID), [pair(0, "claimed")]);
        assert_eq!(claim_retries(&read), ["prepare/0/retry"], "{boundary}");

        let recovered = status(&run(&root, &skill, None)?)?;
        assert_eq!(
            recovered["requests"][0]["id"], "prepare/0/retry",
            "{recovered}"
        );
        assert_eq!(recovered["requests"][0]["kind"], "retry", "{recovered}");
        let read = history(&root)?;
        assert_eq!(read["waiting"][0]["address"], "prepare/0/retry");
        assert_eq!(calls(&read, RUN_ID), [pair(0, "claimed")]);

        let resumed = acknowledge_retries(&root)?;
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
        [pair(0, "executing"), pair(0, "completed"), pair(1, "child")]
    );

    let recovered = status(&run(&root, &skill, None)?)?;
    assert_eq!(recovered["status"], "sealed", "{recovered}");
    let read = history(&root)?;
    assert_eq!(steps(&read, &child), 2, "the child ran nothing again");
    assert_eq!(
        calls(&read, RUN_ID),
        [
            pair(0, "executing"),
            pair(0, "completed"),
            pair(1, "child"),
            pair(1, "completed"),
        ]
    );
    Ok(())
}

/// A fixed step recovers by the same rule as a call (D33): a safe read
/// killed after it was admitted or after it ran reruns with nobody asked,
/// and a write waits on its retry, then runs again on its acknowledgement.
#[test]
fn a_fixed_step_in_flight_at_a_kill_reruns_if_it_only_reads_and_waits_on_its_retry_if_it_writes()
-> TestResult {
    for boundary in ["after_admission", "after_execution"] {
        let root = crate::support::temp_root(&format!("crash-fixed-read-{boundary}"));
        let skill = write_fixed_steps(&root)?;
        crashed(&run(&root, &skill, Some(&format!("{boundary}@read")))?);
        assert_eq!(
            history(&root)?["waiting"],
            serde_json::json!([]),
            "{boundary}"
        );

        let recovered = status(&run(&root, &skill, None)?)?;
        assert_eq!(recovered["status"], "sealed", "{boundary}: {recovered}");
        assert_eq!(
            ended_steps(&history(&root)?, RUN_ID),
            ["read", "write"],
            "{boundary}"
        );
    }
    for boundary in ["after_admission", "after_execution"] {
        let root = crate::support::temp_root(&format!("crash-fixed-write-{boundary}"));
        let skill = write_fixed_steps(&root)?;
        crashed(&run(&root, &skill, Some(&format!("{boundary}@write")))?);

        let recovered = status(&run(&root, &skill, None)?)?;
        assert_eq!(
            recovered["requests"][0]["id"], "write/retry",
            "{boundary}: {recovered}"
        );
        let resumed = acknowledge_retries(&root)?;
        assert_eq!(resumed["status"], "sealed", "{boundary}: {resumed}");
        assert_eq!(fs::read_to_string(root.join("written.txt"))?, "hello");
        assert_eq!(
            ended_steps(&history(&root)?, RUN_ID),
            ["read", "write"],
            "{boundary}"
        );
    }
    Ok(())
}

const WRITE_BESIDE_A_REFUSAL: &str = r#"export default async function (ctx, { value }) {
  await Promise.all([
    ctx.tool("fs.write", { path: "written.txt", contents: value }),
    ctx.tool("fs.read", { path: "secret.txt" }),
  ]);
  return { digested: [] };
}
"#;

/// A script that ends while an effect it made may have happened waits for
/// that effect to settle before its step fails. Killed once its write was
/// admitted, the run's recovery has the script throw when the write's
/// sibling is refused, and the step holds on the write's retry; once the
/// write runs again, the step fails with the script's own reason.
#[test]
fn a_script_that_ends_with_an_effect_unsettled_waits_for_it_then_fails_with_its_reason()
-> TestResult {
    let root = crate::support::temp_root("crash-ended-write");
    let skill = write_skills(
        &root,
        WRITE_BESIDE_A_REFUSAL,
        "          digested: {type: array}\n",
    )?;
    crashed(&run(&root, &skill, Some("after_admission@prepare/0"))?);

    let held = status(&run(&root, &skill, None)?)?;
    assert_eq!(held["requests"][0]["id"], "prepare/0/retry", "{held}");
    assert!(!root.join("written.txt").exists());

    let ended = acknowledge_retries(&root)?;
    assert_eq!(ended["outcome"], "failed", "{ended}");
    assert!(
        ended["result"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("fs.read")),
        "{ended}"
    );
    assert_eq!(fs::read_to_string(root.join("written.txt"))?, "hello");
    let settled = calls(&history(&root)?, RUN_ID);
    assert_eq!(
        settled.last(),
        Some(&pair(0, "completed")),
        "the write settled before the step failed: {settled:?}"
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
/// call recorded as done isn't run again, and a safe read admitted but not
/// recorded reruns by itself (D33). Changed 10 November: before D33 the
/// unrecorded read waited on its retry.
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
            [pair(0, "executing"), pair(1, "executing")],
            "{crash}"
        );

        let sealed = status(&run(&root, &skill, None)?)?;
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

/// A script that digests `calls` values four at a time, each four at once,
/// and returns the digests in position order and the positions in the order
/// they settled.
const BATCHED_DIGESTS: &str = r#"export default async function (ctx, { calls }) {
  const digests = [];
  const settled = [];
  for (let first = 0; first < calls; first += 4) {
    const batch = [];
    for (let index = first; index < Math.min(first + 4, calls); index += 1) {
      batch.push(
        ctx.tool("data.digest", { value: `call ${index}`, encoding: "utf8_text" }).then((made) => {
          settled.push(index);
          return made.digest_result.digest;
        }),
      );
    }
    digests.push(...(await Promise.all(batch)));
  }
  return { digests, settled };
}
"#;

/// `batched` runs [`BATCHED_DIGESTS`] over its `calls` input.
fn write_batched(root: &Path) -> TestResult<PathBuf> {
    let skill = root.join("batched");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: batched\ndescription: Digests a batch of values.\n---\n# Batched\n\nDigest them.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  digest:
    inputs:
      calls: {type: integer}
    steps:
      - id: prepare
        script: {module: batch.mjs}
        inputs:
          calls: $input.calls
        calls:
          tools: [data.digest]
        max_calls: 1000
        concurrency: 4
        outputs:
          digests: {type: array}
          settled: {type: array}
"#,
    )?;
    fs::write(skill.join("batch.mjs"), BATCHED_DIGESTS)?;
    Ok(skill)
}

/// Run `skill` over `calls` under the pinned run id, crashing at `crash`
/// when given, and the work the process counted.
fn run_counted(
    root: &Path,
    skill: &Path,
    calls: usize,
    crash: Option<&str>,
) -> TestResult<(Output, Option<serde_json::Value>)> {
    let counted = root.join("counts.json");
    let _ignored = fs::remove_file(&counted);
    let mut command = unsigned_runx_command_at(root);
    command
        .args([
            "skill",
            skill.to_str().ok_or("non-utf8 skill dir")?,
            "-i",
            &format!("calls={calls}"),
            "--run-id",
            RUN_ID,
            "--receipt-dir",
            root.join("receipts")
                .to_str()
                .ok_or("non-utf8 receipt dir")?,
            "--json",
        ])
        .env("RUNX_PERF_COUNTS_PATH", &counted);
    if let Some(crash) = crash {
        command.env("RUNX_CRASH_AT", crash);
    }
    let output = command.output()?;
    let counts = fs::read(&counted)
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?;
    Ok((output, counts))
}

/// The run as a host reads it, every page of it.
fn whole_history(root: &Path) -> TestResult<serde_json::Value> {
    let mut whole = history(root)?;
    while whole["more"] == true {
        let after = whole["cursor"]
            .as_str()
            .ok_or("a read with more names its cursor")?;
        let read = unsigned_runx_command_at(root)
            .args(["history", RUN_ID, "--after", after, "--json"])
            .output()?;
        assert!(
            read.status.success(),
            "{}",
            String::from_utf8_lossy(&read.stderr)
        );
        let mut page: serde_json::Value = serde_json::from_slice(&read.stdout)?;
        let events = page["events"]
            .as_array_mut()
            .map(std::mem::take)
            .unwrap_or_default();
        whole["events"]
            .as_array_mut()
            .ok_or("a read holds its events")?
            .extend(events);
        whole["more"] = page["more"].take();
        whole["cursor"] = page["cursor"].take();
    }
    Ok(whole)
}

/// What a sealed run returned: its result, or, when the CLI wrote it to an
/// artifact for its size, the artifact's content.
fn result_of(sealed: &serde_json::Value) -> TestResult<serde_json::Value> {
    let result = &sealed["result"];
    match result["artifact"]["path"].as_str() {
        Some(path) => Ok(serde_json::from_slice(&fs::read(path)?)?),
        None => Ok(result.clone()),
    }
}

/// The run's ledger as it ended: each call's target, its input's digest and
/// how it settled, by position, with each output by its digest. A receipt
/// isn't in it, since each run seals its own.
fn ledger(read: &serde_json::Value) -> std::collections::BTreeMap<u64, serde_json::Value> {
    call_events(read, RUN_ID)
        .map(|call| {
            let outcome = &call["outcome"];
            let outputs = outcome["outputs"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|output| {
                    serde_json::json!({
                        "output": output["output"],
                        "digest": output["artifact"]["digest"],
                    })
                })
                .collect::<Vec<_>>();
            (
                call["index"].as_u64().unwrap_or(u64::MAX),
                serde_json::json!({
                    "target": call["target"],
                    "input_digest": call["input_digest"],
                    "status": outcome["status"],
                    "succeeded": outcome["succeeded"],
                    "outputs": outputs,
                }),
            )
        })
        .collect()
}

fn counted(counts: &serde_json::Value, name: &str) -> u64 {
    counts[name].as_u64().unwrap_or(u64::MAX)
}

/// The replay proof (script-step.md, §4.11, D36), counted at 10, 100 and
/// 1,000 calls. A live worker runs the script once, from its beginning, and
/// hands it each settlement once: live running never replays. A run killed
/// mid-batch, between one batch's calls settling and the next one's being
/// admitted, is recovered by one pass over its record, which hands the
/// script each recorded settlement once, in the order the calls settled, and
/// then goes on live: its output is a run's that never crashed. Recovery
/// takes the run whole once, and settling a call costs the same at every
/// size, live or recovered.
#[test]
fn a_lost_worker_is_rebuilt_from_its_record_and_live_running_never_replays() -> TestResult {
    let mut live = Vec::new();
    let mut recovered = Vec::new();
    for size in [10_usize, 100, 1000] {
        let reference = crate::support::temp_root(&format!("replay-live-{size}"));
        let skill = write_batched(&reference)?;
        let (output, counts) = run_counted(&reference, &skill, size, None)?;
        let sealed = status(&output)?;
        assert_eq!(sealed["status"], "sealed", "{sealed}");
        let counts = counts.ok_or("the live run wrote its counts")?;
        assert_eq!(counted(&counts, "script_passes"), 1, "{size}: {counts}");
        assert_eq!(
            counted(&counts, "script_settlements_delivered"),
            size as u64,
            "{size}: {counts}"
        );
        live.push((size, counts));

        // Killed as the batch after the first half is admitted.
        let killed_at = size / 2 / 4 * 4;
        let root = crate::support::temp_root(&format!("replay-recovered-{size}"));
        let skill = write_batched(&root)?;
        let (output, _) = run_counted(
            &root,
            &skill,
            size,
            Some(&format!("before_admission@prepare/{killed_at}")),
        )?;
        crashed(&output);
        let completed = |read: &serde_json::Value| {
            calls(read, RUN_ID)
                .into_iter()
                .filter(|(_, status)| status == "completed")
                .map(|(index, _)| index)
                .collect::<Vec<_>>()
        };
        assert_eq!(completed(&whole_history(&root)?).len(), killed_at);

        let (output, counts) = run_counted(&root, &skill, size, None)?;
        let resumed = status(&output)?;
        assert_eq!(resumed["status"], "sealed", "{resumed}");
        let (result, uncrashed) = (result_of(&resumed)?, result_of(&sealed)?);
        assert_eq!(
            result["digests"], uncrashed["digests"],
            "{size}: the recovered run's output is the uncrashed run's"
        );
        let settled = result["settled"]
            .as_array()
            .ok_or("the script returned the order its calls settled")?
            .iter()
            .filter_map(serde_json::Value::as_u64)
            .collect::<Vec<_>>();
        assert_eq!(
            settled,
            completed(&whole_history(&root)?),
            "{size}: the script saw its calls settle in the order the record holds"
        );
        // The batch run live and the same batch run by replay leave matching
        // ledgers: every call at its position, with its input and how it
        // settled, and the same outputs.
        let (replayed, uncrashed) = (
            ledger(&whole_history(&root)?),
            ledger(&whole_history(&reference)?),
        );
        assert_eq!(replayed.len(), size, "{size}");
        assert_eq!(
            replayed, uncrashed,
            "{size}: the live and replayed ledgers differ"
        );
        let counts = counts.ok_or("the recovery wrote its counts")?;
        assert_eq!(counted(&counts, "script_passes"), 1, "{size}: {counts}");
        assert_eq!(
            counted(&counts, "script_settlements_delivered"),
            size as u64,
            "{size}: each settlement, recorded or new, delivered once: {counts}"
        );
        // Recovery reads each event of the run's stream, and of each of its
        // steps' calls streams, once.
        assert_eq!(
            counted(&counts, "most_reads_of_one_stream_event"),
            1,
            "{size}: {counts}"
        );
        assert_eq!(
            counted(&counts, "stream_events_read_again"),
            0,
            "{size}: {counts}"
        );
        recovered.push((size, counts));
    }
    // Settling a call costs the same at every size: what the store reads and
    // writes, and the runs it takes whole, each a line through the three
    // sizes, against the calls a live run made, and against the calls a
    // recovered run read back and the calls it then made.
    let line = |name: &str, points: &[(u64, u64)]| {
        let [(x0, y0), (x1, y1), (x2, y2)] = points else {
            return;
        };
        assert_eq!(
            (y1 - y0) * (x2 - x0),
            (y2 - y0) * (x1 - x0),
            "{name} at 10, 100 and 1,000 calls: {points:?}"
        );
    };
    let points = |runs: &[(usize, serde_json::Value)], count: &str, calls: fn(u64) -> u64| {
        runs.iter()
            .map(|(size, counts)| (calls(*size as u64), counted(counts, count)))
            .collect::<Vec<_>>()
    };
    let made = |size: u64| size;
    let read_back = |size: u64| size / 2 / 4 * 4;
    let made_after = |size: u64| size - size / 2 / 4 * 4;
    for count in ["store_rows_read", "store_writes", "run_materializations"] {
        line(&format!("live {count}"), &points(&live, count, made));
    }
    line(
        "recovered store_rows_read",
        &points(&recovered, "store_rows_read", read_back),
    );
    line(
        "recovered store_writes",
        &points(&recovered, "store_writes", made_after),
    );
    line(
        "recovered run_materializations",
        &points(&recovered, "run_materializations", made),
    );
    Ok(())
}

/// A script changed between its crash and its recovery is a failed recovery
/// that acts on nothing: the run is pinned to the package it started from,
/// so recovery under another refuses before any call is replayed or made,
/// and nothing is written.
#[test]
fn a_script_changed_after_a_kill_recovers_nothing() -> TestResult {
    let root = crate::support::temp_root("crash-changed");
    let skill = write_batched(&root)?;
    crashed(&run_counted(&root, &skill, 10, Some("before_admission@prepare/4"))?.0);
    let before = calls(&whole_history(&root)?, RUN_ID);
    fs::write(
        skill.join("batch.mjs"),
        BATCHED_DIGESTS.replace("value: `call ${index}`", "value: `changed ${index}`"),
    )?;

    let (output, counts) = run_counted(&root, &skill, 10, None)?;
    let refused = status(&output)?;
    assert_eq!(refused["status"], "failure", "{refused}");
    let counts = counts.ok_or("the refused recovery wrote its counts")?;
    assert_eq!(counted(&counts, "script_passes"), 0, "{counts}");
    assert_eq!(counted(&counts, "store_writes"), 0, "{counts}");
    assert_eq!(calls(&whole_history(&root)?, RUN_ID), before);
    Ok(())
}
