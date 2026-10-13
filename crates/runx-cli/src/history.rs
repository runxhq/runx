use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli_args;
use runx_runtime::receipts::inspection::{ReceiptInspectionProjection, inspect_local_receipt};
use runx_runtime::run_record::{RunFilter, RunListing, RunRecordError, RunState, list_runs};
use runx_runtime::{
    LocalReceiptStore, ReceiptPathInputs, ReceiptStoreError, ReceiptTrust, RuntimeReceiptConfig,
    RuntimeReceiptSignaturePolicy, receipt_verifier, resolve_receipt_path,
};

// Module rationale: the native history command keeps
// parsing, rendering, and CLI tests together.
#[derive(Debug)]
pub enum HistoryCliError {
    InvalidArgs(String),
    InvalidReceiptVerifier(String),
    Runs(RunRecordError),
    Receipt(ReceiptStoreError),
    Serialize(serde_json::Error),
}

impl fmt::Display for HistoryCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgs(message) | Self::InvalidReceiptVerifier(message) => {
                formatter.write_str(message)
            }
            Self::Runs(error) => write!(formatter, "{error}"),
            Self::Receipt(error) => write!(formatter, "{error}"),
            Self::Serialize(error) => write!(formatter, "failed to serialize history: {error}"),
        }
    }
}

impl std::error::Error for HistoryCliError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryCliResult {
    pub output: String,
    pub error_is_usage: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParsedHistoryArgs {
    receipt_dir: Option<PathBuf>,
    query: Option<String>,
    filter: RunFilter,
    detail: bool,
    json: bool,
}

pub fn run_history_command(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<HistoryCliResult, HistoryCliError> {
    let parsed = parse_history_args(args)?;
    if parsed.detail {
        return run_receipt_detail(&parsed, env, cwd);
    }
    let listing = list_runs(env, cwd, &parsed.filter).map_err(HistoryCliError::Runs)?;
    let output = if parsed.json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&listing).map_err(HistoryCliError::Serialize)?
        )
    } else {
        render_history(
            &listing,
            parsed.query.as_deref(),
            parsed.receipt_dir.as_deref(),
        )
    };
    Ok(successful_result(output))
}

/// One receipt from the receipt directory, inspected and verified.
fn run_receipt_detail(
    parsed: &ParsedHistoryArgs,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<HistoryCliResult, HistoryCliError> {
    let receipt_reference = parsed.query.as_deref().ok_or_else(|| {
        HistoryCliError::InvalidArgs("history needs one receipt id to inspect".to_owned())
    })?;
    let filter = &parsed.filter;
    if filter.skill.is_some()
        || filter.state.is_some()
        || filter.since.is_some()
        || filter.until.is_some()
        || filter.limit.is_some()
    {
        return Err(HistoryCliError::InvalidArgs(
            "history <receipt-id> takes no list filters".to_owned(),
        ));
    }
    let receipt_config = RuntimeReceiptConfig::default();
    let paths = runx_runtime::RunxPaths::resolve(env, cwd);
    let resolved = resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: parsed.receipt_dir.as_deref(),
        runtime_config: Some(&receipt_config),
        paths: &paths,
    });
    let store = LocalReceiptStore::new(&resolved.path);
    let verifier = receipt_verifier(env, &paths, ReceiptTrust::HomeAndConfigured)
        .map_err(|error| HistoryCliError::InvalidReceiptVerifier(error.to_string()))?;
    let inspection = inspect_local_receipt(
        &store,
        &paths,
        receipt_reference,
        RuntimeReceiptSignaturePolicy::verifying(&verifier),
    )
    .map_err(HistoryCliError::Receipt)?;
    let output = if parsed.json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&inspection).map_err(HistoryCliError::Serialize)?
        )
    } else {
        render_receipt_inspection(&inspection)
    };
    Ok(successful_result(output))
}

fn successful_result(output: String) -> HistoryCliResult {
    HistoryCliResult {
        output,
        error_is_usage: false,
    }
}

// Function rationale: the public history CLI flag grammar
// lives in one parser.
fn parse_history_args(args: &[OsString]) -> Result<ParsedHistoryArgs, HistoryCliError> {
    if args.first().and_then(|arg| arg.to_str()) != Some("history") {
        return Err(HistoryCliError::InvalidArgs(
            "internal error: history dispatcher received non-history command".to_owned(),
        ));
    }
    let mut parsed = ParsedHistoryArgs::default();
    let mut positionals = Vec::new();
    let mut index = 1;
    while index < args.len() {
        let token = cli_args::os_arg(args, index, "history").map_err(invalid_args)?;
        // `-j` is the one short flag; anything else without `--` searches.
        if token != "-j" && !token.starts_with("--") {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }
        let (flag, inline_value) = cli_args::split_flag(token);
        match flag {
            "--json" | "-j" => {
                if inline_value.is_some() {
                    return Err(invalid_args(format!("{flag} does not take a value")));
                }
                parsed.json = true;
                index += 1;
            }
            "--receipt-dir" | "--skill" | "--status" | "--since" | "--until" | "--limit" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                match flag {
                    "--receipt-dir" => parsed.receipt_dir = Some(PathBuf::from(value)),
                    "--skill" => parsed.filter.skill = Some(value),
                    "--status" => parsed.filter.state = Some(value),
                    "--since" => parsed.filter.since = Some(value),
                    "--until" => parsed.filter.until = Some(value),
                    _ => {
                        parsed.filter.limit = Some(value.parse().map_err(|_| {
                            invalid_args(format!("invalid --limit value '{value}'"))
                        })?);
                    }
                }
                index = next_index;
            }
            _ => {
                return Err(HistoryCliError::InvalidArgs(format!(
                    "unknown history flag {flag}"
                )));
            }
        }
    }
    // A receipt id names one receipt to inspect; anything else searches.
    parsed.detail = matches!(positionals.as_slice(), [only] if is_receipt_id(only));
    parsed.query = (!positionals.is_empty()).then(|| positionals.join(" "));
    parsed.filter.query = parsed.query.clone();
    Ok(parsed)
}

