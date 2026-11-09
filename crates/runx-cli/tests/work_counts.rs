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

/// A package at `root/skills/<name>` whose one method is `calls` native
/// digests, each reading nothing but its own input.
fn native_package(root: &Path, name: &str, calls: usize) -> TestResult {
    let package = root.join("skills").join(name);
    fs::create_dir_all(&package)?;
    fs::write(
        package.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Digests its own subjects.\n---\n# {name}\n"),
    )?;
    let steps = (0..calls)
        .map(|call| {
            format!(
                "      - id: call-{call}\n        tool: data.digest\n        inputs:\n          value: {{subject: call-{call}}}\n"
            )
        })
        .collect::<String>();
    fs::write(
        package.join("run.yaml"),
        format!("minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n{steps}"),
    )?;
    Ok(())
}

fn count_of(counts: &Value, field: &str) -> TestResult<u64> {
    counts[field]
        .as_u64()
        .ok_or_else(|| format!("{field} isn't a count in {counts}").into())
}

/// Readiness follows the skill, not the workspace: inspecting one skill reads
/// and verifies as much beside 40 other packages as alone, verifies no
/// package twice, starts nothing and sends nothing.
#[test]
fn inspecting_a_skill_reads_and_verifies_as_much_whatever_else_is_in_the_workspace() -> TestResult {
    let ready = |others: usize| -> TestResult<Value> {
        let root = crate::support::temp_root(&format!("runx-work-counts-ready-{others}"));
        fs::create_dir_all(root.join("home"))?;
        native_package(&root, "ready", 1)?;
        for other in 0..others {
            native_package(&root, &format!("other-{other:03}"), 1)?;
        }
        counted(&root, &["skill", "inspect", "ready", "--json"])
    };
    let alone = ready(0)?;
    let among = ready(40)?;
    for counts in [&alone, &among] {
        assert_eq!(count_of(counts, "process_spawns")?, 0, "{counts}");
        assert_eq!(count_of(counts, "network_requests")?, 0, "{counts}");
        assert_eq!(
            count_of(counts, "most_verifications_of_one_package")?,
            1,
            "{counts}"
        );
    }
    assert_eq!(
        count_of(&among, "packages_read")?,
        count_of(&alone, "packages_read")?,
        "alone={alone}\namong={among}"
    );
    assert_eq!(
        count_of(&among, "package_verifications")?,
        count_of(&alone, "package_verifications")?,
        "alone={alone}\namong={among}"
    );
    Ok(())
}

/// Each call of a run costs the store the same writes, and no call takes the
/// run whole: five calls write the store as many more times than three as
/// three do than one, and replay and seal the run no more often than one.
#[test]
fn each_call_of_a_run_writes_the_store_alike_and_takes_the_run_whole_never() -> TestResult {
    let run = |calls: usize| -> TestResult<Value> {
        let root = crate::support::temp_root(&format!("runx-work-counts-calls-{calls}"));
        fs::create_dir_all(root.join("home"))?;
        native_package(&root, "calls", calls)?;
        counted(&root, &["skill", "skills/calls", "--json"])
    };
    let one = run(1)?;
    let three = run(3)?;
    let five = run(5)?;
    let writes = |counts: &Value| count_of(counts, "store_writes");
    assert!(writes(&three)? > writes(&one)?, "one={one}\nthree={three}");
    assert_eq!(
        writes(&five)? - writes(&three)?,
        writes(&three)? - writes(&one)?,
        "one={one}\nthree={three}\nfive={five}"
    );
    assert_eq!(
        count_of(&five, "run_materializations")?,
        count_of(&one, "run_materializations")?,
        "one={one}\nfive={five}"
    );
    Ok(())
}

/// What `runx <args>` printed and counted, launched in `dir` with its home
/// under `root` and no workspace named, so runx finds the workspace from `dir`
/// as an operator's shell would.
fn launched_in(dir: &Path, root: &Path, args: &[&str]) -> TestResult<(Value, Value)> {
    let counts = root.join("counts.json");
    let output = crate::support::unsigned_runx_command_with_inherited_cwd()
        .current_dir(dir)
        .env("RUNX_HOME", root.join("home"))
        .env("RUNX_PERF_COUNTS_PATH", &counts)
        .args(args)
        .output()?;
    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((
        serde_json::from_slice(&output.stdout)?,
        serde_json::from_slice(&fs::read(&counts)?)?,
    ))
}

