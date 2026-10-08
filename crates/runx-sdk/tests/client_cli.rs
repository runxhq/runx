use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{JsonObject, JsonValue};
use runx_sdk::{ContinuePayload, RunSkillOptions, RunxClient, RunxClientOptions};

#[test]
fn search_and_run_use_runx_cli_json() -> Result<(), Box<dyn std::error::Error>> {
    let fixture = CliFixture::create()?;
    let client = fixture.client();

    let results = client.search_skills("sourcey", Some("registry"))?;
    let report = client.run_skill(
        "skills/example",
        RunSkillOptions {
            ..RunSkillOptions::default().with_input("message", "hi")
        },
    )?;

    assert_eq!(results[0].skill_id, "acme/sourcey");
    assert_eq!(results[0].required_scopes, vec!["repo:read".to_owned()]);
    assert_eq!(report.status(), Some("success"));
    assert_eq!(
        fs::read_to_string(fixture.args_path())?,
        "skill\nskills/example\n--message\nhi\n--json\n"
    );
    Ok(())
}

#[test]
fn continue_run_writes_answers_file_for_canonical_skill_rerun()
-> Result<(), Box<dyn std::error::Error>> {
    let fixture = CliFixture::create()?;
    let client = fixture.client();
    let mut answer = JsonObject::new();
    answer.insert("ok".to_owned(), JsonValue::Bool(true));

    let report = client.continue_run(
        "skills/example",
        "run-123",
        ContinuePayload::default()
            .with_answer("req-1", JsonValue::Object(answer))
            .with_approval("gate-1", true),
    )?;

    assert_eq!(report.status(), Some("sealed"));
    let args = fs::read_to_string(fixture.args_path())?;
    assert!(args.starts_with("resume\nrun-123\n"));
    assert!(args.ends_with("\n--json\n"));
    assert_eq!(
        fs::read_to_string(fixture.stdin_path())?,
        r#"{"answers":{"req-1":{"ok":true}},"approvals":{"gate-1":true}}"#
    );
    Ok(())
}

struct CliFixture {
    root: PathBuf,
    command: PathBuf,
}

impl CliFixture {
    fn create() -> Result<Self, Box<dyn std::error::Error>> {
        let root = unique_temp_dir()?;
        fs::create_dir_all(&root)?;
        let command = root.join("fake-runx");
        fs::write(&command, fake_runx_script())?;
        make_executable(&command)?;
        Ok(Self { root, command })
    }

    fn client(&self) -> RunxClient {
        RunxClient::with_options(RunxClientOptions {
            command: vec![self.command.to_string_lossy().into_owned()],
            cwd: None,
            env: BTreeMap::from([
                (
                    "RUNX_SDK_ARGS".to_owned(),
                    self.args_path().to_string_lossy().into_owned(),
                ),
                (
                    "RUNX_SDK_STDIN".to_owned(),
                    self.stdin_path().to_string_lossy().into_owned(),
                ),
            ]),
        })
    }

    fn args_path(&self) -> PathBuf {
        self.root.join("args.txt")
    }

    fn stdin_path(&self) -> PathBuf {
        self.root.join("stdin.json")
    }
}

impl Drop for CliFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn unique_temp_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    Ok(std::env::temp_dir().join(format!("runx-sdk-test-{}-{nanos}-{id}", std::process::id())))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

fn fake_runx_script() -> &'static str {
    r#"#!/bin/sh
printf '%s\n' "$@" > "$RUNX_SDK_ARGS"
if [ "$1" = "skill" ] && [ "$2" = "search" ]; then
  printf '%s\n' '{"status":"success","results":[{"skill_id":"acme/sourcey","name":"sourcey","owner":"acme","source":"runx-registry","source_label":"runx registry","trust_tier":"community","required_scopes":["repo:read"],"tags":["docs"],"version":"1.0.0"}]}'
elif [ "$1" = "resume" ]; then
  cat "$3" > "$RUNX_SDK_STDIN"
  printf '%s\n' '{"status":"sealed","args":["skill"]}'
else
  printf '%s\n' '{"status":"success","args":["skill"]}'
fi
"#
}
