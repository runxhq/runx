// Module rationale: this binary is the fixture oracle transaction:
// it replays harness fixtures, signs canonical receipts, and compares committed
// root/step oracles in one reviewable regeneration boundary.
use std::error::Error;
use std::ffi::OsString;
use std::fmt::{Display, Formatter};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use runx_contracts::Receipt;
use runx_receipts::{
    canonical_receipt_body_digest, canonical_receipt_digest, canonical_receipt_json,
};
use runx_runtime::harness::{HarnessFixtureCase, list_cases};
use runx_runtime::{HarnessReplayOutput, replay_harness_fixture, run_harness_fixture};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _ignored = writeln!(io::stderr().lock(), "runx: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse(std::env::args_os().skip(1))?;
    let mut failed = false;
    let mut summaries = Vec::new();

    for fixture in list_cases() {
        let summary = process_fixture(&cli, fixture)?;
        failed |= summary.status == SummaryStatus::Failed;
        summaries.push(summary);
    }

    if cli.summary_json {
        let report = SummaryReport {
            schema: "runx.harness_fixture_replay_summary.v1",
            cases: summaries,
        };
        write_stdout_line(&serde_json::to_string_pretty(&report)?)?;
    }

    if failed {
        Err(Box::new(MessageError(
            "one or more harness fixture oracles are stale".to_owned(),
        )))
    } else {
        Ok(())
    }
}

fn process_fixture(
    cli: &Cli,
    fixture: &HarnessFixtureCase,
) -> Result<FixtureSummary, Box<dyn Error>> {
    let started = Instant::now();
    let fixture_path = cli.repo_root.join(fixture.fixture_path);
    // The case runs the tools `runx harness` runs, so the oracles are what
    // `runx harness` seals. Writing regenerates the expectations this run
    // produces, so it replays the case without asserting them; checking
    // asserts them all.
    let replay = if cli.write {
        replay_harness_fixture
    } else {
        run_harness_fixture
    };
    let output = match replay(&fixture_path, std::env::vars().collect()) {
        Ok(output) => output,
        Err(error) => {
            return Ok(FixtureSummary::failed(
                fixture.name,
                started.elapsed().as_millis(),
                FailureClassification::ReplayError,
                Some(error.to_string()),
            ));
        }
    };
    let receipt_digest = canonical_receipt_digest(&output.receipt)?;
    let root = CheckedReceipt {
        oracle_path: cli.repo_root.join(fixture.root_oracle_path),
        receipt: &output.receipt,
    };
    let root_stale = process_receipt(cli, &root)?;
    let steps_stale = process_step_oracles(cli, fixture, &output)?;
    let digest_stale = check_fixture_digests(cli, fixture, &output.receipt)?;
    let failure_classification = if root_stale || steps_stale {
        Some(FailureClassification::OracleStale)
    } else if digest_stale {
        Some(FailureClassification::DigestStale)
    } else {
        None
    };
    Ok(FixtureSummary {
        name: fixture.name,
        status: if failure_classification.is_some() {
            SummaryStatus::Failed
        } else {
            SummaryStatus::Passed
        },
        elapsed_ms: started.elapsed().as_millis(),
        receipt_id: Some(output.receipt.id.to_string()),
        receipt_digest: Some(receipt_digest),
        failure_classification,
        error: None,
    })
}

fn process_step_oracles(
    cli: &Cli,
    fixture: &HarnessFixtureCase,
    output: &HarnessReplayOutput,
) -> Result<bool, Box<dyn Error>> {
    let mut failed = false;
    for (index, step) in fixture.step_oracles.iter().enumerate() {
        let receipt = output.step_receipts.get(index).ok_or_else(|| {
            MessageError(format!(
                "{} did not emit step receipt {}",
                fixture.name, step.step_id
            ))
        })?;
        let checked = CheckedReceipt {
            oracle_path: cli.repo_root.join(step.oracle_path),
            receipt,
        };
        failed |= process_receipt(cli, &checked)?;
    }
    if output.step_receipts.len() != fixture.step_oracles.len() {
        return Err(Box::new(MessageError(format!(
            "{} emitted {} step receipts, expected {}",
            fixture.name,
            output.step_receipts.len(),
            fixture.step_oracles.len()
        ))));
    }
    Ok(failed)
}

