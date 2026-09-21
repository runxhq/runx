use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::cli_args;
use runx_runtime::journal::{
    HistoryFilter, JournalProjectionError, ReceiptInspectionProjection, inspect_local_receipt,
    inspect_local_receipt_with_policy, list_local_history, list_local_history_with_policy,
};
use runx_runtime::{
    Ed25519ReceiptVerifier, LocalReceiptStore, ReceiptPathInputs, ResolvedReceiptPath,
    RuntimeReceiptConfig, RuntimeReceiptSignaturePolicy, receipt_verifier_from_env,
    resolve_receipt_path,
};

// Module rationale: the native history CLI slice keeps
// parsing, rendering, and CLI parity tests together until the rest of the Rust
// command routing settles.
#[derive(Debug)]
pub enum HistoryCliError {
    InvalidArgs(String),
    InvalidReceiptVerifier(String),
    Projection(JournalProjectionError),
    Serialize(serde_json::Error),
}

impl fmt::Display for HistoryCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgs(message) => formatter.write_str(message),
            Self::InvalidReceiptVerifier(message) => formatter.write_str(message),
            Self::Projection(error) => write!(formatter, "{error}"),
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
    filter: HistoryFilter,
    detail: bool,
    json: bool,
}

pub fn run_history_command(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<HistoryCliResult, HistoryCliError> {
    let parsed = parse_history_args(args)?;
    let receipt_config = RuntimeReceiptConfig::default();
    let resolved = resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: parsed.receipt_dir.as_deref(),
        runtime_config: Some(&receipt_config),
        env,
        cwd,
    });
    let store = LocalReceiptStore::new(&resolved.path);
    let verifier = receipt_verifier_from_env(env)
        .map(|resolved| resolved.map(|verifier| verifier.into_verifier()))
        .map_err(|error| HistoryCliError::InvalidReceiptVerifier(error.to_string()))?;
    if parsed.detail {
        return run_receipt_detail(&parsed, &store, &resolved, verifier.as_ref());
    }
    run_history_list(&parsed, &store, &resolved, verifier.as_ref())
}

fn run_receipt_detail(
    parsed: &ParsedHistoryArgs,
    store: &LocalReceiptStore,
    resolved: &ResolvedReceiptPath,
    verifier: Option<&Ed25519ReceiptVerifier>,
) -> Result<HistoryCliResult, HistoryCliError> {
    let receipt_reference = parsed.query.as_deref().ok_or_else(|| {
        HistoryCliError::InvalidArgs("history --detail requires one exact receipt id".to_owned())
    })?;
    if has_non_query_filters(&parsed.filter) {
        return Err(HistoryCliError::InvalidArgs(
            "history --detail does not accept list filters".to_owned(),
        ));
    }
    let inspection = if let Some(verifier) = verifier {
        inspect_local_receipt_with_policy(
            store,
            &resolved.workspace_base,
            &resolved.project_runx_dir,
            receipt_reference,
            RuntimeReceiptSignaturePolicy::production(verifier),
        )
    } else {
        inspect_local_receipt(
            store,
            &resolved.workspace_base,
            &resolved.project_runx_dir,
            receipt_reference,
        )
    }
    .map_err(HistoryCliError::Projection)?;
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

fn run_history_list(
    parsed: &ParsedHistoryArgs,
    store: &LocalReceiptStore,
    resolved: &ResolvedReceiptPath,
    verifier: Option<&Ed25519ReceiptVerifier>,
) -> Result<HistoryCliResult, HistoryCliError> {
    let history = if let Some(verifier) = verifier {
        list_local_history_with_policy(
            store,
            &resolved.workspace_base,
            &resolved.project_runx_dir,
            &parsed.filter,
            RuntimeReceiptSignaturePolicy::production(verifier),
        )
    } else {
        list_local_history(
            store,
            &resolved.workspace_base,
            &resolved.project_runx_dir,
            &parsed.filter,
        )
    }
    .map_err(HistoryCliError::Projection)?;
    let output = if parsed.json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&history).map_err(HistoryCliError::Serialize)?
        )
    } else {
        render_history(
            &history,
            parsed.query.as_deref(),
            parsed.receipt_dir.as_deref(),
        )
    };
    Ok(successful_result(output))
}

fn successful_result(output: String) -> HistoryCliResult {
    HistoryCliResult {
        output,
        error_is_usage: false,
    }
}

