//! A kill at every boundary of a script's call, and the run's recovery when
//! its start is invoked again under the same run id (plan §9's control-flow
//! row). Each crash is a real abort of the `runx` process at the named point,
//! so recovery reads exactly what a kill there leaves on disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support::unsigned_runx_command_at;

mod revisable_outreach;

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
    answer_retries(root, true)
}

/// Answer every retry the run waits on `{"retry": retry}`, in one resume.
fn answer_retries(root: &Path, retry: bool) -> TestResult<serde_json::Value> {
    let read = history(root)?;
    let mut approvals = serde_json::Map::new();
    for waited in read["waiting"].as_array().into_iter().flatten() {
        if let Some(address) = waited["address"].as_str() {
            approvals.insert(
                address.to_owned(),
                serde_json::json!({"value": {"retry": retry}}),
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

/// A sealed script run invoked again under its id returns its seal: the
/// same receipt, with no pass of its script, no call and nothing written.
#[test]
fn a_sealed_script_run_invoked_again_returns_its_seal() -> TestResult {
    let root = crate::support::temp_root("sealed-again");
    let skill = write_batched(&root)?;
    let (output, _) = run_counted(&root, &skill, 10, None)?;
    let sealed = status(&output)?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    let before = whole_history(&root)?;

    let (output, counts) = run_counted(&root, &skill, 10, None)?;
    let again = status(&output)?;
    assert_eq!(again["status"], "sealed", "{again}");
    assert_eq!(again["receipt_id"], sealed["receipt_id"], "{again}");
    assert_eq!(result_of(&again)?, result_of(&sealed)?);
    let counts = counts.ok_or("the second invocation wrote its counts")?;
    assert_eq!(counted(&counts, "script_passes"), 0, "{counts}");
    assert_eq!(counted(&counts, "store_writes"), 0, "{counts}");
    assert_eq!(whole_history(&root)?["events"], before["events"]);
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

/// A local copy of `fixture/compute`, whose process runs its jobs in the
/// workspace. It enforces a start's key, as the connector declares: each
/// start request it receives is appended to `compute/requests`, and only a
/// key it hasn't seen starts a job, appended to `compute/starts`; a key it
/// has seen returns that key's job, recovered. A start's readback reads the
/// key's job back. Each inspection is appended to `compute/inspections`, and
/// the second finds the job completed.
const LOCAL_COMPUTE: &str = r#"schema: runx.connector.v1
name: fixture/compute
title: Fixture Compute
description: Start jobs in one account of a compute fixture, and read how each is going.
auth:
  credentials:
    access_key:
      kind: api_key
      fields:
        - {name: token, label: compute token}
      delivery: {env: COMPUTE_TOKEN}
      audience: ["*.compute.fixture.invalid"]
placements:
  local:
    auth: [credentials]
operations:
  jobs.start:
    description: Start one job on one stored object.
    verb: write
    scopes: [jobs.execute]
    credentials: [access_key]
    target:
      template: {"$argument": "/target"}
    guarantees:
      provider_idempotency: enforced
    result_fields: [job_ref, status, output_available, failure_reason, recovered]
    implementation:
      process:
        tool: compute.start
        readback:
          matches:
            - expected: {source: acknowledgement, path: job_ref}
              observed: job_ref
            - expected: {source: argument, path: target}
              observed: target
  jobs.inspect:
    description: Read how one job is going, `running` until it ends `completed` or `failed`.
    verb: read
    scopes: [jobs.read]
    credentials: [access_key]
    target:
      template: {"$argument": "/job_ref"}
    result_fields: [job_ref, status, output_available, failure_reason]
    implementation:
      process:
        tool: compute.inspect
"#;

const COMPUTE_START: &str = r#"#!/bin/sh
set -eu
provider="$RUNX_CWD/compute"
mkdir -p "$provider"
touch "$provider/starts"
key="$RUNX_INPUT_IDEMPOTENCY_KEY"
job=0
known=false
while read -r started target; do
  job=$((job + 1))
  if [ "$started" = "$key" ]; then
    known=true
    break
  fi
done < "$provider/starts"
if [ "$RUNX_INPUT_PHASE" = readback ]; then
  if [ "$known" = false ]; then
    echo "no job was started with this key" >&2
    exit 1
  fi
  printf '{"job_ref":"compute://job/%s","target":"%s"}\n' "$job" "$target"
  exit 0
fi
printf '%s\n' "$key" >> "$provider/requests"
if [ "$known" = false ]; then
  printf '%s %s\n' "$key" "$RUNX_INPUT_TARGET" >> "$provider/starts"
  job=$((job + 1))
fi
printf '{"job_ref":"compute://job/%s","status":"started","output_available":false,"failure_reason":"","recovered":%s}\n' "$job" "$known"
"#;

const COMPUTE_INSPECT: &str = r#"#!/bin/sh
set -eu
provider="$RUNX_CWD/compute"
printf '%s\n' "$RUNX_INPUT_JOB_REF" >> "$provider/inspections"
checks=0
while read -r line; do
  checks=$((checks + 1))
done < "$provider/inspections"
status=completed
available=true
if [ "$checks" -lt 2 ]; then
  status=running
  available=false
fi
printf '{"job_ref":"%s","status":"%s","output_available":%s,"failure_reason":""}\n' "$RUNX_INPUT_JOB_REF" "$status" "$available"
"#;

/// The object every job here runs on.
const STORED_OBJECT: &str = "compute://account/306607894404/object/audio-9999";

const COMPUTE_START_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "compute.start",
  "version": "0.1.0",
  "description": "Start a job for a key not seen before, or read a key's job back.",
  "process": {"command": "sh", "args": ["./start.sh"]},
  "inputs": {
    "target": {"type": "string", "description": "The stored object."},
    "phase": {"type": "string", "description": "perform or readback."},
    "idempotency_key": {"type": "string", "description": "The key the start is sent with."},
    "acknowledgement": {"type": "object", "optional": true, "description": "What the start answered, which its readback confirms."}
  }
}
"#;

const COMPUTE_INSPECT_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "compute.inspect",
  "version": "0.1.0",
  "description": "Read how one job is going.",
  "process": {"command": "sh", "args": ["./inspect.sh"]},
  "inputs": {
    "job_ref": {"type": "string", "description": "The job."}
  }
}
"#;

/// A project holding `fixtures/skills/compute-job` and [`LOCAL_COMPUTE`].
fn compute_project(root: &Path) -> TestResult<PathBuf> {
    let connector = root.join("connectors/fixture/compute");
    for (tool, manifest, script) in [
        ("start", COMPUTE_START_TOOL, COMPUTE_START),
        ("inspect", COMPUTE_INSPECT_TOOL, COMPUTE_INSPECT),
    ] {
        let directory = connector.join("tools/compute").join(tool);
        fs::create_dir_all(&directory)?;
        fs::write(directory.join("manifest.json"), manifest)?;
        fs::write(directory.join(format!("{tool}.sh")), script)?;
    }
    fs::write(connector.join("connector.yaml"), LOCAL_COMPUTE)?;
    let skill = root.join("skills/compute-job");
    crate::support::copy_dir(
        &crate::support::repo_root()?.join("fixtures/skills/compute-job"),
        &skill,
    )?;
    Ok(skill)
}

/// A command in `root` that holds the local compute fixture's credential.
fn compute_command(root: &Path) -> std::process::Command {
    let mut command = unsigned_runx_command_at(root);
    command.env("COMPUTE_TOKEN", "local-compute-token");
    command
}

/// Start compute-job's `run` on [`STORED_OBJECT`] under the pinned run id,
/// waiting `interval_seconds` before each check, crashing at `crash` when
/// given.
fn run_job(
    root: &Path,
    skill: &Path,
    interval_seconds: u32,
    crash: Option<&str>,
) -> TestResult<serde_json::Value> {
    let mut command = compute_command(root);
    command.args([
        "skill",
        &format!("{}#run", skill.to_str().ok_or("non-utf8 skill dir")?),
        "-i",
        &format!("target={STORED_OBJECT}"),
        "-i",
        &format!("interval_seconds={interval_seconds}"),
        "--run-id",
        RUN_ID,
        "--receipt-dir",
        root.join("receipts")
            .to_str()
            .ok_or("non-utf8 receipt dir")?,
        "--json",
    ]);
    match crash {
        Some(crash) => {
            crashed(&command.env("RUNX_CRASH_AT", crash).output()?);
            Ok(serde_json::Value::Null)
        }
        None => status(&command.output()?),
    }
}

/// Drive the run again, answering nothing.
fn drive(root: &Path) -> TestResult<serde_json::Value> {
    let nothing = root.join("nothing.json");
    fs::write(&nothing, "{}")?;
    status(
        &compute_command(root)
            .args(["resume", RUN_ID])
            .arg(&nothing)
            .arg("--receipt-dir")
            .arg(root.join("receipts"))
            .arg("--json")
            .output()?,
    )
}

/// The lines the local compute fixture appended to `name`.
fn provider_lines(root: &Path, name: &str) -> TestResult<Vec<String>> {
    match fs::read_to_string(root.join("compute").join(name)) {
        Ok(lines) => Ok(lines.lines().map(str::to_owned).collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// The wait a paused run reports, as `(id, until)`.
fn wait_of(paused: &serde_json::Value) -> (String, i64) {
    let requests = paused["requests"].as_array().cloned().unwrap_or_default();
    assert!(
        matches!(&requests[..], [request] if request["kind"] == "wait"),
        "the run waits on one wait: {paused}"
    );
    (
        requests[0]["id"].as_str().unwrap_or_default().to_owned(),
        requests[0]["until"].as_i64().unwrap_or_default(),
    )
}

/// compute-job's `run` starts its job, then pauses on its wait before each
/// check. A drive before the wait's time settles nothing and checks
/// nothing: it reports the same wait again. A drive at or after it settles
/// it, and the script checks on the job, waiting again while it runs, and
/// ends with it.
#[test]
fn a_provider_jobs_run_pauses_on_each_wait_and_checks_only_once_its_due() -> TestResult {
    let root = crate::support::temp_root("job-wait-not-due");
    let skill = compute_project(&root)?;
    let paused = run_job(&root, &skill, 3600, None)?;
    let (wait, until) = wait_of(&paused);
    assert_eq!(wait, "job/2", "{paused}");
    assert_eq!(provider_lines(&root, "starts")?.len(), 1);
    // Before the wait's time, a drive settles nothing and checks nothing.
    let again = drive(&root)?;
    assert_eq!(wait_of(&again), (wait, until), "{again}");
    assert_eq!(provider_lines(&root, "inspections")?, Vec::<String>::new());

    let root = crate::support::temp_root("job-wait-due");
    let skill = compute_project(&root)?;
    let paused = run_job(&root, &skill, 0, None)?;
    assert_eq!(wait_of(&paused).0, "job/2", "{paused}");
    assert_eq!(provider_lines(&root, "inspections")?, Vec::<String>::new());
    // At or after its time, the wait settles and the script checks on the
    // job, which is running, so it waits again.
    let checked = drive(&root)?;
    assert_eq!(wait_of(&checked).0, "job/5", "{checked}");
    assert_eq!(provider_lines(&root, "inspections")?.len(), 1);
    let ended = drive(&root)?;
    assert_eq!(ended["status"], "sealed", "{ended}");
    assert_eq!(ended["result"]["job"]["status"], "completed", "{ended}");
    assert_eq!(provider_lines(&root, "inspections")?.len(), 2);
    // Each drive went on from the start its run recorded, sending it once.
    assert_eq!(provider_lines(&root, "requests")?.len(), 1);
    Ok(())
}

/// Killed just past its job's start, before its run recorded it, compute-job
/// recovers with one job started. The start ran and custody holds what it
/// read back, so recovery reconciles the start from that with nobody asked,
/// sending nothing again, and the run goes on from its wait to the job's
/// end. Changed 11 November: recovery waited on the start's retry, and a
/// person's acknowledgement then read the start back (ruled: runx reconciles
/// from evidence first, and a person answers only when nothing settles it).
#[test]
fn a_kill_just_past_a_provider_jobs_start_recovers_with_one_start() -> TestResult {
    let root = crate::support::temp_root("job-killed-past-start");
    let skill = compute_project(&root)?;
    run_job(&root, &skill, 0, Some("after_execution@job/0"))?;
    assert_eq!(provider_lines(&root, "starts")?.len(), 1);

    let paused = run_job(&root, &skill, 0, None)?;
    assert_eq!(wait_of(&paused).0, "job/2", "{paused}");
    assert_eq!(
        provider_lines(&root, "requests")?.len(),
        1,
        "recovery read the start back and sent nothing again"
    );
    assert_eq!(wait_of(&drive(&root)?).0, "job/5");
    let ended = drive(&root)?;
    assert_eq!(ended["status"], "sealed", "{ended}");
    assert_eq!(
        ended["result"]["job"]["job_ref"], "compute://job/1",
        "{ended}"
    );
    assert_eq!(provider_lines(&root, "starts")?.len(), 1, "one job started");
    assert_eq!(provider_lines(&root, "requests")?.len(), 1);
    let started = calls(&history(&root)?, RUN_ID)
        .into_iter()
        .filter(|call| *call == pair(0, "completed"))
        .count();
    assert_eq!(started, 1, "the run recorded one start");
    Ok(())
}

/// A script that reads each item of its batch by reference and digests it.
const BATCH_BY_REFERENCE: &str = r#"export default async function (ctx, { items }) {
  const digests = [];
  for (const item of items) {
    const { text } = await ctx.read(item);
    const made = await ctx.tool("data.digest", { value: text, encoding: "utf8_text" });
    digests.push(made.digest_result.digest);
  }
  return { digests };
}
"#;

/// How many items the large batch holds, and how long each one's text is:
/// 12 MiB in all.
const LARGE_BATCH: (usize, usize) = (48, 256 * 1024);

/// `batch-input` runs [`BATCH_BY_REFERENCE`] over its `items`, and `root`
/// holds `items.json`, a batch of [`LARGE_BATCH`] far past the worker's 4 MiB
/// input bound.
fn write_large_batch(root: &Path) -> TestResult<PathBuf> {
    let skill = root.join("batch-input");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: batch-input\ndescription: Digests each item of a batch.\n---\n# Batch input\n\nDigest each.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  digest:
    inputs:
      items: {type: array}
    steps:
      - id: prepare
        script: {module: batch.mjs}
        inputs:
          items: $input.items
        calls:
          tools: [data.digest]
        max_calls: 100
        outputs:
          digests: {type: array}
"#,
    )?;
    fs::write(skill.join("batch.mjs"), BATCH_BY_REFERENCE)?;
    let (items, length) = LARGE_BATCH;
    let items = (0..items)
        .map(|item| serde_json::json!({"text": format!("{item:04}{}", "a".repeat(length))}))
        .collect::<Vec<_>>();
    fs::write(
        root.join("items.json"),
        serde_json::to_vec(&serde_json::json!({"items": items}))?,
    )?;
    Ok(skill)
}

/// Run `batch-input` over `root`'s `items.json` under the pinned run id,
/// crashing at `crash` when given.
fn run_large_batch(root: &Path, skill: &Path, crash: Option<&str>) -> TestResult<Output> {
    let mut command = unsigned_runx_command_at(root);
    command.args([
        "skill",
        skill.to_str().ok_or("non-utf8 skill dir")?,
        "--inputs",
        "items.json",
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

/// A batch far past the worker's 4 MiB input bound reaches a script item by
/// item, each by reference, which the script reads with `ctx.read`. Killed
/// after reading 42 items, 10.5 MiB, past the worker's 10 MiB frame, the run
/// recovers by replaying what it read, each read's content in the frames
/// that follow the start, never all of it in one, and goes on to seal with
/// what an uncrashed run returns.
#[test]
fn a_batch_past_the_input_bound_reaches_a_script_by_reference_and_replays_after_a_kill()
-> TestResult {
    let reference = crate::support::temp_root("large-batch-live");
    let skill = write_large_batch(&reference)?;
    let uncrashed = status(&run_large_batch(&reference, &skill, None)?)?;
    assert_eq!(uncrashed["status"], "sealed", "{uncrashed}");
    let digests = result_of(&uncrashed)?["digests"].clone();
    assert_eq!(
        digests.as_array().map(Vec::len),
        Some(LARGE_BATCH.0),
        "{uncrashed}"
    );

    // Killed as the 42nd item's digest is about to be admitted: item k is
    // read at position 2k and digested at 2k + 1.
    let root = crate::support::temp_root("large-batch-killed");
    let skill = write_large_batch(&root)?;
    crashed(&run_large_batch(
        &root,
        &skill,
        Some("before_admission@prepare/83"),
    )?);
    let recovered = status(&run_large_batch(&root, &skill, None)?)?;
    assert_eq!(recovered["status"], "sealed", "{recovered}");
    assert_eq!(result_of(&recovered)?["digests"], digests);
    Ok(())
}

/// A connector whose one read a process carries out: each read appends the
/// note it read to `notes/reads` and answers with it.
const LOCAL_NOTES: &str = r#"schema: runx.connector.v1
name: fixture/notes
title: Fixture Notes
description: Read notes through a process.
auth:
  none: {}
placements:
  local:
    auth: [none]
operations:
  notes.read:
    description: Read one note.
    verb: read
    scopes: [notes.read]
    target:
      template: {"$argument": "/note"}
    result_fields: [note]
    implementation:
      process:
        tool: notes.read
"#;

const NOTES_READ_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "notes.read",
  "version": "0.1.0",
  "description": "Read one note.",
  "process": {"command": "sh", "args": ["./read.sh"]},
  "inputs": {"note": {"type": "string", "description": "The note."}}
}
"#;

const NOTES_READ: &str = r#"#!/bin/sh
set -eu
mkdir -p "$RUNX_CWD/notes"
printf '%s\n' "$RUNX_INPUT_NOTE" >> "$RUNX_CWD/notes/reads"
printf '{"note":"%s"}\n' "$RUNX_INPUT_NOTE"
"#;

/// `notes-demo` reads its note through [`LOCAL_NOTES`] and digests it, each
/// apart, and says how each went: the read's code when it failed.
fn notes_project(root: &Path) -> TestResult<PathBuf> {
    let tool = root.join("connectors/fixture/notes/tools/notes/read");
    fs::create_dir_all(&tool)?;
    fs::write(tool.join("manifest.json"), NOTES_READ_TOOL)?;
    fs::write(tool.join("read.sh"), NOTES_READ)?;
    fs::write(
        root.join("connectors/fixture/notes/connector.yaml"),
        LOCAL_NOTES,
    )?;
    let skill = root.join("notes-demo");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: notes-demo\ndescription: Reads a note and digests it.\n---\n# Notes demo\n\nRead, then digest.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  prepare:
    inputs:
      value: {type: string}
    steps:
      - id: prepare
        script: {module: prepare.mjs}
        inputs:
          value: $input.value
        calls:
          operations: [fixture/notes/notes.read]
          tools: [data.digest]
        max_calls: 4
        outputs:
          read: {type: string}
          digested: {type: string}
"#,
    )?;
    fs::write(
        skill.join("prepare.mjs"),
        r#"export default async function (ctx, { value }) {
  const [read, digested] = await Promise.allSettled([
    ctx.operation("fixture/notes/notes.read", { note: value }),
    ctx.tool("data.digest", { value, encoding: "utf8_text" }),
  ]);
  return {
    read: read.status === "fulfilled" ? read.value.provider_operation.result.note : read.reason.code,
    digested: digested.status,
  };
}
"#,
    )?;
    Ok(skill)
}

/// The lines `root`'s fixture provider appended to `path`.
fn lines(root: &Path, path: &str) -> TestResult<Vec<String>> {
    match fs::read_to_string(root.join(path)) {
        Ok(lines) => Ok(lines.lines().map(str::to_owned).collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

/// A read a process carries out rests on its author's promise that it only
/// read, so one in flight at a kill isn't run again unseen: it waits on its
/// own retry, which says it can't be sent again safely, while the script's
/// other call goes on, and the run stays open. A person's `{"retry": true}`
/// reads it again; `{"retry": false}` leaves it failed with
/// `retry_declined`, which the script catches, and the run seals.
#[test]
fn a_process_read_in_flight_at_a_kill_waits_on_its_own_retry_and_the_run_goes_on() -> TestResult {
    for retry in [true, false] {
        let root = crate::support::temp_root("process-read-in-flight");
        let skill = notes_project(&root)?;
        crashed(&run(&root, &skill, Some("after_execution@prepare/0"))?);
        assert_eq!(lines(&root, "notes/reads")?, ["hello"]);

        let held = status(&run(&root, &skill, None)?)?;
        let requests = held["requests"].as_array().cloned().unwrap_or_default();
        assert!(
            matches!(&requests[..], [request] if request["id"] == "prepare/0/retry"),
            "the read waits on its own retry: {held}"
        );
        let asked: serde_json::Value = serde_json::from_slice(&fs::read(
            requests[0]["artifact"]["path"].as_str().unwrap_or_default(),
        )?)?;
        assert_eq!(asked["resend"], "unsafe", "{asked}");
        // The digest went on, and the read ran no further unasked.
        let read = history(&root)?;
        assert!(
            calls(&read, RUN_ID).contains(&pair(1, "completed")),
            "{read}"
        );
        assert_eq!(lines(&root, "notes/reads")?, ["hello"]);

        let ended = answer_retries(&root, retry)?;
        assert_eq!(ended["status"], "sealed", "{ended}");
        let result = result_of(&ended)?;
        assert_eq!(result["digested"], "fulfilled", "{ended}");
        if retry {
            assert_eq!(result["read"], "hello", "{ended}");
            assert_eq!(lines(&root, "notes/reads")?, ["hello", "hello"]);
        } else {
            assert_eq!(result["read"], "retry_declined", "{ended}");
            assert_eq!(lines(&root, "notes/reads")?, ["hello"]);
        }
    }
    Ok(())
}

/// A local copy of `fixture/mail`, whose process sends from one mailbox. It
/// doesn't deduplicate: each send it performs appends `<key> <recipient>` to
/// `mail/sent`, and it refuses a recipient listed in `mail/refuse`. A send's
/// readback finds the message sent with its key.
const LOCAL_MAIL: &str = r#"schema: runx.connector.v1
name: fixture/mail
title: Fixture Mail
description: Send messages from one mailbox of a mail fixture.
auth:
  credentials:
    mailbox_key:
      kind: api_key
      fields:
        - {name: token, label: mailbox token}
      delivery: {env: MAIL_TOKEN}
      audience: ["*.mail.fixture.invalid"]
placements:
  local:
    auth: [credentials]
operations:
  message.send:
    description: Send one message to one recipient.
    verb: write
    scopes: [message.send]
    credentials: [mailbox_key]
    target:
      template: {"$argument": "/to"}
    result_fields: [message_ref, to]
    implementation:
      process:
        tool: mail.send
        readback:
          matches:
            - expected: {source: argument, path: to}
              observed: to
"#;

const MAIL_SEND_TOOL: &str = r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "mail.send",
  "version": "0.1.0",
  "description": "Send one message, or read a sent one back by its key.",
  "process": {"command": "sh", "args": ["./send.sh"]},
  "inputs": {
    "to": {"type": "string", "description": "The recipient."},
    "body": {"type": "string", "description": "The message."},
    "phase": {"type": "string", "description": "perform or readback."},
    "idempotency_key": {"type": "string", "description": "The key the message is sent with."},
    "acknowledgement": {"type": "object", "optional": true, "description": "What the send answered."}
  }
}
"#;

const MAIL_SEND: &str = r#"#!/bin/sh
set -eu
mailbox="$RUNX_CWD/mail"
mkdir -p "$mailbox"
touch "$mailbox/sent" "$mailbox/refuse"
key="$RUNX_INPUT_IDEMPOTENCY_KEY"
if [ "$RUNX_INPUT_PHASE" = readback ]; then
  while read -r sent to; do
    if [ "$sent" = "$key" ]; then
      printf '{"message_ref":"mail://%s","to":"%s"}\n' "$key" "$to"
      exit 0
    fi
  done < "$mailbox/sent"
  echo "no message was sent with this key" >&2
  exit 1
fi
if grep -qx "$RUNX_INPUT_TO" "$mailbox/refuse"; then
  printf '{"refused":"the mailbox refuses %s"}\n' "$RUNX_INPUT_TO"
  exit 0
fi
printf '%s %s\n' "$key" "$RUNX_INPUT_TO" >> "$mailbox/sent"
printf '{"message_ref":"mail://%s","to":"%s"}\n' "$key" "$RUNX_INPUT_TO"
"#;

/// A project holding `fixtures/skills/send-batch` and [`LOCAL_MAIL`].
fn mail_project(root: &Path) -> TestResult<PathBuf> {
    let tool = root.join("connectors/fixture/mail/tools/mail/send");
    fs::create_dir_all(&tool)?;
    fs::write(tool.join("manifest.json"), MAIL_SEND_TOOL)?;
    fs::write(tool.join("send.sh"), MAIL_SEND)?;
    fs::write(
        root.join("connectors/fixture/mail/connector.yaml"),
        LOCAL_MAIL,
    )?;
    let skill = root.join("skills/send-batch");
    crate::support::copy_dir(
        &crate::support::repo_root()?.join("fixtures/skills/send-batch"),
        &skill,
    )?;
    Ok(skill)
}

/// Send `body` to each of ann, bob and carol as run `run_id`, crashing at
/// `crash` when given.
fn send_batch(root: &Path, skill: &Path, run_id: &str, crash: Option<&str>) -> TestResult<Output> {
    send_batch_of(root, skill, run_id, "The fixture is ready.", crash)
}

/// [`send_batch`], sending `body`.
fn send_batch_of(
    root: &Path,
    skill: &Path,
    run_id: &str,
    body: &str,
    crash: Option<&str>,
) -> TestResult<Output> {
    fs::write(
        root.join("batch.json"),
        serde_json::json!({
            "recipients": ["ann@example.test", "bob@example.test", "carol@example.test"],
            "body": body,
        })
        .to_string(),
    )?;
    let mut command = unsigned_runx_command_at(root);
    command
        .env("MAIL_TOKEN", "local-mailbox-token")
        .arg("skill")
        .arg(skill)
        .args([
            "--inputs",
            "batch.json",
            "--run-id",
            run_id,
            "--receipt-dir",
        ])
        .arg(root.join("receipts"))
        .arg("--json");
    if let Some(crash) = crash {
        command.env("RUNX_CRASH_AT", crash);
    }
    Ok(command.output()?)
}

/// How many messages the mailbox sent `to`.
fn sent_to(root: &Path, to: &str) -> TestResult<usize> {
    Ok(lines(root, "mail/sent")?
        .iter()
        .filter(|line| line.split_whitespace().nth(1) == Some(to))
        .count())
}

/// Each recipient's status in a sealed batch.
fn statuses(sealed: &serde_json::Value) -> TestResult<Vec<(String, String)>> {
    Ok(result_of(sealed)?["sent"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|sent| {
            (
                sent["to"].as_str().unwrap_or_default().to_owned(),
                sent["status"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect())
}

/// The interrupted send (plan §9's revise row). Killed just past bob's send,
/// before its run recorded it, the batch has sent bob one message, and the
/// call waits as claimed while custody keeps the attempt unsettled. Driven
/// again, runx reconciles it from what custody read back, with nobody asked,
/// so bob is never sent twice, and the batch goes on to carol, whose send the
/// mailbox refuses. A later run under the same keys sends ann and bob nothing:
/// each is already done under its key. Carol's refused send is tried again,
/// and goes.
#[test]
fn an_interrupted_send_reconciles_and_the_batch_goes_on() -> TestResult {
    let root = crate::support::temp_root("interrupted-send");
    let skill = mail_project(&root)?;
    fs::create_dir_all(root.join("mail"))?;
    fs::write(root.join("mail/refuse"), "carol@example.test\n")?;

    crashed(&send_batch(
        &root,
        &skill,
        RUN_ID,
        Some("after_execution@send/1"),
    )?);
    assert_eq!(sent_to(&root, "ann@example.test")?, 1);
    assert_eq!(sent_to(&root, "bob@example.test")?, 1, "one observed send");
    // The kill left bob's call claimed: its unknown outcome persisted.
    let read = history(&root)?;
    let bob = calls(&read, RUN_ID)
        .into_iter()
        .rev()
        .find(|(index, _)| *index == 1);
    assert_eq!(bob, Some(pair(1, "claimed")), "{read}");

    let sealed = status(&send_batch(&root, &skill, RUN_ID, None)?)?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    assert_eq!(
        statuses(&sealed)?,
        [
            ("ann@example.test".to_owned(), "sent".to_owned()),
            ("bob@example.test".to_owned(), "sent".to_owned()),
            ("carol@example.test".to_owned(), "failed".to_owned()),
        ]
    );
    assert_eq!(
        sent_to(&root, "bob@example.test")?,
        1,
        "reconciled, never resent"
    );
    assert!(
        calls(&history(&root)?, RUN_ID).contains(&pair(1, "completed")),
        "bob's call completed from what custody read back"
    );

    // Under the same keys, a later run sends nothing already done, and tries
    // the refused send again.
    fs::write(root.join("mail/refuse"), "")?;
    let again = status(&send_batch(&root, &skill, "interrupted_send_again", None)?)?;
    assert_eq!(again["status"], "sealed", "{again}");
    assert_eq!(
        statuses(&again)?,
        [
            ("ann@example.test".to_owned(), "already_done".to_owned()),
            ("bob@example.test".to_owned(), "already_done".to_owned()),
            ("carol@example.test".to_owned(), "sent".to_owned()),
        ]
    );
    for to in ["ann@example.test", "bob@example.test", "carol@example.test"] {
        assert_eq!(sent_to(&root, to)?, 1, "{to}");
    }
    Ok(())
}

/// `send-one` sends ann one message from a fixed step, under one key.
fn write_send_one(root: &Path) -> TestResult<PathBuf> {
    let skill = root.join("send-one");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: send-one\ndescription: Sends ann one message.\n---\n# Send one\n\nSend it.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  send:
    steps:
      - id: send
        operation: fixture/mail/message.send
        idempotency_key: send-one:ann
        inputs:
          to: ann@example.test
          body: The fixture is ready.
"#,
    )?;
    Ok(skill)
}

/// A fixed step reconciles from custody as a call does, with nobody asked.
/// Killed after its send ran, its answer is in custody and settles it, so
/// nothing is sent again; killed before custody recorded an attempt, it never
/// reached the mailbox, so it sends now. Either way the run seals with one
/// message sent and no retry asked of a person.
#[test]
fn a_fixed_operation_step_in_flight_at_a_kill_reconciles_from_custody() -> TestResult {
    for crash in ["after_execution@send", "after_admission@send"] {
        let root = crate::support::temp_root("fixed-send-in-flight");
        mail_project(&root)?;
        let skill = write_send_one(&root)?;
        let mut killed = unsigned_runx_command_at(&root);
        killed
            .env("MAIL_TOKEN", "local-mailbox-token")
            .env("RUNX_CRASH_AT", crash)
            .arg("skill")
            .arg(&skill)
            .args(["--run-id", RUN_ID, "--receipt-dir"])
            .arg(root.join("receipts"))
            .arg("--json");
        crashed(&killed.output()?);

        let mut recovered = unsigned_runx_command_at(&root);
        recovered
            .env("MAIL_TOKEN", "local-mailbox-token")
            .arg("skill")
            .arg(&skill)
            .args(["--run-id", RUN_ID, "--receipt-dir"])
            .arg(root.join("receipts"))
            .arg("--json");
        let sealed = status(&recovered.output()?)?;
        assert_eq!(sealed["status"], "sealed", "{crash}: {sealed}");
        assert_eq!(sealed["outcome"], "completed", "{crash}: {sealed}");
        assert_eq!(sent_to(&root, "ann@example.test")?, 1, "{crash}");
    }

    // A read a process carries out is in no custody, so nothing settles it:
    // driven again, it waits on its retry as it was, read once.
    let root = crate::support::temp_root("fixed-read-in-flight");
    notes_project(&root)?;
    let skill = root.join("read-one");
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: read-one\ndescription: Reads one note.\n---\n# Read one\n\nRead it.\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  read:\n    inputs:\n      value: {type: string}\n    steps:\n      - id: read\n        operation: fixture/notes/notes.read\n        inputs:\n          note: $input.value\n",
    )?;
    crashed(&run(&root, &skill, Some("after_execution@read"))?);
    for _ in 0..2 {
        let held = status(&run(&root, &skill, None)?)?;
        let requests = held["requests"].as_array().cloned().unwrap_or_default();
        assert!(
            matches!(&requests[..], [request] if request["id"] == "read/retry"),
            "{held}"
        );
    }
    assert_eq!(lines(&root, "notes/reads")?, ["hello"]);
    Ok(())
}

/// Drive run `run_id` again, answering nothing, with the mailbox's token.
fn drive_mail(root: &Path, run_id: &str) -> TestResult<serde_json::Value> {
    let nothing = root.join("nothing.json");
    fs::write(&nothing, "{}")?;
    status(
        &unsigned_runx_command_at(root)
            .env("MAIL_TOKEN", "local-mailbox-token")
            .args(["resume", run_id])
            .arg(&nothing)
            .arg("--receipt-dir")
            .arg(root.join("receipts"))
            .arg("--json")
            .output()?,
    )
}

/// The hold (D34): an act whose attempt of another intent is unsettled holds
/// a call of it as a wait on that call alone. Killed just past bob's send, a
/// batch leaves bob's attempt unsettled. A batch with another body then finds
/// ann already done, sends carol, and waits on bob's retry, sending him
/// nothing, since his first attempt read back what it sent. That attempt
/// settles by its own run's recovery, and the waiting batch, driven again,
/// finds bob's act done under his key and seals. Each recipient got one
/// message.
#[test]
fn another_bodys_unsettled_send_holds_that_call_while_the_others_go_on() -> TestResult {
    let root = crate::support::temp_root("held-send");
    let skill = mail_project(&root)?;
    crashed(&send_batch(
        &root,
        &skill,
        RUN_ID,
        Some("after_execution@send/1"),
    )?);

    let revised = "The fixture is ready, and so is the guide.";
    let held = status(&send_batch_of(&root, &skill, "held_batch", revised, None)?)?;
    let requests = held["requests"].as_array().cloned().unwrap_or_default();
    assert!(
        matches!(&requests[..], [request] if request["id"] == "send/1/retry"),
        "only bob's call waits: {held}"
    );
    assert_eq!(
        sent_to(&root, "bob@example.test")?,
        1,
        "bob is sent nothing"
    );
    assert_eq!(sent_to(&root, "carol@example.test")?, 1, "carol goes on");
    // Driven again with nothing settled, it still waits on bob alone.
    let still = drive_mail(&root, "held_batch")?;
    assert_eq!(still["requests"][0]["id"], "send/1/retry", "{still}");

    let first = status(&send_batch(&root, &skill, RUN_ID, None)?)?;
    assert_eq!(first["status"], "sealed", "{first}");
    let sealed = drive_mail(&root, "held_batch")?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    assert_eq!(
        statuses(&sealed)?,
        [
            ("ann@example.test".to_owned(), "already_done".to_owned()),
            ("bob@example.test".to_owned(), "already_done".to_owned()),
            ("carol@example.test".to_owned(), "sent".to_owned()),
        ]
    );
    for to in ["ann@example.test", "bob@example.test", "carol@example.test"] {
        assert_eq!(sent_to(&root, to)?, 1, "{to}");
    }
    Ok(())
}

/// A power cut, as against a kill: what the crash's sync points left. The run
/// is killed at `crash` with `RUNX_POWER_LOSS_IMAGE` set, so each sync records
/// what a cut then leaves on the disk; the cut puts each store back as its
/// last sync left it, and of the receipt and artifact files keeps those a
/// sync made durable, as that sync left them. Any other is lost, or left torn
/// as a cut can leave a file it was writing.
fn power_cut_at(root: &Path, skill: &Path, crash: &str, torn: bool) -> TestResult {
    let image = root.join("power-loss-image");
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
    command
        .env("RUNX_CRASH_AT", crash)
        .env("RUNX_POWER_LOSS_IMAGE", &image);
    crashed(&command.output()?);
    restore_power_loss_image(root, &image, torn)
}

/// Restore exactly what the test's last full flushes made durable.
pub(super) fn restore_power_loss_image(root: &Path, image: &Path, torn: bool) -> TestResult {
    if let Ok(failed) = fs::read_to_string(image.join("failed")) {
        return Err(failed.into());
    }
    let manifest = fs::read_to_string(image.join("manifest"))?;
    let mut stores = std::collections::BTreeMap::new();
    let mut synced = std::collections::BTreeMap::new();
    for line in manifest.lines() {
        match line.split('\t').collect::<Vec<_>>().as_slice() {
            ["store", name, database] => {
                stores.insert(PathBuf::from(database), (*name).to_owned());
            }
            ["file", name, path] => {
                synced.insert(canonical(Path::new(path))?, (*name).to_owned());
            }
            _ => return Err(format!("the image's manifest holds {line:?}").into()),
        }
    }
    let beside = |path: &Path, suffix: &str| PathBuf::from(format!("{}{suffix}", path.display()));
    for (database, name) in &stores {
        let copy = image.join("stores").join(name);
        fs::copy(&copy, database)?;
        if beside(&copy, "-wal").exists() {
            fs::copy(beside(&copy, "-wal"), beside(database, "-wal"))?;
        } else if beside(database, "-wal").exists() {
            fs::remove_file(beside(database, "-wal"))?;
        }
        if beside(database, "-shm").exists() {
            fs::remove_file(beside(database, "-shm"))?;
        }
    }
    for directory in [root.join("receipts"), root.join(".runx").join("artifacts")] {
        for file in files_under(&directory)? {
            // A synced file is as its last sync left it.
            if let Some(name) = synced.get(&canonical(&file)?) {
                fs::copy(image.join("files").join(name), &file)?;
                continue;
            }
            if torn {
                fs::File::create(&file)?;
            } else {
                fs::remove_file(&file)?;
            }
        }
    }
    fs::remove_dir_all(image)?;
    Ok(())
}

/// `path` with its directory resolved, so two spellings of one file match.
fn canonical(path: &Path) -> TestResult<PathBuf> {
    let directory = path
        .parent()
        .ok_or("a file has a directory")?
        .canonicalize()?;
    Ok(directory.join(path.file_name().ok_or("a file has a name")?))
}

/// Every file under `directory`, at any depth; none when it isn't there.
fn files_under(directory: &Path) -> TestResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !directory.is_dir() {
        return Ok(files);
    }
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            files.extend(files_under(&path)?);
        } else {
            files.push(path);
        }
    }
    Ok(files)
}

/// A write is claimed durably before it reaches the world, so a power cut
/// while it runs leaves its claim, and recovery waits on its retry rather
/// than send it again unseen.
#[test]
fn a_power_cut_with_a_write_in_flight_leaves_its_claim_and_recovery_waits_on_its_retry()
-> TestResult {
    let root = crate::support::temp_root("power-cut-write");
    let skill = write_skills(
        &root,
        WRITE_THEN_DIGEST,
        "          digested: {type: array}\n",
    )?;
    power_cut_at(&root, &skill, "after_execution@prepare/0", false)?;
    let read = history(&root)?;
    assert_eq!(calls(&read, RUN_ID), [pair(0, "claimed")]);

    let recovered = status(&run(&root, &skill, None)?)?;
    assert_eq!(
        recovered["requests"][0]["id"], "prepare/0/retry",
        "{recovered}"
    );
    let resumed = acknowledge_retries(&root)?;
    assert_eq!(resumed["status"], "sealed", "{resumed}");
    Ok(())
}

/// A safe read a power cut catches in flight, or just after it settled,
/// reruns on recovery with nobody asked (D33), and the run seals with it
/// completed once.
#[test]
fn a_power_cut_around_a_safe_read_reruns_it_and_the_run_seals() -> TestResult {
    for crash in ["after_execution@prepare/0", "before_admission@prepare/1"] {
        let root = crate::support::temp_root("power-cut-read");
        let skill = write_skills(&root, TWO_DIGESTS, "          digested: {type: array}\n")?;
        power_cut_at(&root, &skill, crash, false)?;
        let recovered = status(&run(&root, &skill, None)?)?;
        assert_eq!(recovered["status"], "sealed", "{crash}: {recovered}");
        let settled = calls(&history(&root)?, RUN_ID);
        for index in [0, 1] {
            assert_eq!(
                settled
                    .iter()
                    .filter(|call| **call == pair(index, "completed"))
                    .count(),
                1,
                "{crash}: {settled:?}"
            );
        }
    }
    Ok(())
}

/// A sealed run's receipts are its record's: a power cut after the seal,
/// before its receipt files were written, leaves a run that, run again,
/// returns what it sealed and writes each receipt whole.
#[test]
fn a_power_cut_after_the_seal_leaves_a_run_whose_receipts_its_record_restores() -> TestResult {
    let root = crate::support::temp_root("power-cut-seal");
    let skill = write_skills(&root, TWO_DIGESTS, "          digested: {type: array}\n")?;
    power_cut_at(&root, &skill, &format!("after_seal@{RUN_ID}"), false)?;
    verify_sealed_again(&root, &skill)
}

/// A run's receipt files are a copy of its sealed record, written without a
/// sync of their own, so a power cut after they were written can lose or
/// tear them. A read by a receipt's id restores its run's from the record,
/// before the run is ever run again, and the run run again returns what it
/// sealed with each receipt whole.
#[test]
fn a_power_cut_after_the_receipts_were_written_loses_none_a_read_by_id_finds() -> TestResult {
    for torn in [false, true] {
        let other = crate::support::temp_root("power-cut-receipts");
        let skill = write_skills(&other, TWO_DIGESTS, "          digested: {type: array}\n")?;
        power_cut_at(&other, &skill, &format!("after_receipts@{RUN_ID}"), torn)?;
        // The run's record holds its seal, which names its receipt.
        let receipt = history(&other)?["events"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|record| record["stream"] == RUN_ID && record["event"]["kind"] == "sealed")
            .and_then(|record| record["event"]["receipt_id"].as_str())
            .ok_or("the record holds the run's seal")?
            .to_owned();
        let lost = files_under(&other.join("receipts"))?
            .iter()
            .filter(|file| {
                file.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .filter(|file| fs::metadata(file).is_ok_and(|metadata| metadata.len() > 0))
            .count();
        assert_eq!(lost, 0, "torn {torn}: the cut left no receipt whole");
        assert_eq!(
            verified(&other, &receipt)?,
            Ok(()),
            "torn {torn}: a read by id restores the run's receipts"
        );
        verify_sealed_again(&other, &skill)?;
    }
    Ok(())
}

/// Whether `runx verify` verifies `receipt` in `root`'s receipt directory,
/// or what it said.
fn verified(root: &Path, receipt: &str) -> TestResult<Result<(), String>> {
    let verified = unsigned_runx_command_at(root)
        .args(["verify", receipt, "--receipt-dir"])
        .arg(root.join("receipts"))
        .arg("--json")
        .output()?;
    Ok(if verified.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&verified.stdout),
            String::from_utf8_lossy(&verified.stderr)
        ))
    })
}

/// Run `skill` again, which a sealed run answers with what it sealed, and
/// verify the receipt it names.
fn verify_sealed_again(root: &Path, skill: &Path) -> TestResult {
    let sealed = status(&run(root, skill, None)?)?;
    assert_eq!(sealed["status"], "sealed", "{sealed}");
    let receipt = sealed["receipt_id"]
        .as_str()
        .ok_or("a sealed run names its receipt")?;
    let verified = unsigned_runx_command_at(root)
        .args(["verify", receipt, "--receipt-dir"])
        .arg(root.join("receipts"))
        .arg("--json")
        .output()?;
    assert!(
        verified.status.success(),
        "{}",
        String::from_utf8_lossy(&verified.stderr)
    );
    Ok(())
}
