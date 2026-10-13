// Each integration test compiles this module separately and uses a different helper subset.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
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

/// One project per test: nextest runs each test in its own process, and
/// `cargo test` runs each on its own thread, so per-thread is per-test under
/// both and the commands of one test still share their project.
fn test_project_dir() -> PathBuf {
    thread_local! {
        static PROJECT: PathBuf = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("runx-cli-projects")
            .join(unique_temp_suffix())
            .join(".runx");
    }
    PROJECT.with(Clone::clone)
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
/// may additionally emit the human-readable operator preflight on stderr:
/// compact, or under `--diagnostics` followed by the full operator context.
/// Accept only those exact channel shapes so tests do not hide arbitrary
/// warnings.
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
    if lines.get(cursor) == Some(&"") {
        require_stderr_line(&lines, &mut cursor, "")?;
        require_stderr_line(&lines, &mut cursor, "Full operator context")?;
        return Ok(());
    }
    require_stderr_line(&lines, &mut cursor, "  Full context: add --diagnostics")?;
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
            reason: None,
            act_ids: None,
            decision_ids: None,
            child_receipt_refs: None,
            child_receipt_count: None,
            verification_refs: None,
        });
    }
    Ok(serde_json::to_string(&fixture)?)
}

/// How a [`MockConnect`] answers a provider operation's request body: with
/// the response to write, or `None` to close the connection unanswered, as a
/// transport failure after the provider may have acted.
pub type ProviderOperations =
    Box<dyn FnMut(&serde_json::Value) -> Option<serde_json::Value> + Send>;

/// A runx Connect the operator's runs reach on loopback: it authenticates
/// `principal_id` and lists `grants`, each `(grant id, connector, scopes)`
/// with the connector's exact binding. A provider operation is refused
/// unless [`MockConnect::with_operations`] answers it, so a run that sends
/// an unexpected request fails loudly.
pub struct MockConnect {
    pub url: String,
}

impl MockConnect {
    pub fn serve(
        principal_id: &str,
        grants: &[(&str, &str, &[&str])],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::serve_with(principal_id, grants, None)
    }

    pub fn with_operations(
        principal_id: &str,
        grants: &[(&str, &str, &[&str])],
        operations: ProviderOperations,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::serve_with(principal_id, grants, Some(operations))
    }

    fn serve_with(
        principal_id: &str,
        grants: &[(&str, &str, &[&str])],
        mut operations: Option<ProviderOperations>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use std::io::Write;

        // The runx source tree, where every runx-maintained connector is in reach.
        let scope = runx_runtime::ConnectorScope::at(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."),
        )?;
        let grants = grants
            .iter()
            .map(|(grant_id, connector, scopes)| {
                let connector = scope.connector(connector)?;
                Ok(serde_json::json!({
                    "grant_id": grant_id,
                    "provider": connector.manifest.id.as_str(),
                    "connector_binding": connector.binding(),
                    "status": "active",
                    "scopes": scopes,
                }))
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        let me =
            serde_json::json!({"status": "success", "principal": {"principal_id": principal_id}})
                .to_string();
        let listed = serde_json::json!({"status": "success", "grants": grants}).to_string();
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Ok((request_line, body)) = read_http_request(&mut stream) else {
                    continue;
                };
                let answered;
                let (status, body) = if request_line.starts_with("GET /v1/me ") {
                    ("200 OK", me.as_str())
                } else if request_line.starts_with("GET /v1/grants ") {
                    ("200 OK", listed.as_str())
                } else if let (true, Some(operations)) = (
                    request_line.starts_with("POST /v1/provider-operations "),
                    operations.as_mut(),
                ) {
                    let Ok(request) = serde_json::from_slice(&body) else {
                        continue;
                    };
                    let Some(response) = operations(&request) else {
                        continue;
                    };
                    answered = response.to_string();
                    ("200 OK", answered.as_str())
                } else {
                    (
                        "404 Not Found",
                        r#"{"status":"error","error":{"code":"not_found"}}"#,
                    )
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Ok(Self { url })
    }

    /// Point `command` at this Connect.
    pub fn apply(&self, command: &mut Command) {
        command
            .env("RUNX_PUBLIC_API_BASE_URL", &self.url)
            .env("RUNX_PUBLIC_API_TOKEN", "rxk_operator")
            .env("RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK", "1");
    }
}

/// A hosted registry on loopback that answers fixed `GET` routes with JSON;
/// any other request is a 404.
pub struct MockRegistry {
    pub url: String,
}

impl MockRegistry {
    pub fn serve(routes: &[(&str, serde_json::Value)]) -> Result<Self, Box<dyn std::error::Error>> {
        use std::io::Write;

        let routes = routes
            .iter()
            .map(|(path, body)| (format!("GET {path} "), body.to_string()))
            .collect::<Vec<_>>();
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}", listener.local_addr()?);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let Ok((request_line, _body)) = read_http_request(&mut stream) else {
                    continue;
                };
                let (status, body) = routes
                    .iter()
                    .find(|(route, _)| request_line.starts_with(route.as_str()))
                    .map_or(
                        (
                            "404 Not Found",
                            r#"{"status":"error","error":{"code":"not_found"}}"#,
                        ),
                        |(_, body)| ("200 OK", body.as_str()),
                    );
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Ok(Self { url })
    }
}

/// Read one HTTP request: its request line and its `content-length` body.
fn read_http_request(stream: &mut std::net::TcpStream) -> std::io::Result<(String, Vec<u8>)> {
    use std::io::Read;

    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut expected = None;
    while expected.is_none_or(|expected| bytes.len() < expected) {
        let length = stream.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..length]);
        if expected.is_none()
            && let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n")
        {
            let length = String::from_utf8_lossy(&bytes[..end])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            expected = Some(end + 4 + length);
        }
    }
    let end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(bytes.len());
    let request_line = String::from_utf8_lossy(&bytes[..end])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    let body = bytes.get(end + 4..).unwrap_or_default().to_vec();
    Ok((request_line, body))
}

/// Makes `root` a runx plugin holding copies of the runx source connectors
/// `ids`: how a project reaches published connectors without a registry.
/// Copy a directory tree.
pub fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

pub fn plugin_with_source_connectors(
    root: &std::path::Path,
    ids: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(root)?;
    std::fs::write(root.join("plugin.json"), r#"{"namespace":"runx"}"#)?;
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../connectors");
    for id in ids {
        copy_dir(&source.join(id), &root.join("connectors").join(id))?;
    }
    Ok(())
}