fn process_receipt(cli: &Cli, checked: &CheckedReceipt<'_>) -> Result<bool, Box<dyn Error>> {
    let canonical = format!("{}\n", canonical_receipt_json(checked.receipt)?);
    if cli.write {
        if let Some(parent) = checked.oracle_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&checked.oracle_path, canonical)?;
        return Ok(false);
    }

    let expected = match fs::read_to_string(&checked.oracle_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_stderr_line(&format!(
                "missing harness oracle {}",
                cli.relative(&checked.oracle_path).display()
            ))?;
            return Ok(true);
        }
        Err(error) => return Err(Box::new(error)),
    };

    if expected != canonical {
        write_stderr_line(&format!(
            "stale harness oracle {}",
            cli.relative(&checked.oracle_path).display()
        ))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

fn check_fixture_digests(
    cli: &Cli,
    fixture: &HarnessFixtureCase,
    receipt: &Receipt,
) -> Result<bool, Box<dyn Error>> {
    let body_digest = canonical_receipt_body_digest(receipt)?;
    let receipt_digest = canonical_receipt_digest(receipt)?;
    let fixture_path = cli.repo_root.join(fixture.fixture_path);

    if cli.write {
        let child_receipt_refs = receipt
            .lineage
            .children
            .iter()
            .map(|reference| reference.uri.as_str())
            .collect::<Vec<_>>();
        rewrite_fixture_receipt_expectations(
            &fixture_path,
            &body_digest,
            &receipt_digest,
            &child_receipt_refs,
        )?;
        write_stdout_line(&format!("{} body_digest={body_digest}", fixture.name))?;
        write_stdout_line(&format!("{} receipt_digest={receipt_digest}", fixture.name))?;
        return Ok(false);
    }

    let contents = fs::read_to_string(&fixture_path)?;
    let mut failed = false;

    if !contents.contains(&format!("body_digest: {body_digest}")) {
        write_stderr_line(&format!(
            "stale body_digest in {}",
            cli.relative(&fixture_path).display()
        ))?;
        failed = true;
    }
    if !contents.contains(&format!("receipt_digest: {receipt_digest}")) {
        write_stderr_line(&format!(
            "stale receipt_digest in {}",
            cli.relative(&fixture_path).display()
        ))?;
        failed = true;
    }

    Ok(failed)
}

fn rewrite_fixture_receipt_expectations(
    fixture_path: &Path,
    body_digest: &str,
    receipt_digest: &str,
    child_receipt_refs: &[&str],
) -> Result<(), Box<dyn Error>> {
    let contents = fs::read_to_string(fixture_path)?;
    let ends_with_newline = contents.ends_with('\n');
    let mut lines = contents.lines().peekable();
    let mut rewritten = Vec::new();

    while let Some(line) = lines.next() {
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("body_digest:") {
            rewritten.push(format!("{}body_digest: {body_digest}", &line[..indent]));
            continue;
        }
        if trimmed.starts_with("receipt_digest:") {
            rewritten.push(format!(
                "{}receipt_digest: {receipt_digest}",
                &line[..indent]
            ));
            continue;
        }
        if trimmed == "child_receipt_refs:" {
            rewritten.push(line.to_owned());
            while lines.peek().is_some_and(|next| {
                let next_indent = next.len() - next.trim_start().len();
                next_indent > indent && next.trim_start().starts_with("- ")
            }) {
                let _discarded = lines.next();
            }
            rewritten.extend(
                child_receipt_refs
                    .iter()
                    .map(|reference| format!("{}  - {reference}", &line[..indent])),
            );
            continue;
        }
        rewritten.push(line.to_owned());
    }

    let mut output = rewritten.join("\n");
    if ends_with_newline {
        output.push('\n');
    }
    fs::write(fixture_path, output)?;
    Ok(())
}

fn write_stdout_line(message: &str) -> io::Result<()> {
    writeln!(io::stdout().lock(), "{message}")
}

fn write_stderr_line(message: &str) -> io::Result<()> {
    writeln!(io::stderr().lock(), "{message}")
}

struct Cli {
    repo_root: PathBuf,
    write: bool,
    summary_json: bool,
}

impl Cli {
    fn parse(args: impl Iterator<Item = OsString>) -> Result<Self, Box<dyn Error>> {
        let mut repo_root = std::env::current_dir()?;
        let mut write = false;
        let mut check = false;
        let mut summary_json = false;
        let mut args = args.peekable();

        while let Some(arg) = args.next() {
            let token = arg
                .to_str()
                .ok_or_else(|| MessageError("harness oracle arguments must be UTF-8".to_owned()))?;
            match token {
                "--write" | "--generate" => write = true,
                "--check" => check = true,
                "--summary-json" => summary_json = true,
                "--repo-root" => {
                    let value = args
                        .next()
                        .ok_or_else(|| MessageError("--repo-root requires a path".to_owned()))?;
                    repo_root = PathBuf::from(value);
                }
                "--help" | "-h" => {
                    return Err(Box::new(MessageError(usage())));
                }
                _ => {
                    return Err(Box::new(MessageError(format!(
                        "unknown harness oracle argument {token}\n{}",
                        usage()
                    ))));
                }
            }
        }

        if write && check {
            return Err(Box::new(MessageError(
                "use either --write or --check, not both".to_owned(),
            )));
        }

        Ok(Self {
            repo_root,
            write: write && !check,
            summary_json,
        })
    }

    fn relative(&self, path: &Path) -> PathBuf {
        path.strip_prefix(&self.repo_root)
            .map_or_else(|_| path.to_path_buf(), Path::to_path_buf)
    }
}

fn usage() -> String {
    "usage: runx-harness-fixture-oracles [--check|--write] [--summary-json] [--repo-root path]"
        .to_owned()
}

struct CheckedReceipt<'a> {
    oracle_path: PathBuf,
    receipt: &'a Receipt,
}

#[derive(serde::Serialize)]
struct SummaryReport {
    schema: &'static str,
    cases: Vec<FixtureSummary>,
}

#[derive(serde::Serialize)]
struct FixtureSummary {
    name: &'static str,
    status: SummaryStatus,
    elapsed_ms: u128,
    receipt_id: Option<String>,
    receipt_digest: Option<String>,
    failure_classification: Option<FailureClassification>,
    error: Option<String>,
}

impl FixtureSummary {
    fn failed(
        name: &'static str,
        elapsed_ms: u128,
        failure_classification: FailureClassification,
        error: Option<String>,
    ) -> Self {
        Self {
            name,
            status: SummaryStatus::Failed,
            elapsed_ms,
            receipt_id: None,
            receipt_digest: None,
            failure_classification: Some(failure_classification),
            error,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum SummaryStatus {
    Passed,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum FailureClassification {
    ReplayError,
    OracleStale,
    DigestStale,
}

#[derive(Debug)]
struct MessageError(String);

impl Display for MessageError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for MessageError {}
