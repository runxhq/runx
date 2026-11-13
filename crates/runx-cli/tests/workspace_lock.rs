//! The workspace's lock, across runs (vocabulary-and-loops.md, §9's control
//! flow row): two runs' commands in one workspace never overlap, a runx
//! started under a command joins its hold and doesn't deadlock, and a process
//! a command leaves running holds nothing and gets no free pass. Each process
//! is kept where the test needs it by a named pipe, which blocks it until the
//! test opens the other end, and a run waiting on the lock says so on a pipe
//! of the test's (a test build's `RUNX_REACHED`), so nothing waits on a clock.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use crate::support::unsigned_runx_command_at;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A one-step skill at `root/<name>` whose process step runs `script`, a
/// file beside it, with the step's inputs `inputs` in its environment.
fn process_skill(root: &Path, name: &str, inputs: &[&str], script: &str) -> TestResult<PathBuf> {
    let skill = root.join(name);
    fs::create_dir_all(&skill)?;
    fs::write(
        skill.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: Runs a command.\n---\n# {name}\n\nRun it.\n"),
    )?;
    let declared = inputs
        .iter()
        .map(|input| format!("      {input}: {{type: string}}\n"))
        .collect::<String>();
    let given = inputs
        .iter()
        .map(|input| format!("          {input}: $input.{input}\n"))
        .collect::<String>();
    fs::write(
        skill.join("run.yaml"),
        format!(
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    inputs:\n{declared}    steps:\n      - id: run\n        tool: {{command: sh, args: [./run.sh]}}\n        inputs:\n{given}        outputs:\n          done: {{type: boolean}}\n"
        ),
    )?;
    fs::write(skill.join("run.sh"), script)?;
    Ok(skill)
}

/// A named pipe at `path`.
fn pipe(path: &Path) -> TestResult<PathBuf> {
    let made = Command::new("mkfifo").arg(path).status()?;
    if !made.success() {
        return Err(format!("mkfifo {} failed", path.display()).into());
    }
    Ok(path.to_path_buf())
}

/// Wait until whoever writes `pipe` says so.
fn said(pipe: &Path) -> TestResult {
    let mut said = String::new();
    fs::File::open(pipe)?.read_to_string(&mut said)?;
    assert_eq!(said, "x");
    Ok(())
}

/// Release whoever reads `pipe`.
fn release(pipe: &Path) -> TestResult {
    let mut pipe = fs::OpenOptions::new().write(true).open(pipe)?;
    writeln!(pipe)?;
    Ok(())
}