/// A receipt's id: `sha256:` and the 64 hex digits of its content address.
fn is_receipt_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn render_history(listing: &RunListing, query: Option<&str>, receipt_dir: Option<&Path>) -> String {
    if listing.runs.is_empty() {
        if let Some(query) = query {
            return format!(
                "\n  No runs matched {query}.\n  Try runx history to see every local run.\n\n"
            );
        }
        return "\n  No runs yet. Try a live run first:\n  runx skill <skill-dir> --json\n\n"
            .to_owned();
    }
    let paused = listing
        .runs
        .iter()
        .filter(|run| run.state == RunState::Paused)
        .count();
    let mut lines = vec![
        String::new(),
        if paused == 0 {
            format!("  history  {} run(s)", listing.runs.len())
        } else {
            format!("  history  {} run(s), {paused} paused", listing.runs.len())
        },
        String::new(),
    ];
    for run in &listing.runs {
        let status = run
            .disposition
            .as_ref()
            .map_or(run.state.label(), |disposition| disposition.label());
        lines.push(format!("  {status}  {}  {}", run.skill, run.run_id));
        if let Some(receipt_id) = &run.receipt_id {
            lines.push(format!("     receipt  {receipt_id}"));
        }
        if let Some(request_id) = &run.pending_request_id {
            let resume =
                crate::resume::render_skill_resume_command(crate::resume::SkillResumeCommand {
                    run_id: &run.run_id,
                    receipt_dir,
                    answers_path: None,
                });
            lines.push(format!("     waits on  {request_id}"));
            lines.push(format!("     next  {resume}"));
        }
    }
    lines.push(String::new());
    lines.push(if paused == 0 {
        "  next  runx history <receipt-id> --json".to_owned()
    } else {
        "  next  pipe one answers object into one of the commands above".to_owned()
    });
    lines.push(String::new());
    lines.join("\n")
}

