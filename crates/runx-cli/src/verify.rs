// Module rationale: verify parses the receipt, store and notary surfaces and
// renders the verdicts runx-receipts and the runtime's receipt store reach.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use runx_receipts::{
    NotaryVerifyVerdict, ReceiptProofContext, ReceiptVerifyVerdict,
    ed25519_public_key_from_spki_pem, verify_notary_document, verify_receipt_document_verdict,
};
use runx_runtime::{
    Ed25519CounterSealVerifier, Ed25519ReceiptVerifier, LocalReceiptStore,
    RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV, RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, ReceiptPathInputs, ReceiptStoreError, ReceiptTrust,
    RuntimeReceiptConfig, RuntimeReceiptSignaturePolicy, RunxPaths, StoreTreeVerification,
    resolve_receipt_path,
};
use serde::Serialize;

const SINGLE_RECEIPT_MAX_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug)]
pub enum VerifyCliError {
    InvalidArgs(String),
    InvalidReceiptVerifier(String),
    Store(String),
    Serialize(serde_json::Error),
}

impl fmt::Display for VerifyCliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArgs(message) | Self::InvalidReceiptVerifier(message) => {
                formatter.write_str(message)
            }
            Self::Store(message) => formatter.write_str(message),
            Self::Serialize(error) => write!(formatter, "failed to serialize report: {error}"),
        }
    }
}

impl std::error::Error for VerifyCliError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyCliResult {
    pub output: String,
    pub failed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ParsedVerifyArgs {
    receipt_id: Option<String>,
    receipt_dir: Option<PathBuf>,
    receipt: Option<ReceiptInput>,
    notary: Option<ReceiptInput>,
    notary_keys: Vec<PathBuf>,
    trust: Option<ReceiptTrust>,
    json: bool,
}

impl ParsedVerifyArgs {
    /// The keys a receipt is checked against: the selected home's own key
    /// and every configured key, unless the caller trusts configured keys
    /// only.
    fn trust(&self) -> ReceiptTrust {
        self.trust.unwrap_or(ReceiptTrust::HomeAndConfigured)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReceiptInput {
    Path(PathBuf),
    Stdin,
}

#[derive(Clone, Debug, Serialize)]
struct VerifyReport {
    receipt_dir: String,
    trees: Vec<TreeReport>,
    unreadable_files: Vec<FileIssue>,
    valid: bool,
}

#[derive(Clone, Debug, Serialize)]
struct TreeReport {
    root_receipt_id: String,
    receipt_count: usize,
    valid: bool,
    findings: Vec<FindingReport>,
}

#[derive(Clone, Debug, Serialize)]
struct FindingReport {
    code: String,
    path: String,
    message: String,
}

#[derive(Clone, Debug, Serialize)]
struct FileIssue {
    file: String,
    message: String,
}

impl VerifyReport {
    fn of(receipt_dir: &Path, verification: StoreTreeVerification) -> Self {
        let valid = verification.valid();
        Self {
            receipt_dir: receipt_dir.display().to_string(),
            trees: verification
                .trees
                .into_iter()
                .map(|tree| TreeReport {
                    root_receipt_id: tree.root_receipt_id,
                    receipt_count: tree.receipt_count,
                    valid: tree.verification.valid,
                    findings: tree
                        .verification
                        .findings
                        .into_iter()
                        .map(|finding| FindingReport {
                            code: finding.code.as_str().to_owned(),
                            path: finding.path,
                            message: finding.message,
                        })
                        .collect(),
                })
                .collect(),
            unreadable_files: verification
                .unreadable
                .into_iter()
                .map(|file| FileIssue {
                    file: file.file.display().to_string(),
                    message: file.message,
                })
                .collect(),
            valid,
        }
    }
}

pub fn run_verify_command(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<VerifyCliResult, VerifyCliError> {
    run_verify_command_with_stdin(args, env, cwd, io::empty())
}

pub fn run_verify_command_with_stdin<R: Read>(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
    stdin: R,
) -> Result<VerifyCliResult, VerifyCliError> {
    let parsed = parse_verify_args(args)?;
    if let Some(input) = parsed.notary.as_ref() {
        return run_notary_verify(input, &parsed.notary_keys, parsed.json, cwd, stdin);
    }
    if let Some(input) = parsed.receipt.as_ref() {
        return run_single_receipt_verify(input, parsed.json, parsed.trust(), env, cwd, stdin);
    }
    let paths = RunxPaths::resolve(env, cwd);
    let receipt_config = RuntimeReceiptConfig::default();
    let resolved = resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: parsed.receipt_dir.as_deref(),
        runtime_config: Some(&receipt_config),
        paths: &paths,
    });
    let verifier = receipt_verifier(env, &paths, parsed.trust())?;
    let verification = LocalReceiptStore::new(&resolved.path)
        .verify_trees(
            parsed.receipt_id.as_deref(),
            RuntimeReceiptSignaturePolicy::verifying(&verifier),
        )
        .map_err(|error| match error {
            ReceiptStoreError::ReceiptNotInStore { .. } => {
                VerifyCliError::InvalidArgs(error.to_string())
            }
            error => VerifyCliError::Store(format!(
                "failed to read receipt dir {}: {error}",
                resolved.path.display()
            )),
        })?;
    let report = VerifyReport::of(&resolved.path, verification);
    let output = if parsed.json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&report).map_err(VerifyCliError::Serialize)?
        )
    } else {
        render_report(&report)
    };
    Ok(VerifyCliResult {
        output,
        failed: !report.valid,
    })
}

