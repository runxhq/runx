// Each integration test compiles this module separately and uses a different helper subset.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::ReceiptSchema;
use runx_runtime::harness::{HarnessFixture, ReceiptExpectation};

const FIXTURE_SIGNING_SEED: &str = "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=";
static NEXT_TEMP_ROOT: AtomicU64 = AtomicU64::new(0);

pub fn repo_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

pub fn temp_root(prefix: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("{prefix}-{}", unique_temp_suffix()));
    if root.exists() {
        let _ignored = fs::remove_dir_all(&root);
    }
    root
}

pub fn isolated_target_temp_root(prefix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = repo_root()?
        .join("crates")
        .join("target")
        .join(prefix)
        .join(unique_temp_suffix());
    fs::remove_dir_all(&path).ok();
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn unique_temp_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = NEXT_TEMP_ROOT.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

pub fn signed_runx_command(signing_key_id: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env("NO_COLOR", "1");
    isolate_project(&mut command);
    apply_fixture_signing(&mut command, signing_key_id);
    command
}

/// `runx` resolving the repository as its workspace, with its own project
/// state.
pub fn repo_runx_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env("NO_COLOR", "1");
    if let Ok(root) = repo_root() {
        command.env("RUNX_CWD", root);
    }
    isolate_project(&mut command);
    command
}

pub fn isolated_runx_command(signing_key_id: &str) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = isolated_runx_command_with_inherited_cwd(signing_key_id);
    command.current_dir(repo_root()?);
    Ok(command)
}

pub fn isolated_runx_command_with_inherited_cwd(signing_key_id: &str) -> Command {
    let mut command = unsigned_runx_command_with_inherited_cwd();
    isolate_project(&mut command);
    apply_fixture_signing(&mut command, signing_key_id);
    command
}

/// Keep a command's project state (kernel store, runs, custody) in this test
/// process's own directory, so a command run from inside the repository
/// never writes the repository's `.runx`. Call it after any `env_clear`.
pub fn isolate_project(command: &mut Command) {
    command.env("RUNX_PROJECT_DIR", test_project_dir());
}

/// nextest runs each test in its own process, so each test gets its own.
fn test_project_dir() -> &'static Path {
    static PROJECT: OnceLock<PathBuf> = OnceLock::new();
    PROJECT.get_or_init(|| {
        Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("runx-cli-projects")
            .join(unique_temp_suffix())
            .join(".runx")
    })
}

pub fn unsigned_runx_command_with_inherited_cwd() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("NO_COLOR", "1");
    command
}

pub fn unsigned_runx_command_at(root: &Path) -> Command {
    let mut command = unsigned_runx_command_with_inherited_cwd();
    command
        .current_dir(root)
        .env("RUNX_HOME", root.join("home"));
    command
}

pub fn apply_fixture_signing(command: &mut Command, signing_key_id: &str) {
    command.env("RUNX_RECEIPT_SIGN_KID", signing_key_id);
    command.env(
        "RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64",
        FIXTURE_SIGNING_SEED,
    );
    command.env("RUNX_RECEIPT_SIGN_ISSUER_TYPE", "hosted");
}

/// JSON commands keep their machine envelope on stdout. A governed skill run
/// may additionally emit the compact, human-readable operator preflight on
/// stderr. Accept only those two exact channel shapes so tests do not hide
/// arbitrary warnings or diagnostics.
pub fn assert_json_stderr(stderr: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let stderr = std::str::from_utf8(stderr)?;
    if stderr.is_empty() {
        return Ok(());
    }
    let Some(body) = stderr.strip_suffix("\n\n") else {
        return Err(format!("unexpected stderr for JSON command:\n{stderr}").into());
    };
    let lines = body.split('\n').collect::<Vec<_>>();
    let mut cursor = 0;
    require_stderr_line(&lines, &mut cursor, "Prepared run")?;
    require_stderr_prefix(&lines, &mut cursor, "  Skill:  ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Runner: ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Source: ")?;
    if lines
        .get(cursor)
        .is_some_and(|line| line.starts_with("  Run:    "))
    {
        require_stderr_prefix(&lines, &mut cursor, "  Run:    ")?;
    }
    if lines
        .get(cursor)
        .is_some_and(|line| line.starts_with("  Receipts: "))
    {
        require_stderr_prefix(&lines, &mut cursor, "  Receipts: ")?;
    }
    require_stderr_prefix(&lines, &mut cursor, "  Steps:  ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Tools:  ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Boundaries: ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Managed agent: ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Inputs: ")?;
    require_stderr_prefix(&lines, &mut cursor, "  Digest: sha256:")?;
    require_stderr_line(
        &lines,
        &mut cursor,
        "  Full context: add --full-operator-context",
    )?;
    if cursor != lines.len() {
        return Err(format!("unexpected stderr for JSON command:\n{stderr}").into());
    }
    Ok(())
}

