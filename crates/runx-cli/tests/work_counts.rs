//! The work a runx process does, counted and written where
//! `RUNX_PERF_COUNTS_PATH` names: what the performance matrix's structural
//! budgets read (performance.md), measured by counting, never by timing.

use std::fs;
use std::path::Path;

use serde_json::Value;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// What `runx <args>` in `root` counted.
fn counted(root: &Path, args: &[&str]) -> TestResult<Value> {
    let counts = root.join("counts.json");
    let output = crate::support::unsigned_runx_command_at(root)
        .env("RUNX_CWD", root)
        .env("RUNX_PERF_COUNTS_PATH", &counts)
        .args(args)
        .output()?;
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&fs::read(&counts)?)?)
}

#[test]
fn a_launch_that_runs_nothing_spawns_no_process() -> TestResult {
    let root = crate::support::temp_root("runx-work-counts-launch");
    fs::create_dir_all(root.join("home"))?;
    let counts = counted(&root, &["--version"])?;
    assert_eq!(counts["process_spawns"], 0, "{counts}");
    Ok(())
}

#[test]
fn a_process_tool_is_one_spawn() -> TestResult {
    let root = crate::support::temp_root("runx-work-counts-process");
    let skill = root.join("skills/printer");
    fs::create_dir_all(&skill)?;
    fs::create_dir_all(root.join("home"))?;
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: printer\ndescription: Prints one object.\n---\n# Printer\n",
    )?;
    fs::write(
        skill.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  print:\n    steps:\n      - id: print\n        tool: {command: sh, args: [-c, 'printf \"{\\\"ok\\\":true}\"'], timeout_seconds: 5}\n        outputs:\n          ok: {type: boolean}\n",
    )?;
    let counts = counted(&root, &["skill", "skills/printer", "--json"])?;
    assert_eq!(counts["process_spawns"], 1, "{counts}");
    Ok(())
}