// Function rationale: verify accepts receipt-directory flags and the
// single-receipt machine surface in one mutually-exclusive parser.
fn parse_verify_args(args: &[OsString]) -> Result<ParsedVerifyArgs, VerifyCliError> {
    let mut parsed = ParsedVerifyArgs::default();
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let Some(text) = arg.to_str() else {
            return Err(invalid_args("arguments must be valid UTF-8"));
        };
        match text {
            "--json" | "-j" => parsed.json = true,
            "--trust" => {
                let value = iter
                    .next()
                    .ok_or_else(|| invalid_args("--trust requires home or configured"))?;
                parsed.trust = Some(parse_trust(value.to_str().unwrap_or_default())?);
            }
            other if other.starts_with("--trust=") => {
                parsed.trust = Some(parse_trust(other.trim_start_matches("--trust="))?);
            }
            "--receipt-dir" => {
                let value = iter
                    .next()
                    .ok_or_else(|| invalid_args("--receipt-dir requires a directory"))?;
                parsed.receipt_dir = Some(PathBuf::from(value));
            }
            "--receipt" => {
                let value = iter
                    .next()
                    .ok_or_else(|| invalid_args("--receipt requires a path or -"))?;
                parsed.receipt = Some(parse_receipt_input(value)?);
            }
            "--notary" => {
                let value = iter
                    .next()
                    .ok_or_else(|| invalid_args("--notary requires a path or -"))?;
                parsed.notary = Some(parse_receipt_input(value)?);
            }
            "--notary-key" => {
                let value = iter.next().ok_or_else(|| {
                    invalid_args("--notary-key requires a trusted public key PEM path")
                })?;
                parsed.notary_keys.push(PathBuf::from(value));
            }
            other if other.starts_with("--receipt=") => {
                let value = other.trim_start_matches("--receipt=");
                parsed.receipt = Some(parse_receipt_input_text(value)?);
            }
            other if other.starts_with("--notary=") => {
                let value = other.trim_start_matches("--notary=");
                parsed.notary = Some(parse_receipt_input_text(value)?);
            }
            other if other.starts_with("--notary-key=") => {
                let value = other.trim_start_matches("--notary-key=");
                if value.is_empty() {
                    return Err(invalid_args(
                        "--notary-key requires a trusted public key PEM path",
                    ));
                }
                parsed.notary_keys.push(PathBuf::from(value));
            }
            other if other.starts_with("--") => {
                return Err(invalid_args(format!("unknown verify flag {other}")));
            }
            other => {
                if parsed.receipt_id.is_some() {
                    return Err(invalid_args("verify accepts at most one receipt id"));
                }
                parsed.receipt_id = Some(other.to_owned());
            }
        }
    }
    if parsed.receipt.is_some() && (parsed.receipt_id.is_some() || parsed.receipt_dir.is_some()) {
        return Err(invalid_args(
            "--receipt cannot be combined with a receipt id or --receipt-dir",
        ));
    }
    if parsed.notary.is_some()
        && (parsed.receipt.is_some() || parsed.receipt_id.is_some() || parsed.receipt_dir.is_some())
    {
        return Err(invalid_args(
            "--notary cannot be combined with --receipt, a receipt id, or --receipt-dir",
        ));
    }
    if parsed.notary.is_none() && !parsed.notary_keys.is_empty() {
        return Err(invalid_args("--notary-key requires --notary"));
    }
    if parsed.notary.is_some() && parsed.notary_keys.is_empty() {
        return Err(invalid_args(
            "--notary requires at least one external trusted public key via --notary-key",
        ));
    }
    if parsed.notary.is_some() && parsed.trust.is_some() {
        return Err(invalid_args(
            "--trust selects receipt keys; a notary counter-seal is checked against --notary-key",
        ));
    }
    Ok(parsed)
}

fn parse_trust(value: &str) -> Result<ReceiptTrust, VerifyCliError> {
    match value {
        "home" => Ok(ReceiptTrust::HomeAndConfigured),
        "configured" => Ok(ReceiptTrust::Configured),
        _ => Err(invalid_args("--trust requires home or configured")),
    }
}

fn parse_receipt_input(value: &OsString) -> Result<ReceiptInput, VerifyCliError> {
    let Some(text) = value.to_str() else {
        return Err(invalid_args("--receipt path must be valid UTF-8"));
    };
    parse_receipt_input_text(text)
}

fn parse_receipt_input_text(value: &str) -> Result<ReceiptInput, VerifyCliError> {
    if value.is_empty() {
        return Err(invalid_args("--receipt requires a path or -"));
    }
    Ok(if value == "-" {
        ReceiptInput::Stdin
    } else {
        ReceiptInput::Path(PathBuf::from(value))
    })
}

fn run_single_receipt_verify<R: Read>(
    input: &ReceiptInput,
    json: bool,
    trust: ReceiptTrust,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    stdin: R,
) -> Result<VerifyCliResult, VerifyCliError> {
    let document = read_single_receipt_input(input, cwd, stdin)?;
    // A malformed document yields a parse-error verdict without requiring trust
    // keys: you do not need a verifier to report that bytes are not a receipt.
    // Keys are required only to verify the signature of a well-formed receipt.
    let verifier = if runx_receipts::decode_receipt(&document).is_ok() {
        receipt_verifier(env, &RunxPaths::resolve(env, cwd), trust)?
    } else {
        Ed25519ReceiptVerifier::default()
    };
    let context = ReceiptProofContext {
        signature_verifier: Some(&verifier),
    };
    let verdict = verify_receipt_document_verdict(&document, &context);
    let output = if json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&verdict).map_err(VerifyCliError::Serialize)?
        )
    } else {
        render_single_receipt_verdict(&verdict)
    };
    Ok(VerifyCliResult {
        output,
        failed: !verdict.valid,
    })
}