fn require_stderr_line(lines: &[&str], cursor: &mut usize, expected: &str) -> Result<(), String> {
    match lines.get(*cursor) {
        Some(actual) if *actual == expected => {
            *cursor += 1;
            Ok(())
        }
        actual => Err(format!(
            "expected stderr line {expected:?}, received {actual:?}"
        )),
    }
}

fn require_stderr_prefix(lines: &[&str], cursor: &mut usize, prefix: &str) -> Result<(), String> {
    match lines.get(*cursor) {
        Some(actual) if actual.starts_with(prefix) && actual.len() > prefix.len() => {
            *cursor += 1;
            Ok(())
        }
        actual => Err(format!(
            "expected stderr line prefixed by {prefix:?}, received {actual:?}"
        )),
    }
}

pub struct GovernedHarnessFixture {
    path: PathBuf,
    root: PathBuf,
}

impl GovernedHarnessFixture {
    pub fn path_str(&self) -> Result<&str, Box<dyn std::error::Error>> {
        self.path
            .to_str()
            .ok_or_else(|| "non-utf8 governed harness path".into())
    }

    pub fn receipt_dir(&self) -> PathBuf {
        self.root.join(".runx").join("receipts")
    }
}

impl Drop for GovernedHarnessFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).ok();
    }
}

pub fn governed_harness_fixture(
    fixture: &str,
) -> Result<GovernedHarnessFixture, Box<dyn std::error::Error>> {
    let repo = repo_root()?;
    let source_path = repo.join(fixture);
    let parent = source_path
        .parent()
        .ok_or("harness fixture path has no parent")?;
    let root = isolated_target_temp_root("governed-harness")?;
    let file_name = source_path
        .file_name()
        .ok_or("harness fixture path has no file name")?;
    let path = root.join(file_name);
    let fixture = runx_runtime::load_harness_fixture(&source_path)?;
    fs::write(&path, governed_harness_document(fixture, parent)?)?;
    Ok(GovernedHarnessFixture { path, root })
}

pub fn write_agent_task_skill(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("issue-intake");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: issue-intake\ndescription: issue-intake fixture.\n---\n# Issue Intake\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"
skill: issue-intake
runners:
  intake:
    default: true
    inputs:
      thread_title:
        type: string
        required: false
    steps:
      - id: intake
        agent: { role: builder, task: issue-intake }
        inputs:
          thread_title: $input.thread_title
        outputs:
          intake_report: object
"#,
    )?;
    Ok(skill_dir)
}

fn governed_harness_document(
    mut fixture: HarnessFixture,
    fixture_parent: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    if !fixture.target.is_empty() {
        let target = Path::new(&fixture.target);
        fixture.target = if target.is_absolute() {
            target.to_path_buf()
        } else {
            fixture_parent.join(target)
        }
        .canonicalize()?
        .to_string_lossy()
        .into_owned();
    }
    if fixture.expect.receipt.is_some() {
        fixture.expect.receipt = Some(ReceiptExpectation {
            schema: ReceiptSchema::V1,
            body_digest: None,
            receipt_id: None,
            receipt_digest: None,
            subject_ref: None,
            state: None,
            disposition: None,
            reason_code: None,
            act_ids: None,
            decision_ids: None,
            child_receipt_refs: None,
            child_receipt_count: None,
            verification_refs: None,
        });
    }
    Ok(serde_json::to_string(&fixture)?)
}