/// `.` names the directory it's typed in: packaging `.` inside a package
/// packages that package, never the workspace around it, and reads as much
/// beside 20 other packages as beside none. The workspace root here is a
/// package itself, so packaging it would carry every package below it.
#[test]
fn packaging_dot_inside_a_package_packages_that_package_whatever_else_is_in_the_workspace()
-> TestResult {
    let package = |others: usize| -> TestResult<(Value, Value)> {
        let root = crate::support::temp_root(&format!("runx-work-counts-dot-{others}"));
        fs::create_dir_all(root.join("home"))?;
        fs::write(
            root.join("SKILL.md"),
            "---\nname: workspace\ndescription: The workspace's own package.\n---\n# Workspace\n",
        )?;
        native_package(&root, "inner", 1)?;
        for other in 0..others {
            native_package(&root, &format!("other-{other:03}"), 1)?;
        }
        launched_in(
            &root.join("skills/inner"),
            &root,
            &["registry", "package", ".", "--json"],
        )
    };
    let (alone, alone_counts) = package(0)?;
    let (among, among_counts) = package(20)?;
    for packaged in [&alone, &among] {
        let markdown = packaged["registry"]["package"]["markdown"]
            .as_str()
            .ok_or_else(|| format!("no packaged manual in {packaged}"))?;
        assert!(markdown.contains("name: inner"), "{markdown}");
    }
    assert_eq!(
        alone["registry"]["package"]["package_files"],
        among["registry"]["package"]["package_files"],
        "alone={alone}\namong={among}"
    );
    assert_eq!(
        count_of(&among_counts, "packages_read")?,
        count_of(&alone_counts, "packages_read")?,
        "alone={alone_counts}\namong={among_counts}"
    );
    Ok(())
}

/// A package at `root/skills/<name>` whose one method is `calls` agent steps,
/// each answered by the package's own test, and that test.
fn answered_package(root: &Path, name: &str, calls: usize) -> TestResult {
    let package = root.join("skills").join(name);
    fs::create_dir_all(package.join("tests"))?;
    fs::write(
        package.join("SKILL.md"),
        format!(
            "---\nname: {name}\ndescription: Asks for one word per call.\n---\n# {name}\n\n## Agent task: answer\n\nAnswer with one word.\n"
        ),
    )?;
    let steps = (0..calls)
        .map(|call| {
            format!(
                "      - id: call-{call}\n        agent: answer\n        outputs:\n          answer: {{type: string}}\n"
            )
        })
        .collect::<String>();
    fs::write(
        package.join("run.yaml"),
        format!("minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n{steps}"),
    )?;
    let answers = (0..calls)
        .map(|call| format!("    call-{call}: {{answer: yes}}\n"))
        .collect::<String>();
    fs::write(
        package.join("tests/answers.yaml"),
        format!(
            "name: answers\nmethod: run\nhost:\n  answers:\n{answers}expect:\n  status: sealed\n"
        ),
    )?;
    Ok(())
}

/// An agent call settles at the same cost at call 1,000 as at call 10: the
/// run's record is replayed and its calls read no more often in a run of
/// 1,000 agent calls than in a run of 10.
#[test]
fn an_agent_call_reads_the_run_record_no_more_at_call_1000_than_at_call_10() -> TestResult {
    let run = |calls: usize| -> TestResult<Value> {
        let root = crate::support::temp_root(&format!("runx-work-counts-agent-{calls}"));
        fs::create_dir_all(root.join("home"))?;
        answered_package(&root, "answers", calls)?;
        counted(&root, &["test", "skills/answers", "--json"])
    };
    let ten = run(10)?;
    let thousand = run(1000)?;
    assert_eq!(
        count_of(&thousand, "run_materializations")?,
        count_of(&ten, "run_materializations")?,
        "ten={ten}\nthousand={thousand}"
    );
    Ok(())
}

/// A package at `root/skills/<name>` whose one method is `calls` calls of
/// its own JavaScript module, and a test that runs it.
fn module_package(root: &Path, name: &str, calls: usize) -> TestResult {
    let package = root.join("skills").join(name);
    fs::create_dir_all(package.join("tests"))?;
    fs::write(
        package.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Calls its own module.\n---\n# {name}\n"),
    )?;
    fs::write(
        package.join("call.mjs"),
        "export default () => ({ ok: true });\n",
    )?;
    let steps = (0..calls)
        .map(|call| {
            format!(
                "      - id: call-{call}\n        tool: {{module: call.mjs}}\n        outputs:\n          ok: {{type: boolean}}\n"
            )
        })
        .collect::<String>();
    fs::write(
        package.join("run.yaml"),
        format!("minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n{steps}"),
    )?;
    fs::write(
        package.join("tests/calls.yaml"),
        "name: calls\nmethod: run\nexpect:\n  status: sealed\n",
    )?;
    Ok(())
}

/// A run resolves its package once: a run of 1,000 calls of the package's
/// own module reads the package as often as a run of 10.
#[test]
fn a_run_reads_its_package_no_more_at_call_1000_than_at_call_10() -> TestResult {
    let run = |calls: usize| -> TestResult<Value> {
        let root = crate::support::temp_root(&format!("runx-work-counts-reads-{calls}"));
        fs::create_dir_all(root.join("home"))?;
        module_package(&root, "reads", calls)?;
        counted(&root, &["test", "skills/reads", "--json"])
    };
    let ten = run(10)?;
    let thousand = run(1000)?;
    assert_eq!(
        count_of(&thousand, "packages_read")?,
        count_of(&ten, "packages_read")?,
        "ten={ten}\nthousand={thousand}"
    );
    Ok(())
}