/// Resolve a possibly-relative user-supplied path against `cwd`. The single
/// resolver for receipt and notary-key file inputs, so both stay consistent.
fn resolve_under_cwd(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn read_single_receipt_input<R: Read>(
    input: &ReceiptInput,
    cwd: &Path,
    stdin: R,
) -> Result<Vec<u8>, VerifyCliError> {
    match input {
        ReceiptInput::Path(path) => {
            let path = resolve_under_cwd(path, cwd);
            if let Ok(metadata) = fs::metadata(&path)
                && metadata.len() > SINGLE_RECEIPT_MAX_BYTES as u64
            {
                return Err(single_receipt_too_large());
            }
            let document = fs::read(&path).map_err(|error| {
                VerifyCliError::Store(format!(
                    "failed to read receipt {}: {error}",
                    path.display()
                ))
            })?;
            if document.len() > SINGLE_RECEIPT_MAX_BYTES {
                return Err(single_receipt_too_large());
            }
            Ok(document)
        }
        ReceiptInput::Stdin => read_limited_stdin(stdin),
    }
}

fn read_limited_stdin<R: Read>(stdin: R) -> Result<Vec<u8>, VerifyCliError> {
    let mut limited = stdin.take((SINGLE_RECEIPT_MAX_BYTES + 1) as u64);
    let mut document = Vec::new();
    limited.read_to_end(&mut document).map_err(|error| {
        VerifyCliError::Store(format!("failed to read receipt from stdin: {error}"))
    })?;
    if document.len() > SINGLE_RECEIPT_MAX_BYTES {
        return Err(single_receipt_too_large());
    }
    Ok(document)
}

fn run_notary_verify<R: Read>(
    input: &ReceiptInput,
    trusted_key_paths: &[PathBuf],
    json: bool,
    cwd: &Path,
    stdin: R,
) -> Result<VerifyCliResult, VerifyCliError> {
    let document = read_single_receipt_input(input, cwd, stdin)?;
    let trusted_keys = trusted_notary_keys_from_paths(trusted_key_paths, cwd)?;
    let verdict = verify_notary_document(&document, &trusted_keys, &Ed25519CounterSealVerifier);
    let output = if json {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&verdict).map_err(VerifyCliError::Serialize)?
        )
    } else {
        render_notary_verdict(&verdict)
    };
    Ok(VerifyCliResult {
        output,
        failed: !verdict.valid,
    })
}

fn trusted_notary_keys_from_paths(
    paths: &[PathBuf],
    cwd: &Path,
) -> Result<Vec<Vec<u8>>, VerifyCliError> {
    let mut keys = Vec::with_capacity(paths.len());
    for path in paths {
        let path = resolve_under_cwd(path, cwd);
        let pem = fs::read_to_string(&path).map_err(|error| {
            VerifyCliError::Store(format!(
                "failed to read notary key {}: {error}",
                path.display()
            ))
        })?;
        keys.push(ed25519_public_key_from_spki_pem(&pem).map_err(|message| {
            VerifyCliError::InvalidReceiptVerifier(format!(
                "invalid notary key {}: {message}",
                path.display()
            ))
        })?);
    }
    Ok(keys)
}

fn render_notary_verdict(verdict: &NotaryVerifyVerdict) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "notary counter-seal: {}\n",
        if verdict.valid { "ok" } else { "INVALID" }
    ));
    output.push_str(&format!(
        "digest: {}\nsignature: {}\ntrusted keys: {}\n",
        verdict.counter_seal.digest_status,
        verdict.counter_seal.signature_status,
        verdict.counter_seal.trusted_key_count
    ));
    for finding in &verdict.findings {
        output.push_str(&format!(
            "finding {} at {}: {}\n",
            finding.code, finding.path, finding.message
        ));
    }
    output
}

/// The keys `trust` selects. None at all is an error, not a report of every
/// signature failing: there is nothing to check a receipt against.
fn receipt_verifier(
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
    trust: ReceiptTrust,
) -> Result<Ed25519ReceiptVerifier, VerifyCliError> {
    let verifier = runx_runtime::receipt_verifier(env, paths, trust)
        .map_err(|error| VerifyCliError::InvalidReceiptVerifier(error.to_string()))?;
    if verifier.keys().is_empty() {
        return Err(VerifyCliError::InvalidReceiptVerifier(match trust {
            ReceiptTrust::HomeAndConfigured => format!(
                "runx verify has no trusted receipt key: the runx home at {} has no local key, and no verifier is configured. Set {RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV}, {RUNX_RECEIPT_VERIFY_KID_ENV} and {RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV} to trust a receipt from another home or host.",
                paths.home().display()
            ),
            ReceiptTrust::Configured => format!(
                "runx verify --trust configured requires {RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV}, {RUNX_RECEIPT_VERIFY_KID_ENV} and {RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV}, or a configured RUNX_RECEIPT_SIGN_* identity."
            ),
        }));
    }
    Ok(verifier)
}

fn render_report(report: &VerifyReport) -> String {
    let mut output = String::new();
    output.push_str(&format!("receipt dir: {}\n", report.receipt_dir));
    if report.trees.is_empty() {
        output.push_str("no receipts found\n");
    }
    for tree in &report.trees {
        // A tree signed by a key this home does not trust is unverified,
        // not tampered with; the run still fails until its key is trusted.
        let status = if tree.valid {
            "ok"
        } else if tree
            .findings
            .iter()
            .all(|finding| finding.code == "signature_key_missing")
        {
            "UNTRUSTED (signed by a key this home does not trust; configure its public key to verify it)"
        } else {
            "INVALID"
        };
        output.push_str(&format!(
            "tree {} ({} receipt{}): {status}\n",
            tree.root_receipt_id,
            tree.receipt_count,
            if tree.receipt_count == 1 { "" } else { "s" },
        ));
        for finding in &tree.findings {
            output.push_str(&format!(
                "  {} at {}: {}\n",
                finding.code,
                if finding.path.is_empty() {
                    "<root>"
                } else {
                    &finding.path
                },
                finding.message
            ));
        }
    }
    for issue in &report.unreadable_files {
        output.push_str(&format!("unreadable {}: {}\n", issue.file, issue.message));
    }
    output.push_str(if report.valid {
        "verification: ok\n"
    } else {
        "verification: FAILED\n"
    });
    output
}

fn render_single_receipt_verdict(verdict: &ReceiptVerifyVerdict) -> String {
    let mut output = String::new();
    output.push_str("receipt verification\n");
    output.push_str(&format!(
        "receipt: {}\n",
        verdict.receipt_id.as_deref().unwrap_or("<unparsed>")
    ));
    output.push_str(&format!(
        "signature: {}{}\n",
        verdict.signature.status,
        match (&verdict.signature.issuer_type, &verdict.signature.kid) {
            (Some(issuer_type), Some(kid)) => format!(" ({} key {kid})", issuer_type.label()),
            _ => String::new(),
        }
    ));
    output.push_str(&format!("digest: {}\n", verdict.digest.status));
    output.push_str(&format!(
        "content address: {}\n",
        verdict.content_address.status
    ));
    output.push_str(&format!("lineage: {}\n", verdict.lineage.status));
    for finding in &verdict.findings {
        output.push_str(&format!(
            "  {} at {}: {}\n",
            finding.code,
            if finding.path.is_empty() {
                "<root>"
            } else {
                &finding.path
            },
            finding.message
        ));
    }
    output.push_str(if verdict.valid {
        "verification: ok\n"
    } else {
        "verification: FAILED\n"
    });
    output
}