fn render_receipt_inspection(inspection: &ReceiptInspectionProjection) -> String {
    let receipt = &inspection.receipt;
    let mut lines = vec![
        String::new(),
        format!(
            "  receipt  {}  {}  {}",
            receipt.status, receipt.verification.status, receipt.id
        ),
        format!("  subject  {}", receipt.subject_ref),
        format!(
            "  actor  {}",
            receipt
                .authority
                .actor_ref
                .as_deref()
                .unwrap_or("none stated")
        ),
        format!(
            "  authority  {} scope(s), {} grant(s), {} approval(s)",
            receipt.authority.exercised_scopes.len(),
            receipt.authority.grant_refs.len(),
            receipt.authority.approval_refs.len()
        ),
        format!("  acts  {}", receipt.acts.len()),
    ];
    for act in &receipt.acts {
        lines.push(format!("  {}  {}  {}", act.form, act.id, act.purpose));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn invalid_args(message: impl Into<String>) -> HistoryCliError {
    HistoryCliError::InvalidArgs(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;

    use super::*;
    use runx_contracts::ReceiptIssuerType;
    use runx_runtime::receipts::step_receipt;
    use runx_runtime::{
        Ed25519ReceiptSigner, Ed25519ReceiptVerifier, InvocationOutput,
        RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV,
        RUNX_RECEIPT_VERIFY_KID_ENV, RuntimeError,
    };

    #[test]
    fn parses_history_args_without_comparing_against_runtime_constants() -> Result<(), io::Error> {
        let parsed = parse_history_args(&[
            "history".into(),
            "sourcey".into(),
            "--skill".into(),
            "source".into(),
            "--status=paused".into(),
            "--limit".into(),
            "5".into(),
            "--json".into(),
        ])
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert_eq!(parsed.query.as_deref(), Some("sourcey"));
        assert_eq!(parsed.filter.skill.as_deref(), Some("source"));
        assert_eq!(parsed.filter.state.as_deref(), Some("paused"));
        assert_eq!(parsed.filter.limit, Some(5));
        assert!(parsed.json);
        Ok(())
    }

    #[test]
    fn short_json_flag_is_a_flag_not_a_query() -> Result<(), io::Error> {
        let parsed = parse_history_args(&["history".into(), "-j".into()])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(parsed.json);
        assert_eq!(parsed.query, None);
        Ok(())
    }

    #[test]
    fn a_receipt_id_names_one_receipt_and_takes_no_list_filters() -> Result<(), io::Error> {
        let receipt_id = format!("sha256:{}", "a".repeat(64));
        let parsed =
            parse_history_args(&["history".into(), receipt_id.clone().into(), "--json".into()])
                .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(parsed.detail);
        assert_eq!(parsed.query.as_deref(), Some(receipt_id.as_str()));
        let searched = parse_history_args(&["history".into(), "sha256:receipt".into()])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(!searched.detail);

        let invalid = run_history_command(
            &[
                "history".into(),
                receipt_id.into(),
                "--status".into(),
                "closed".into(),
            ],
            &BTreeMap::new(),
            Path::new("."),
        );
        assert!(matches!(invalid, Err(HistoryCliError::InvalidArgs(_))));
        Ok(())
    }

    #[test]
    fn history_lists_no_runs_in_an_empty_workspace() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let result = run_history_command(&["history".into(), "--json".into()], &env, &temp)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let listing: serde_json::Value = serde_json::from_str(&result.output)
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(listing["runs"], serde_json::json!([]));
        let human = run_history_command(&["history".into()], &env, &temp)
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(human.output.contains("No runs yet"));
        Ok(())
    }

    #[test]
    fn history_detail_json_projects_one_verified_receipt_without_execution_bodies()
    -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(|error| io::Error::other(error.to_string()))?;
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let store = LocalReceiptStore::new(&receipt_dir);
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        store
            .write_receipt(
                &receipt,
                RuntimeReceiptSignaturePolicy::verifying(&verifier),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;

        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        env.insert(
            RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV.to_owned(),
            "hosted".to_owned(),
        );
        env.insert(
            RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(),
            FIXTURE_KID.to_owned(),
        );
        env.insert(
            RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
            base64_standard(signer.public_key()),
        );
        let result = run_history_command(
            &[
                "history".into(),
                receipt.id.to_string().into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
                "--json".into(),
            ],
            &env,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        let output: ReceiptInspectionProjection = serde_json::from_str(&result.output)
            .map_err(|error| io::Error::other(error.to_string()))?;

        assert_eq!(output.receipt.id, receipt.id.to_string());
        assert_eq!(output.receipt.verification.status, "verified");
        assert!(!result.output.contains("structured_output"));
        assert!(!result.output.contains("stderr"));
        Ok(())
    }

    fn tempfile_dir() -> Result<PathBuf, io::Error> {
        let path = std::env::temp_dir().join(format!(
            "runx-cli-history-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| io::Error::other(error.to_string()))?
                .as_nanos()
        ));
        fs::create_dir_all(&path)?;
        Ok(path)
    }

    const FIXTURE_KID: &str = "runx-cli-prod-history-fixture-key";
    const FIXTURE_SEED: [u8; 32] = [0x42; 32];

    fn fixture_signer() -> Result<Ed25519ReceiptSigner, runx_runtime::RuntimeReceiptSigningError> {
        Ed25519ReceiptSigner::from_seed(FIXTURE_KID, ReceiptIssuerType::Hosted, &FIXTURE_SEED)
    }

    fn production_signed_receipt(
        signer: &Ed25519ReceiptSigner,
    ) -> Result<runx_contracts::Receipt, RuntimeError> {
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        let output = InvocationOutput::runtime_success(
            runx_contracts::JsonValue::Object(BTreeMap::from([(
                "message".to_owned(),
                runx_contracts::JsonValue::String("cli history".to_owned()),
            )])),
            10,
        );
        let claim =
            output
                .value
                .as_object()
                .cloned()
                .ok_or_else(|| RuntimeError::ReceiptInvalid {
                    message: "history fixture output must be an object".to_owned(),
                })?;
        step_receipt(
            runx_runtime::receipts::RunScope {
                run_id: "run_cli-history",
                graph_name: "cli-history",
            },
            "production-verified",
            1,
            &output,
            &claim,
            "2026-05-25T00:00:00Z",
            RuntimeReceiptSignaturePolicy::signing(signer, &verifier),
        )
    }

    fn base64_standard(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
        for chunk in bytes.chunks(3) {
            let first = chunk[0];
            let second = chunk.get(1).copied().unwrap_or(0);
            let third = chunk.get(2).copied().unwrap_or(0);
            let combined = ((first as u32) << 16) | ((second as u32) << 8) | third as u32;
            encoded.push(TABLE[((combined >> 18) & 0x3f) as usize] as char);
            encoded.push(TABLE[((combined >> 12) & 0x3f) as usize] as char);
            if chunk.len() > 1 {
                encoded.push(TABLE[((combined >> 6) & 0x3f) as usize] as char);
            } else {
                encoded.push('=');
            }
            if chunk.len() > 2 {
                encoded.push(TABLE[(combined & 0x3f) as usize] as char);
            } else {
                encoded.push('=');
            }
        }
        encoded
    }
}