// Function rationale: this mirrors the public history CLI
// flag grammar in one parser during the hard cutover.
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
        if token == "-j" {
            parsed.json = true;
            index += 1;
            continue;
        }
        if !token.starts_with("--") {
            positionals.push(token.to_owned());
            index += 1;
            continue;
        }
        let (flag, inline_value) = cli_args::split_flag(token);
        match flag {
            "--json" => {
                if inline_value.is_some() {
                    return Err(invalid_args("--json does not take a value"));
                }
                parsed.json = true;
                index += 1;
            }
            "--detail" => {
                if inline_value.is_some() {
                    return Err(invalid_args("--detail does not take a value"));
                }
                parsed.detail = true;
                index += 1;
            }
            "--include-harness" => {
                if inline_value.is_some() {
                    return Err(invalid_args("--include-harness does not take a value"));
                }
                parsed.filter.include_harness = true;
                index += 1;
            }
            "--include-internal" => {
                if inline_value.is_some() {
                    return Err(invalid_args("--include-internal does not take a value"));
                }
                parsed.filter.include_internal = true;
                index += 1;
            }
            "--receipt-dir" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.receipt_dir = Some(PathBuf::from(value));
                index = next_index;
            }
            "--skill" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.skill = Some(value);
                index = next_index;
            }
            "--status" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.status = Some(value);
                index = next_index;
            }
            "--source" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.source = Some(value);
                index = next_index;
            }
            "--actor" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.actor = Some(value);
                index = next_index;
            }
            "--artifact-type" | "--artifact_type" | "--artifactType" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.artifact_type = Some(value);
                index = next_index;
            }
            "--since" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.since = Some(value);
                index = next_index;
            }
            "--until" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.until = Some(value);
                index = next_index;
            }
            "--limit" => {
                let (value, next_index) =
                    cli_args::flag_value(args, index, flag, inline_value, "history")
                        .map_err(invalid_args)?;
                parsed.filter.limit = Some(value.parse().map_err(|_| {
                    HistoryCliError::InvalidArgs(format!("invalid --limit value '{value}'"))
                })?);
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

fn has_non_query_filters(filter: &HistoryFilter) -> bool {
    filter.skill.is_some()
        || filter.status.is_some()
        || filter.source.is_some()
        || filter.actor.is_some()
        || filter.artifact_type.is_some()
        || filter.since.is_some()
        || filter.until.is_some()
        || filter.limit.is_some()
        || filter.include_harness
        || filter.include_internal
}

fn render_history(
    history: &runx_runtime::journal::LocalHistoryProjection,
    query: Option<&str>,
    receipt_dir: Option<&Path>,
) -> String {
    let total = history.receipts.len() + history.pending_runs.len();
    if total == 0 {
        if let Some(query) = query {
            return format!(
                "\n  No receipts matched {query}.\n  Try runx history to see every local run.\n\n"
            );
        }
        return "\n  No receipts yet. Try a live run first:\n  runx skill <skill-dir> --json\n\n"
            .to_owned();
    }
    let mut lines = Vec::new();
    lines.push(String::new());
    lines.push(history_header(history));
    lines.push(String::new());
    for pending in &history.pending_runs {
        push_pending_run_lines(&mut lines, pending, receipt_dir);
    }
    for receipt in &history.receipts {
        push_receipt_line(&mut lines, receipt);
    }
    lines.push(String::new());
    lines.push(history_next_line(history));
    lines.push(String::new());
    lines.join("\n")
}

fn history_header(history: &runx_runtime::journal::LocalHistoryProjection) -> String {
    if history.pending_runs.is_empty() {
        format!("  history  {} receipt(s)", history.receipts.len())
    } else {
        format!(
            "  history  {} receipt(s), {} needs_agent",
            history.receipts.len(),
            history.pending_runs.len()
        )
    }
}

fn push_pending_run_lines(
    lines: &mut Vec<String>,
    pending: &runx_runtime::journal::PausedRunSummary,
    receipt_dir: Option<&Path>,
) {
    let step = pending
        .step_labels
        .first()
        .or_else(|| pending.step_ids.first())
        .map_or("", String::as_str);
    lines.push(format!(
        "  *  {}  needs_agent  {}  {}",
        pending.name,
        step,
        short_id(&pending.id)
    ));
    if pending.resume_skill_ref.is_some() {
        let resume_command =
            crate::resume::render_skill_resume_command(crate::resume::SkillResumeCommand {
                run_id: &pending.id,
                receipt_dir,
                answers_path: None,
            });
        lines.push(format!("     next  {resume_command}"));
    } else {
        lines.push(
            "     next  this legacy pending run lacks a resumable skill binding; rerun the original skill"
                .to_owned(),
        );
    }
}

fn push_receipt_line(
    lines: &mut Vec<String>,
    receipt: &runx_runtime::journal::LocalHistoryReceipt,
) {
    lines.push(format!(
        "  {}  {}  {}  {}",
        receipt.status,
        receipt.name,
        receipt.verification.status,
        short_id(&receipt.id)
    ));
}

fn history_next_line(history: &runx_runtime::journal::LocalHistoryProjection) -> String {
    if history.pending_runs.is_empty() {
        "  next  runx history <receipt-id> --detail --json".to_owned()
    } else if history
        .pending_runs
        .iter()
        .any(|run| run.resume_skill_ref.is_some())
    {
        "  next  pipe one answers object into one of the commands above".to_owned()
    } else {
        "  next  rerun the original skill; these legacy pending runs lack resume bindings"
            .to_owned()
    }
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
        lines.push(format!("  {}  {}  {}", act.disposition, act.form, act.id));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn short_id(value: &str) -> &str {
    value.get(..12).unwrap_or(value)
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
    use runx_runtime::journal::{PausedRunCheckpoint, append_paused_run_checkpoint};
    use runx_runtime::receipts::step_receipt_with_signature_policy;
    use runx_runtime::{
        Ed25519ReceiptSigner, InvocationOutput, RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV,
        RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV, RUNX_RECEIPT_SIGN_KID_ENV,
        RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_KID_ENV,
        RuntimeError,
    };

    #[test]
    fn parses_history_args_without_comparing_against_runtime_constants() -> Result<(), io::Error> {
        let parsed = parse_history_args(&[
            "history".into(),
            "sourcey".into(),
            "--skill".into(),
            "source".into(),
            "--status=needs_agent".into(),
            "--artifact-type".into(),
            "artifact".into(),
            "--json".into(),
        ])
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert_eq!(parsed.query.as_deref(), Some("sourcey"));
        assert_eq!(parsed.filter.skill.as_deref(), Some("source"));
        assert_eq!(parsed.filter.status.as_deref(), Some("needs_agent"));
        assert_eq!(parsed.filter.artifact_type.as_deref(), Some("artifact"));
        assert!(parsed.json);
        Ok(())
    }

    #[test]
    fn documented_short_json_flag_is_not_swallowed_as_a_history_query() -> Result<(), io::Error> {
        let parsed = parse_history_args(&["history".into(), "-j".into(), "sourcey".into()])
            .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(parsed.json);
        assert_eq!(parsed.query.as_deref(), Some("sourcey"));
        assert_eq!(parsed.filter.query.as_deref(), Some("sourcey"));
        assert!(crate::router::json_requested(&[
            "history".into(),
            "-j".into()
        ]));

        let detail = parse_history_args(&[
            "history".into(),
            "-j".into(),
            "sha256:receipt".into(),
            "--detail".into(),
        ])
        .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(detail.json);
        assert!(detail.detail);
        assert_eq!(detail.query.as_deref(), Some("sha256:receipt"));
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
    fn executes_history_json_with_pending_run() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        write_needs_agent_ledger(&receipt_dir)?;

        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let result = run_history_command(
            &[
                "history".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
                "--json".into(),
            ],
            &env,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        let output: HistoryOutput = serde_json::from_str(&result.output)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let first_pending_run = output.pending_runs.first().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "history output has no pending run",
            )
        })?;

        assert_eq!(output.pending_runs.len(), 1);
        assert_eq!(first_pending_run.id, "gx_needs_agent_oracle");
        assert_eq!(first_pending_run.status, "paused");
        assert_eq!(
            first_pending_run.selected_runner,
            Some("agent-task".to_owned())
        );
        assert_eq!(
            first_pending_run.resume_skill_ref,
            Some("../skills/sourcey".to_owned())
        );
        Ok(())
    }

    #[test]
    fn history_human_pending_run_includes_resume_command() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        write_needs_agent_ledger(&receipt_dir)?;

        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let result = run_history_command(
            &[
                "history".into(),
                "--receipt-dir".into(),
                receipt_dir.clone().into_os_string(),
            ],
            &env,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        let receipt_dir_arg = receipt_dir.to_string_lossy();
        assert!(
            result
                .output
                .contains("next  runx resume gx_needs_agent_oracle -")
        );
        assert!(
            result
                .output
                .contains(&format!("--receipt-dir {}", receipt_dir_arg))
        );
        assert!(
            result
                .output
                .contains("pipe one answers object into one of the commands above")
        );
        Ok(())
    }

    #[test]
    fn history_human_pending_run_omits_default_receipt_dir_from_resume_command()
    -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join(".runx").join("receipts");
        write_needs_agent_ledger(&receipt_dir)?;

        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let result = run_history_command(&["history".into()], &env, &temp)
            .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            result
                .output
                .contains("next  runx resume gx_needs_agent_oracle -")
        );
        assert!(
            !result.output.contains("--receipt-dir"),
            "default receipt dir must not be echoed into resume commands:\n{}",
            result.output
        );
        Ok(())
    }

    #[test]
    fn history_human_pending_run_does_not_invent_resume_command() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        write_needs_agent_ledger_with_resume(&receipt_dir, None)?;

        let mut env = BTreeMap::new();
        env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
        let result = run_history_command(
            &[
                "history".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
            ],
            &env,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(!result.output.contains("runx skill sourcey"));
        assert!(!result.output.contains("runx resume"));
        assert!(result.output.contains("lacks a resumable skill binding"));
        Ok(())
    }

    #[test]
    fn history_json_reports_production_verified_receipts_from_verifier_or_signer_environment()
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

        for mut env in [verifier_env(&signer), signer_env()] {
            env.insert("RUNX_CWD".to_owned(), temp.to_string_lossy().to_string());
            let result = run_history_command(
                &[
                    "history".into(),
                    receipt.id.to_string().into(),
                    "--receipt-dir".into(),
                    receipt_dir.clone().into_os_string(),
                    "--json".into(),
                ],
                &env,
                &temp,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
            let output: HistoryOutput = serde_json::from_str(&result.output)
                .map_err(|error| io::Error::other(error.to_string()))?;
            let first_receipt = output.receipts.first().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "history output has no receipt")
            })?;

            assert_eq!(first_receipt.id, receipt.id.to_string());
            assert_eq!(first_receipt.verification.status, "verified");
        }
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

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct HistoryOutput {
        receipts: Vec<HistoryReceipt>,
        pending_runs: Vec<HistoryPendingRun>,
    }

    #[derive(serde::Deserialize)]
    struct HistoryReceipt {
        id: String,
        verification: HistoryReceiptVerification,
    }

    #[derive(serde::Deserialize)]
    struct HistoryReceiptVerification {
        status: String,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct HistoryPendingRun {
        id: String,
        status: String,
        resume_skill_ref: Option<String>,
        selected_runner: Option<String>,
    }

    fn write_needs_agent_ledger(receipt_dir: &Path) -> Result<(), io::Error> {
        write_needs_agent_ledger_with_resume(receipt_dir, Some("../skills/sourcey"))
    }

    fn write_needs_agent_ledger_with_resume(
        receipt_dir: &Path,
        resume_skill_ref: Option<&str>,
    ) -> Result<(), io::Error> {
        append_paused_run_checkpoint(
            receipt_dir,
            &PausedRunCheckpoint {
                id: "gx_needs_agent_oracle".to_owned(),
                name: "sourcey".to_owned(),
                kind: "graph".to_owned(),
                started_at: Some("2026-04-28T01:00:00.000Z".to_owned()),
                resume_skill_ref: resume_skill_ref.map(str::to_owned),
                selected_runner: Some("agent-task".to_owned()),
                credential_profile: None,
                package_digest: Some("sha256:package".to_owned()),
                execution_closure_digest: Some("sha256:closure".to_owned()),
                step_ids: vec!["discover".to_owned()],
                step_labels: vec!["inspect repo".to_owned()],
            },
        )
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

    fn verifier_env(signer: &Ed25519ReceiptSigner) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(),
                FIXTURE_KID.to_owned(),
            ),
            (
                RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
                base64_standard(signer.public_key()),
            ),
        ])
    }

    fn signer_env() -> BTreeMap<String, String> {
        BTreeMap::from([
            (RUNX_RECEIPT_SIGN_KID_ENV.to_owned(), FIXTURE_KID.to_owned()),
            (
                RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV.to_owned(),
                "hosted".to_owned(),
            ),
            (
                RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV.to_owned(),
                base64_standard(&FIXTURE_SEED),
            ),
        ])
    }

    fn production_signed_receipt(
        signer: &Ed25519ReceiptSigner,
    ) -> Result<runx_contracts::Receipt, RuntimeError> {
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
        let output = InvocationOutput::runtime_success(
            runx_contracts::JsonValue::Object(BTreeMap::from([(
                "artifact".to_owned(),
                runx_contracts::JsonValue::Object(BTreeMap::from([
                    (
                        "artifact_id".to_owned(),
                        runx_contracts::JsonValue::String("artifact_cli_history".to_owned()),
                    ),
                    (
                        "artifact_type".to_owned(),
                        runx_contracts::JsonValue::String("artifact".to_owned()),
                    ),
                ])),
            )])),
            10,
            BTreeMap::new(),
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
            "cli-history",
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