/// `runx skill <skill>` in `root` with `inputs`.
fn skill_command(root: &Path, skill: &Path, inputs: &[(&str, &Path)]) -> TestResult<Command> {
    let mut command = unsigned_runx_command_at(root);
    command.arg("skill").arg(skill);
    for (name, value) in inputs {
        command.arg("-i").arg(format!(
            "{name}={}",
            value.to_str().ok_or("a non-utf8 path")?
        ));
    }
    command
        .arg("--json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    Ok(command)
}

fn sealed(output: &Output) -> TestResult {
    let ran: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        format!(
            "{error}: stdout={}\nstderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })?;
    assert_eq!(ran["status"], "sealed", "{ran}");
    assert_eq!(ran["outcome"], "completed", "{ran}");
    Ok(())
}

fn finished(child: Child) -> TestResult {
    sealed(&child.wait_with_output()?)
}

/// The command that holds the workspace while it runs: it says it started,
/// waits to be released, then appends `a` to the log.
const UNTIL_RELEASED: &str = "#!/bin/sh\nset -eu\nprintf x > \"$RUNX_INPUT_STARTED\"\nread line < \"$RUNX_INPUT_RELEASE\"\nprintf a >> \"$RUNX_INPUT_LOG\"\nprintf '{\"done\":true}'\n";

/// A command that appends `b` to the log.
const APPEND: &str =
    "#!/bin/sh\nset -eu\nprintf b >> \"$RUNX_INPUT_LOG\"\nprintf '{\"done\":true}'\n";

/// Two runs' commands in one workspace never overlap: the second run's
/// command waits on the lock the first's holds, and runs once it returns.
#[test]
fn two_runs_commands_in_one_workspace_never_overlap() -> TestResult {
    let root = crate::support::temp_root("workspace-lock-overlap");
    fs::create_dir_all(root.join("home"))?;
    let blocking = process_skill(
        &root,
        "blocking",
        &["started", "release", "log"],
        UNTIL_RELEASED,
    )?;
    let append = process_skill(&root, "append", &["log"], APPEND)?;
    let (started, released, waiting) = (
        pipe(&root.join("started"))?,
        pipe(&root.join("release"))?,
        pipe(&root.join("waiting"))?,
    );
    let log = root.join("log");

    let holding = skill_command(
        &root,
        &blocking,
        &[("started", &started), ("release", &released), ("log", &log)],
    )?
    .spawn()?;
    said(&started)?;
    let appending = skill_command(&root, &append, &[("log", &log)])?
        .env(
            "RUNX_REACHED",
            format!("workspace_lock_wait@{}", waiting.display()),
        )
        .spawn()?;
    // The second run reached the lock and waits on it; its command hasn't
    // run.
    said(&waiting)?;
    assert!(!log.exists(), "the second command ran while the first held");

    release(&released)?;
    finished(holding)?;
    finished(appending)?;
    assert_eq!(fs::read_to_string(&log)?, "ab");
    Ok(())
}

/// A command that runs runx again, which joins the command's hold: the inner
/// run appends to the log and the command returns, with nothing waiting on
/// the lock its own command holds.
const NEST: &str = "#!/bin/sh\nset -eu\nRUNX_HOME=\"$RUNX_INPUT_HOME\" \"$RUNX_INPUT_RUNX\" skill \"$RUNX_INPUT_SKILL\" -i log=\"$RUNX_INPUT_LOG\" --json > /dev/null\nprintf '{\"done\":true}'\n";

/// A runx started under a command joins the command's hold, so it doesn't
/// deadlock waiting on the lock its own command holds.
#[test]
fn a_runx_started_under_a_command_joins_its_lock() -> TestResult {
    let root = crate::support::temp_root("workspace-lock-nested");
    fs::create_dir_all(root.join("home"))?;
    let append = process_skill(&root, "append", &["log"], APPEND)?;
    let nest = process_skill(&root, "nest", &["home", "runx", "skill", "log"], NEST)?;
    let log = root.join("log");
    let runx = PathBuf::from(env!("CARGO_BIN_EXE_runx"));
    finished(
        skill_command(
            &root,
            &nest,
            &[
                ("home", &root.join("home")),
                ("runx", &runx),
                ("skill", &append),
                ("log", &log),
            ],
        )?
        .spawn()?,
    )?;
    assert_eq!(fs::read_to_string(&log)?, "b");
    Ok(())
}

/// A command that runs `runx trust`, saying on `err` what runx said, and
/// answering whether runx refused it.
const TRUST: &str = "#!/bin/sh\nset -u\nif RUNX_HOME=\"$RUNX_INPUT_HOME\" \"$RUNX_INPUT_RUNX\" trust acme/echo@1.0.0 2> \"$RUNX_INPUT_ERR\"; then printf '{\"done\":false}'; else printf '{\"done\":true}'; fi\n";

/// Trust is the person's statement, so `runx trust` a run's command runs,
/// which carries the workspace lock's token, is refused, and nothing is
/// recorded in the home.
#[test]
fn runx_trust_inside_a_run_is_refused() -> TestResult {
    let root = crate::support::temp_root("workspace-lock-trust");
    fs::create_dir_all(root.join("home"))?;
    let trust = process_skill(&root, "trust", &["home", "runx", "err"], TRUST)?;
    let err = root.join("err");
    let runx = PathBuf::from(env!("CARGO_BIN_EXE_runx"));
    let output = skill_command(
        &root,
        &trust,
        &[("home", &root.join("home")), ("runx", &runx), ("err", &err)],
    )?
    .output()?;
    sealed(&output)?;
    let ran: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(ran["result"]["done"], true, "runx trust was refused: {ran}");
    let said = fs::read_to_string(&err)?;
    assert!(said.contains("inside a runx run"), "{said}");
    assert!(!root.join("home/trust").exists(), "nothing is recorded");
    Ok(())
}

/// A command that leaves a process running and returns: in a process group
/// of its own, detached explicitly rather than relying on shell job control,
/// so it outlives the command's. The
/// process waits on `go`, then runs runx under the command's old token,
/// saying on `waiting` if that run waits on the lock, and on `done` once it
/// ends.
const LEAVE: &str = r#"#!/bin/sh
set -eu
node - <<'NODE'
const { spawn } = require('node:child_process');
const child = spawn('sh', ['-c', `
  read line < "$RUNX_INPUT_GO"
  RUNX_HOME="$RUNX_INPUT_HOME" RUNX_REACHED="workspace_lock_wait@$RUNX_INPUT_WAITING" "$RUNX_INPUT_RUNX" skill "$RUNX_INPUT_SKILL" -i log="$RUNX_INPUT_LOG" --json > /dev/null
  printf x > "$RUNX_INPUT_DONE"
`], { detached: true, stdio: 'ignore' });
child.unref();
process.stdout.write('{"done":true}');
NODE
"#;

/// A process a command leaves running holds nothing: the next run's command
/// proceeds once the first returns. And it gets no free pass: runx it starts
/// later, under the command's old token, takes the lock as any run does, so
/// it waits on another run's command that holds it.
#[test]
fn a_process_a_command_leaves_running_takes_nothing_and_gets_no_free_pass() -> TestResult {
    let root = crate::support::temp_root("workspace-lock-left-running");
    fs::create_dir_all(root.join("home"))?;
    let append = process_skill(&root, "append", &["log"], APPEND)?;
    let blocking = process_skill(
        &root,
        "blocking",
        &["started", "release", "log"],
        UNTIL_RELEASED,
    )?;
    let leave = process_skill(
        &root,
        "leave",
        &["go", "home", "runx", "skill", "log", "waiting", "done"],
        LEAVE,
    )?;
    let (go, waiting, done, started, released) = (
        pipe(&root.join("go"))?,
        pipe(&root.join("waiting"))?,
        pipe(&root.join("done"))?,
        pipe(&root.join("started"))?,
        pipe(&root.join("release"))?,
    );
    let log = root.join("log");
    let runx = PathBuf::from(env!("CARGO_BIN_EXE_runx"));
    finished(
        skill_command(
            &root,
            &leave,
            &[
                ("go", &go),
                ("home", &root.join("home")),
                ("runx", &runx),
                ("skill", &append),
                ("log", &log),
                ("waiting", &waiting),
                ("done", &done),
            ],
        )?
        .spawn()?,
    )?;

    // The process it left running holds nothing: another run's command
    // takes the lock and runs.
    let holding = skill_command(
        &root,
        &blocking,
        &[("started", &started), ("release", &released), ("log", &log)],
    )?
    .spawn()?;
    said(&started)?;

    // Its runx, under the old token, waits on the lock that command holds.
    release(&go)?;
    said(&waiting)?;
    assert!(!log.exists(), "the late run joined a hold that had ended");
    release(&released)?;
    finished(holding)?;
    said(&done)?;
    assert_eq!(fs::read_to_string(&log)?, "ab");
    Ok(())
}