fn single_receipt_too_large() -> VerifyCliError {
    invalid_args(format!(
        "--receipt input exceeds {SINGLE_RECEIPT_MAX_BYTES} bytes"
    ))
}

fn invalid_args(message: impl Into<String>) -> VerifyCliError {
    VerifyCliError::InvalidArgs(message.into())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;

    use super::*;
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use ring::signature::KeyPair;
    use runx_contracts::{Receipt, ReceiptIssuerType, sha256_prefixed};
    use runx_runtime::receipts::step_receipt;
    use runx_runtime::{
        Ed25519ReceiptSigner, InvocationOutput, LocalReceiptStore,
        RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
        RUNX_RECEIPT_SIGN_KID_ENV, RuntimeError,
    };
    use serde::Deserialize;
    use serde_json as test_json;

    type JsonValue = test_json::Value;

    const CORPUS_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/receipt-verify");
    const FIXTURE_KID: &str = "runx-cli-verify-fixture-key";
    const FIXTURE_SEED: [u8; 32] = [0x47; 32];

    #[derive(Debug, Deserialize)]
    struct CorpusVerifier {
        keys: Vec<CorpusKey>,
    }

    #[derive(Debug, Deserialize)]
    struct CorpusKey {
        issuer_type: String,
        kid: String,
        public_key_base64: String,
    }

    #[derive(Debug, Deserialize)]
    struct CorpusCase {
        name: String,
        receipt: String,
        expected: String,
        issuer_type: Option<String>,
    }

    #[test]
    fn verifies_signed_receipt_store_from_verifier_or_signer_environment() -> Result<(), io::Error>
    {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(io::Error::other)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        LocalReceiptStore::new(&receipt_dir)
            .write_receipt(
                &receipt,
                RuntimeReceiptSignaturePolicy::verifying(&verifier),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;

        for env in [verifier_env(&signer), signer_env()] {
            let result = run_verify_command(
                &[
                    "verify".into(),
                    "--receipt-dir".into(),
                    receipt_dir.clone().into_os_string(),
                    "--json".into(),
                ],
                &env,
                &temp,
            )
            .map_err(|error| io::Error::other(error.to_string()))?;

            assert!(
                !result.failed,
                "expected clean verification: {}",
                result.output
            );
            let report: JsonValue =
                serde_json::from_str(&result.output).map_err(io::Error::other)?;
            assert_eq!(report["valid"], JsonValue::Bool(true));
        }
        Ok(())
    }

    #[test]
    fn receipt_store_verification_ignores_non_receipt_state_json() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(io::Error::other)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        LocalReceiptStore::new(&receipt_dir)
            .write_receipt(
                &receipt,
                RuntimeReceiptSignaturePolicy::verifying(&verifier),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;
        fs::write(
            receipt_dir.join("effect-state.json"),
            r#"{"schema_version":1,"entries":{}}"#,
        )?;

        let result = run_verify_command(
            &[
                "verify".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
                "--json".into(),
            ],
            &verifier_env(&signer),
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            !result.failed,
            "non-receipt state must not poison verification: {}",
            result.output
        );
        let report: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(report["valid"], JsonValue::Bool(true));
        assert_eq!(report["unreadable_files"].as_array().map(Vec::len), Some(0));
        Ok(())
    }

    #[test]
    fn flags_tampered_receipt_body() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(io::Error::other)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        LocalReceiptStore::new(&receipt_dir)
            .write_receipt(
                &receipt,
                RuntimeReceiptSignaturePolicy::verifying(&verifier),
            )
            .map_err(|error| io::Error::other(error.to_string()))?;

        // Tamper with the sealed body after signing.
        let receipt_file = LocalReceiptStore::new(&receipt_dir)
            .receipt_path(&receipt.id)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let tampered = fs::read_to_string(&receipt_file)?
            .replace("production-verified", "production-tampered");
        fs::write(&receipt_file, tampered)?;

        let env = verifier_env(&signer);
        let result = run_verify_command(
            &[
                "verify".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
            ],
            &env,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            result.failed,
            "tampered receipt must fail: {}",
            result.output
        );
        assert!(result.output.contains("verification: FAILED"));
        Ok(())
    }

    #[test]
    fn missing_receipt_id_is_a_usage_error() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        fs::create_dir_all(&receipt_dir)?;
        let signer = fixture_signer().map_err(io::Error::other)?;
        let error = match run_verify_command(
            &[
                "verify".into(),
                "receipt_missing".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
            ],
            &verifier_env(&signer),
            &temp,
        ) {
            Ok(result) => {
                return Err(io::Error::other(format!(
                    "unknown receipt id must error: {}",
                    result.output
                )));
            }
            Err(error) => error,
        };
        assert!(
            matches!(&error, VerifyCliError::InvalidArgs(message) if message.contains("was not found")),
            "expected an unknown receipt id error, got {error}"
        );
        Ok(())
    }

    #[test]
    fn verifies_single_receipt_file_as_machine_verdict() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let signer = fixture_signer().map_err(io::Error::other)?;
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let receipt_file = temp.join("receipt.json");
        fs::write(
            &receipt_file,
            serde_json::to_vec_pretty(&receipt).map_err(io::Error::other)?,
        )?;

        let result = run_verify_command(
            &[
                "verify".into(),
                "--receipt".into(),
                receipt_file.into_os_string(),
                "--json".into(),
            ],
            &verifier_env(&signer),
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            !result.failed,
            "expected clean single receipt verdict: {}",
            result.output
        );
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["schema"], "runx.verify_verdict.v1");
        assert_eq!(verdict["valid"], JsonValue::Bool(true));
        assert_eq!(
            verdict["receipt_id"],
            JsonValue::String(receipt.id.to_string())
        );
        assert_eq!(verdict["signature"]["issuer_type"], "hosted");
        assert_eq!(verdict["signature"]["status"], "valid");
        assert_eq!(verdict["digest"]["status"], "valid");
        assert_eq!(verdict["content_address"]["status"], "valid");
        assert_eq!(verdict["lineage"]["status"], "unverified");
        assert!(verdict["findings"].as_array().is_some_and(Vec::is_empty));
        Ok(())
    }

    #[test]
    fn verifies_single_receipt_from_stdin() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let signer = fixture_signer().map_err(io::Error::other)?;
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let input = serde_json::to_vec(&receipt).map_err(io::Error::other)?;

        let result = run_verify_command_with_stdin(
            &[
                "verify".into(),
                "--receipt".into(),
                "-".into(),
                "--json".into(),
            ],
            &verifier_env(&signer),
            &temp,
            io::Cursor::new(input),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(!result.failed, "stdin verdict failed: {}", result.output);
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(
            verdict["receipt_id"],
            JsonValue::String(receipt.id.to_string())
        );
        assert_eq!(verdict["valid"], JsonValue::Bool(true));
        Ok(())
    }

    #[test]
    fn single_receipt_requires_trusted_keys_by_default() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let signer = fixture_signer().map_err(io::Error::other)?;
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let receipt_file = temp.join("receipt.json");
        fs::write(
            &receipt_file,
            serde_json::to_vec_pretty(&receipt).map_err(io::Error::other)?,
        )?;

        let error = match run_verify_command(
            &[
                "verify".into(),
                "--receipt".into(),
                receipt_file.into_os_string(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
        ) {
            Ok(result) => {
                return Err(io::Error::other(format!(
                    "receipt verification must fail closed without trusted keys: {}",
                    result.output
                )));
            }
            Err(error) => error,
        };
        assert!(
            matches!(error, VerifyCliError::InvalidReceiptVerifier(message) if message.contains("has no trusted receipt key"))
        );
        Ok(())
    }

    #[test]
    fn verifies_hosted_notary_counter_seal_from_stdin() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&FIXTURE_SEED)
            .map_err(|_| io::Error::other("fixture key must be valid"))?;
        let public_key_path = temp.join("trusted-notary.pem");
        fs::write(
            &public_key_path,
            ed25519_spki_pem(key_pair.public_key().as_ref()),
        )?;
        let payload = test_json::json!({
            "binary_version": "runx-test",
            "digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "issued_at": "2026-06-10T00:00:00Z",
            "mode": "full",
            "schema": "runx.hosted_notary_counter_seal_payload.v1",
            "verdict_digest": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        });
        let canonical_payload = runx_contracts::canonical_stable_json(&payload)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let signature = key_pair.sign(canonical_payload.as_bytes());
        let notary = test_json::json!({
            "notary_verification": {
                "counter_seal": {
                    "schema": "runx.hosted_notary_counter_seal.v1",
                    "payload": payload,
                    "digest": sha256_prefixed(canonical_payload.as_bytes()),
                    "signature": {
                        "alg": "Ed25519",
                        "value": format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
                    }
                },
                "signer_public_keys": [{
                    "kid": "trusted-hosted-receipt-notary-1",
                    "public_key_pem": ed25519_spki_pem(key_pair.public_key().as_ref())
                }]
            }
        });
        let signed = &notary["notary_verification"]["counter_seal"]["payload"];
        let document = test_json::json!({
            "receipt": {
                "digest": signed["digest"].clone(),
                "mode": signed["mode"].clone(),
                "binary_version": signed["binary_version"].clone(),
                "notary_verification": notary["notary_verification"].clone(),
            }
        });

        let result = run_verify_command_with_stdin(
            &[
                "verify".into(),
                "--notary".into(),
                "-".into(),
                "--notary-key".into(),
                public_key_path.into_os_string(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
            io::Cursor::new(serde_json::to_vec(&document).map_err(io::Error::other)?),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(!result.failed, "notary verifier failed: {}", result.output);
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["schema"], "runx.notary_verify_verdict.v1");
        assert_eq!(verdict["valid"], JsonValue::Bool(true));
        assert_eq!(verdict["counter_seal"]["digest_status"], "valid");
        assert_eq!(verdict["counter_seal"]["signature_status"], "valid");
        Ok(())
    }

    #[test]
    fn hosted_notary_flags_object_document_without_notary_verification() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&FIXTURE_SEED)
            .map_err(|_| io::Error::other("fixture key must be valid"))?;
        let public_key_path = temp.join("trusted-notary.pem");
        fs::write(
            &public_key_path,
            ed25519_spki_pem(key_pair.public_key().as_ref()),
        )?;
        // An object document that carries no notary_verification (or
        // receipt.notary_verification) wrapper must surface the missing finding,
        // not fall through to a misleading counter_seal_missing diagnostic.
        let document = test_json::json!({ "counter_seal": "not-a-notary-block" });

        let result = run_verify_command_with_stdin(
            &[
                "verify".into(),
                "--notary".into(),
                "-".into(),
                "--notary-key".into(),
                public_key_path.into_os_string(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
            io::Cursor::new(serde_json::to_vec(&document).map_err(io::Error::other)?),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            result.failed,
            "document without notary_verification must fail: {}",
            result.output
        );
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["valid"], JsonValue::Bool(false));
        let codes = finding_codes(&verdict);
        assert!(
            codes.contains(&"notary_verification_missing".to_owned()),
            "expected notary_verification_missing, got {codes:?}"
        );
        assert!(
            !codes.contains(&"counter_seal_missing".to_owned()),
            "must not fall through to counter_seal_missing: {codes:?}"
        );
        Ok(())
    }

    #[test]
    // Function rationale: this notary negative fixture builds the
    // untrusted projection and verifies each emitted finding together.
    fn hosted_notary_rejects_embedded_key_without_external_trust() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let signer = ring::signature::Ed25519KeyPair::from_seed_unchecked(&FIXTURE_SEED)
            .map_err(|_| io::Error::other("fixture key must be valid"))?;
        let untrusted = ring::signature::Ed25519KeyPair::from_seed_unchecked(&[7u8; 32])
            .map_err(|_| io::Error::other("fixture key must be valid"))?;
        let untrusted_key_path = temp.join("untrusted-notary.pem");
        fs::write(
            &untrusted_key_path,
            ed25519_spki_pem(untrusted.public_key().as_ref()),
        )?;
        let payload = test_json::json!({
            "binary_version": "runx-test",
            "digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "issued_at": "2026-06-10T00:00:00Z",
            "mode": "full",
            "schema": "runx.hosted_notary_counter_seal_payload.v1",
            "verdict_digest": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        });
        let canonical_payload = runx_contracts::canonical_stable_json(&payload)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let signature = signer.sign(canonical_payload.as_bytes());
        let notary = test_json::json!({
            "notary_verification": {
                "counter_seal": {
                    "schema": "runx.hosted_notary_counter_seal.v1",
                    "payload": payload,
                    "digest": sha256_prefixed(canonical_payload.as_bytes()),
                    "signature": {
                        "alg": "Ed25519",
                        "value": format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
                    }
                },
                "signer_public_keys": [{
                    "kid": "self-attested-hosted-receipt-notary",
                    "public_key_pem": ed25519_spki_pem(signer.public_key().as_ref())
                }]
            }
        });
        let signed = &notary["notary_verification"]["counter_seal"]["payload"];
        let document = test_json::json!({
            "receipt": {
                "digest": signed["digest"].clone(),
                "mode": signed["mode"].clone(),
                "binary_version": signed["binary_version"].clone(),
                "notary_verification": notary["notary_verification"].clone(),
            }
        });

        let result = run_verify_command_with_stdin(
            &[
                "verify".into(),
                "--notary".into(),
                "-".into(),
                "--notary-key".into(),
                untrusted_key_path.into_os_string(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
            io::Cursor::new(serde_json::to_vec(&document).map_err(io::Error::other)?),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            result.failed,
            "notary verifier should fail: {}",
            result.output
        );
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["valid"], JsonValue::Bool(false));
        assert_eq!(verdict["counter_seal"]["signature_status"], "invalid");
        assert!(finding_codes(&verdict).contains(&"counter_seal_signature_mismatch".to_owned()));
        assert!(finding_codes(&verdict).contains(&"notary_embedded_key_untrusted".to_owned()));
        Ok(())
    }

    #[test]
    // Function rationale: this notary fixture keeps the signed
    // payload, projection, and verification readback in one regression.
    fn hosted_notary_binds_signed_payload_to_public_projection() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let key_pair = ring::signature::Ed25519KeyPair::from_seed_unchecked(&FIXTURE_SEED)
            .map_err(|_| io::Error::other("fixture key must be valid"))?;
        let public_key_path = temp.join("trusted-notary.pem");
        fs::write(
            &public_key_path,
            ed25519_spki_pem(key_pair.public_key().as_ref()),
        )?;
        let payload = test_json::json!({
            "binary_version": "runx-test",
            "digest": "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            "issued_at": "2026-06-10T00:00:00Z",
            "mode": "full",
            "schema": "runx.hosted_notary_counter_seal_payload.v1",
            "verdict_digest": "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        });
        let canonical_payload = runx_contracts::canonical_stable_json(&payload)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let signature = key_pair.sign(canonical_payload.as_bytes());
        let document = test_json::json!({
            "receipt": {
                "digest": "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
                "mode": "full",
                "binary_version": "runx-test",
                "notary_verification": {
                    "counter_seal": {
                        "schema": "runx.hosted_notary_counter_seal.v1",
                        "payload": payload,
                        "payload_digest": sha256_prefixed(canonical_payload.as_bytes()),
                        "digest": sha256_prefixed(canonical_payload.as_bytes()),
                        "signature": {
                            "alg": "Ed25519",
                            "value": format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()))
                        }
                    },
                    "signer_public_keys": [{
                        "kid": "trusted-hosted-receipt-notary-1",
                        "public_key_pem": ed25519_spki_pem(key_pair.public_key().as_ref())
                    }]
                }
            }
        });

        let result = run_verify_command_with_stdin(
            &[
                "verify".into(),
                "--notary".into(),
                "-".into(),
                "--notary-key".into(),
                public_key_path.into_os_string(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
            io::Cursor::new(serde_json::to_vec(&document).map_err(io::Error::other)?),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            result.failed,
            "projection binding should fail: {}",
            result.output
        );
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["counter_seal"]["signature_status"], "valid");
        assert!(finding_codes(&verdict).contains(&"notary_projection_binding_mismatch".to_owned()));
        Ok(())
    }

    // Function rationale: malformed receipt regression covers capped stdin, invalid JSON, and machine verdict fields together.
    #[test]
    fn malformed_single_receipt_returns_invalid_verdict() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;

        let result = run_verify_command_with_stdin(
            &[
                "verify".into(),
                "--receipt".into(),
                "-".into(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
            io::Cursor::new(br#"{"schema":"runx.receipt.v1","#.to_vec()),
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(result.failed, "malformed receipt must fail");
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["schema"], "runx.verify_verdict.v1");
        assert_eq!(verdict["valid"], JsonValue::Bool(false));
        assert_eq!(verdict["receipt_id"], JsonValue::Null);
        assert_eq!(verdict["findings"][0]["code"], "receipt_parse_error");
        Ok(())
    }

    #[test]
    fn single_receipt_rejects_store_selection_flags() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let error = match run_verify_command(
            &[
                "verify".into(),
                "receipt_1".into(),
                "--receipt".into(),
                "receipt.json".into(),
            ],
            &BTreeMap::new(),
            &temp,
        ) {
            Ok(result) => {
                return Err(io::Error::other(format!(
                    "--receipt must be mutually exclusive with receipt ids: {}",
                    result.output
                )));
            }
            Err(error) => error,
        };
        assert!(matches!(error, VerifyCliError::InvalidArgs(_)));
        Ok(())
    }

    #[test]
    fn single_receipt_stdin_is_size_capped() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let error = match run_verify_command_with_stdin(
            &["verify".into(), "--receipt".into(), "-".into()],
            &BTreeMap::new(),
            &temp,
            io::Cursor::new(vec![b' '; SINGLE_RECEIPT_MAX_BYTES + 1]),
        ) {
            Ok(result) => {
                return Err(io::Error::other(format!(
                    "oversized stdin must be a usage error: {}",
                    result.output
                )));
            }
            Err(error) => error,
        };
        assert!(matches!(error, VerifyCliError::InvalidArgs(_)));
        Ok(())
    }

    #[test]
    fn receipt_verify_corpus_replays_through_cli_surface() -> Result<(), io::Error> {
        let root = PathBuf::from(CORPUS_ROOT);
        for (case_dir, case) in corpus_cases(&root)? {
            // Each case is verified under the corpus key its receipt names,
            // configured as an operator configures one.
            let env = match &case.issuer_type {
                Some(issuer_type) => corpus_key_env(&root, issuer_type)?,
                None => BTreeMap::new(),
            };
            let args = vec![
                "verify".into(),
                "--receipt".into(),
                case_dir.join(&case.receipt).into_os_string(),
                "--json".into(),
            ];
            let result = run_verify_command(&args, &env, &root)
                .map_err(|error| io::Error::other(error.to_string()))?;
            let actual: JsonValue =
                serde_json::from_str(&result.output).map_err(io::Error::other)?;
            let expected = expected_verdict(&case_dir, &case)?;

            assert_eq!(actual, expected, "corpus case {} drifted", case.name);
            assert_eq!(
                result.failed,
                !expected["valid"].as_bool().unwrap_or(false),
                "corpus case {} had inconsistent exit status",
                case.name
            );
        }
        Ok(())
    }

    /// A tree another home signed is unverified here, not tampered with: it
    /// fails the run as untrusted, and says how to trust it.
    #[cfg(unix)]
    #[test]
    fn a_workspace_tree_from_another_home_is_untrusted_not_invalid() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let source = home_env(&temp.join("source-home"));
        let receipt = home_signed_receipt(&source, &temp)?;
        let receipt_dir = temp.join("receipts");
        fs::create_dir_all(&receipt_dir)?;
        fs::write(
            receipt_dir.join(format!("{}.json", receipt.id.to_string().replace(':', "-"))),
            serde_json::to_vec(&receipt).map_err(io::Error::other)?,
        )?;
        let destination = home_env(&temp.join("destination-home"));
        home_signed_receipt(&destination, &temp)?;

        let result = run_verify_command(
            &[
                "verify".into(),
                "--receipt-dir".into(),
                receipt_dir.as_os_str().to_owned(),
            ],
            &destination,
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(result.failed, "{}", result.output);
        assert!(result.output.contains("UNTRUSTED"), "{}", result.output);
        assert!(!result.output.contains("INVALID"), "{}", result.output);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn verifies_selected_home_local_receipt() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let env = home_env(&temp.join("home"));
        let receipt = home_signed_receipt(&env, &temp)?;
        let receipt_file = temp.join("receipt.json");
        fs::write(
            &receipt_file,
            serde_json::to_vec(&receipt).map_err(io::Error::other)?,
        )?;

        let result = verify_receipt_file(&receipt_file, &env, &temp, &[])
            .map_err(|error| io::Error::other(error.to_string()))?;

        assert!(
            !result.failed,
            "home receipt must verify: {}",
            result.output
        );
        let verdict: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(verdict["signature"]["issuer_type"], "local");
        assert_eq!(verdict["signature"]["kid"], receipt.issuer.kid.as_str());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn moved_local_receipt_requires_explicit_public_key() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let source = home_env(&temp.join("source-home"));
        let receipt = home_signed_receipt(&source, &temp)?;
        let receipt_file = temp.join("receipt.json");
        fs::write(
            &receipt_file,
            serde_json::to_vec(&receipt).map_err(io::Error::other)?,
        )?;

        // Another home, with a key of its own, does not trust it.
        let mut destination = home_env(&temp.join("destination-home"));
        home_signed_receipt(&destination, &temp)?;
        let untrusted = verify_receipt_file(&receipt_file, &destination, &temp, &[])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(untrusted.failed);
        assert!(untrusted.output.contains("signature_key_missing"));

        // Its public key, configured explicitly, does.
        let public_key = runx_runtime::receipt_trust(
            &source,
            &runx_runtime::RunxPaths::resolve(&source, &temp),
            ReceiptTrust::HomeAndConfigured,
        )
        .map_err(io::Error::other)?
        .pop()
        .ok_or_else(|| io::Error::other("the source home has no key"))?
        .key;
        destination.extend(trust_env(
            "local",
            public_key.kid(),
            public_key.public_key(),
        ));
        let trusted = verify_receipt_file(&receipt_file, &destination, &temp, &[])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(
            !trusted.failed,
            "configured key must verify: {}",
            trusted.output
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn configured_only_trust_rejects_local_receipt() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let mut env = home_env(&temp.join("home"));
        let receipt = home_signed_receipt(&env, &temp)?;
        let receipt_file = temp.join("receipt.json");
        fs::write(
            &receipt_file,
            serde_json::to_vec(&receipt).map_err(io::Error::other)?,
        )?;

        let Err(VerifyCliError::InvalidReceiptVerifier(message)) =
            verify_receipt_file(&receipt_file, &env, &temp, &["--trust", "configured"])
        else {
            return Err(io::Error::other(
                "configured trust must not fall back to the home key",
            ));
        };
        assert!(message.contains("--trust configured requires"));

        let signer = fixture_signer().map_err(io::Error::other)?;
        env.extend(verifier_env(&signer));
        let result = verify_receipt_file(&receipt_file, &env, &temp, &["--trust", "configured"])
            .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(result.failed);
        assert!(result.output.contains("signature_key_missing"));
        Ok(())
    }

    fn home_env(home: &Path) -> BTreeMap<String, String> {
        BTreeMap::from([(
            runx_runtime::RUNX_HOME_ENV.to_owned(),
            home.display().to_string(),
        )])
    }

    /// A receipt sealed under the home `env` selects, its key created on
    /// first use.
    fn home_signed_receipt(
        env: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Receipt, io::Error> {
        let signature = runx_runtime::RuntimeReceiptSignatureConfig::resolve(
            env,
            &runx_runtime::RunxPaths::resolve(env, cwd),
        )
        .map_err(io::Error::other)?;
        step_receipt(
            runx_runtime::receipts::RunScope {
                run_id: "run_cli-home",
                graph_name: "cli-home",
            },
            "home-signed",
            1,
            &InvocationOutput::runtime_success(
                runx_contracts::JsonValue::Object(BTreeMap::new()),
                1,
            ),
            &BTreeMap::new(),
            "2026-10-11T00:00:00Z",
            signature.signature_policy(),
        )
        .map_err(|error| io::Error::other(error.to_string()))
    }

    fn verify_receipt_file(
        receipt_file: &Path,
        env: &BTreeMap<String, String>,
        cwd: &Path,
        extra: &[&str],
    ) -> Result<VerifyCliResult, VerifyCliError> {
        let mut args: Vec<OsString> = vec![
            "verify".into(),
            "--receipt".into(),
            receipt_file.as_os_str().to_owned(),
            "--json".into(),
        ];
        args.extend(extra.iter().map(OsString::from));
        run_verify_command(&args, env, cwd)
    }

    fn trust_env(issuer_type: &str, kid: &str, public_key: &[u8]) -> BTreeMap<String, String> {
        BTreeMap::from([
            (
                RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV.to_owned(),
                issuer_type.to_owned(),
            ),
            (RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(), kid.to_owned()),
            (
                RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
                base64_standard(public_key),
            ),
        ])
    }

    fn corpus_key_env(
        root: &Path,
        issuer_type: &str,
    ) -> Result<BTreeMap<String, String>, io::Error> {
        let verifier: CorpusVerifier =
            serde_json::from_str(&fs::read_to_string(root.join("verifier.json"))?)
                .map_err(io::Error::other)?;
        let key = verifier
            .keys
            .into_iter()
            .find(|key| key.issuer_type == issuer_type)
            .ok_or_else(|| io::Error::other("the corpus has no key of that issuer type"))?;
        Ok(BTreeMap::from([
            (
                RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV.to_owned(),
                key.issuer_type,
            ),
            (RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(), key.kid),
            (
                RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
                key.public_key_base64,
            ),
        ]))
    }

    fn corpus_cases(root: &Path) -> Result<Vec<(PathBuf, CorpusCase)>, io::Error> {
        let mut cases = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            let case_path = path.join("case.json");
            if !case_path.exists() {
                continue;
            }
            let case: CorpusCase =
                serde_json::from_str(&fs::read_to_string(case_path)?).map_err(io::Error::other)?;
            cases.push((path, case));
        }
        cases.sort_by(|left, right| left.1.name.cmp(&right.1.name));
        Ok(cases)
    }

    fn expected_verdict(case_dir: &Path, case: &CorpusCase) -> Result<JsonValue, io::Error> {
        serde_json::from_str(&fs::read_to_string(case_dir.join(&case.expected))?)
            .map_err(io::Error::other)
    }

    fn verifier_env(signer: &Ed25519ReceiptSigner) -> BTreeMap<String, String> {
        trust_env("hosted", FIXTURE_KID, signer.public_key())
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

    fn fixture_signer() -> Result<Ed25519ReceiptSigner, runx_runtime::RuntimeReceiptSigningError> {
        Ed25519ReceiptSigner::from_seed(FIXTURE_KID, ReceiptIssuerType::Hosted, &FIXTURE_SEED)
    }

    fn production_signed_receipt(signer: &Ed25519ReceiptSigner) -> Result<Receipt, RuntimeError> {
        let verifier = Ed25519ReceiptVerifier::new([signer.trusted_key()]);
        let output = InvocationOutput::runtime_success(
            runx_contracts::JsonValue::Object(BTreeMap::from([(
                "message".to_owned(),
                runx_contracts::JsonValue::String("cli verify".to_owned()),
            )])),
            10,
        );
        let claim =
            output
                .value
                .as_object()
                .cloned()
                .ok_or_else(|| RuntimeError::ReceiptInvalid {
                    message: "verify fixture output must be an object".to_owned(),
                })?;
        step_receipt(
            runx_runtime::receipts::RunScope {
                run_id: "run_cli-verify",
                graph_name: "cli-verify",
            },
            "production-verified",
            1,
            &output,
            &claim,
            "2026-06-10T00:00:00Z",
            RuntimeReceiptSignaturePolicy::signing(signer, &verifier),
        )
    }

    fn base64_standard(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut output = String::new();
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
            let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
            let triple = (b0 << 16) | (b1 << 8) | b2;
            output.push(TABLE[(triple >> 18) as usize & 0x3f] as char);
            output.push(TABLE[(triple >> 12) as usize & 0x3f] as char);
            output.push(if chunk.len() > 1 {
                TABLE[(triple >> 6) as usize & 0x3f] as char
            } else {
                '='
            });
            output.push(if chunk.len() > 2 {
                TABLE[triple as usize & 0x3f] as char
            } else {
                '='
            });
        }
        output
    }

    fn ed25519_spki_pem(raw_public_key: &[u8]) -> String {
        let mut der = Vec::from([
            0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
        ]);
        der.extend_from_slice(raw_public_key);
        format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----",
            base64_standard(&der)
        )
    }

    fn finding_codes(verdict: &JsonValue) -> Vec<String> {
        verdict["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|finding| finding["code"].as_str().map(ToOwned::to_owned))
            .collect()
    }

    fn tempfile_dir() -> Result<PathBuf, io::Error> {
        let path = std::env::temp_dir().join(format!(
            "runx-cli-verify-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|error| io::Error::other(error.to_string()))?
                .as_nanos()
        ));
        fs::create_dir_all(&path)?;
        Ok(path)
    }
}
