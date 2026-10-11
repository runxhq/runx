use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli_args;
use runx_runtime::receipts::inspection::{
    ReceiptInspectionProjection, inspect_local_receipt, inspect_local_receipt_with_policy,
};
use runx_runtime::run_record::{RunFilter, RunListing, RunRecordError, RunState, list_runs};
use runx_runtime::{
    Ed25519ReceiptVerifier, LocalReceiptStore, ReceiptPathInputs, ReceiptStoreError,
    RuntimeReceiptConfig, RuntimeReceiptSignaturePolicy, receipt_verifier_from_env,
    resolve_receipt_path,
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
        HistoryCliError::InvalidArgs("history --detail requires one exact receipt id".to_owned())
    })?;
    let filter = &parsed.filter;
    if filter.skill.is_some()
        || filter.state.is_some()
        || filter.since.is_some()
        || filter.until.is_some()
        || filter.limit.is_some()
    {
        return Err(HistoryCliError::InvalidArgs(
            "history --detail does not accept list filters".to_owned(),
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
    let verifier: Option<Ed25519ReceiptVerifier> = receipt_verifier_from_env(env)
        .map(|resolved| resolved.map(|verifier| verifier.into_verifier()))
        .map_err(|error| HistoryCliError::InvalidReceiptVerifier(error.to_string()))?;
    let inspection = if let Some(verifier) = verifier.as_ref() {
        inspect_local_receipt_with_policy(
            &store,
            &paths,
            receipt_reference,
            RuntimeReceiptSignaturePolicy::production(verifier),
        )
    } else {
        inspect_local_receipt(&store, &paths, receipt_reference)
    }
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
        if !token.starts_with("--") {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }
        let (flag, inline_value) = cli_args::split_flag(token);
        match flag {
            "--json" | "--detail" => {
                if inline_value.is_some() {
                    return Err(invalid_args(format!("{flag} does not take a value")));
                }
                if flag == "--json" {
                    parsed.json = true;
                } else {
                    parsed.detail = true;
                }
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
    parsed.query = (!positionals.is_empty()).then(|| positionals.join(" "));
    parsed.filter.query = parsed.query.clone();
    if parsed.detail && positionals.len() != 1 {
        return Err(invalid_args(
            "history --detail requires one exact receipt id",
        ));
    }
    Ok(parsed)
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
        "  next  runx history <receipt-id> --detail --json".to_owned()
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
        format!("  actor  {}", receipt.authority.actor_ref),
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
    use runx_runtime::receipts::step_receipt_with_signature_policy;
    use runx_runtime::{
        Ed25519ReceiptSigner, InvocationOutput, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
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
    fn parses_exact_detail_request_and_rejects_list_filters() -> Result<(), io::Error> {
        let parsed = parse_history_args(&[
            "history".into(),
            "sha256:receipt".into(),
            "--detail".into(),
            "--json".into(),
        ])
        .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(parsed.detail);
        assert_eq!(parsed.query.as_deref(), Some("sha256:receipt"));

        let invalid = run_history_command(
            &[
                "history".into(),
                "sha256:receipt".into(),
                "--detail".into(),
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
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
        store
            .write_receipt_with_policy(
                &receipt,
                RuntimeReceiptSignaturePolicy::production(&verifier),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;

        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
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
                "--detail".into(),
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
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
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
        step_receipt_with_signature_policy(
            runx_runtime::receipts::RunScope {
                run_id: "run_cli-history",
                graph_name: "cli-history",
            },
            "production-verified",
            1,
            &output,
            &claim,
            "2026-05-25T00:00:00Z",
            RuntimeReceiptSignaturePolicy::production_signing(signer, &verifier),
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
