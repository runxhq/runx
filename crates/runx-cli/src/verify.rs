// Module rationale: verify owns receipt-directory tree checks plus the single-receipt machine verdict.
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::{ED25519, UnparsedPublicKey};
use runx_contracts::{Receipt, Reference, ReferenceType, sha256_prefixed};
use runx_receipts::{
    ReceiptProofContext, ReceiptVerifySignatureMode, ReceiptVerifyVerdict,
    SignatureVerificationFailure, SignatureVerifier, receipt_edge_references,
    verify_receipt_document_verdict,
};
use runx_runtime::{
    Ed25519ReceiptVerifier, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, ReceiptPathInputs, ReceiptTreeConfig, RuntimeReceiptConfig,
    RuntimeReceiptSignaturePolicy, receipt_verifier_from_env, resolve_receipt_path,
    verify_runtime_receipt_tree_with_policy,
};
use serde::Serialize;

const RECEIPT_REFERENCE_PREFIX: &str = "runx:receipt:";
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
    allow_local_development_signatures: bool,
    json: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ReceiptInput {
    Path(PathBuf),
    Stdin,
}

#[derive(Clone, Debug, Serialize)]
struct VerifyReport {
    receipt_dir: String,
    signature_mode: &'static str,
    trees: Vec<TreeReport>,
    unreadable_files: Vec<FileIssue>,
    valid: bool,
}

#[derive(Clone, Debug, Serialize)]
struct TreeReport {
    root_receipt_id: String,
    receipt_count: usize,
    parent_missing: Option<String>,
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

#[derive(Clone, Debug, Serialize)]
struct NotaryVerifyVerdict {
    schema: &'static str,
    valid: bool,
    counter_seal: NotaryCounterSealReport,
    findings: Vec<FindingReport>,
}

#[derive(Clone, Debug, Serialize)]
struct NotaryCounterSealReport {
    schema: Option<String>,
    digest_status: &'static str,
    signature_status: &'static str,
    trusted_key_count: usize,
}

pub fn run_verify_command(
    args: &[OsString],
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<VerifyCliResult, VerifyCliError> {
    run_verify_command_with_stdin(args, env, cwd, io::empty())
}

// Function rationale: argument dispatch keeps tree and single-receipt modes mutually exclusive in one parser.
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
        return run_single_receipt_verify(
            input,
            parsed.json,
            parsed.allow_local_development_signatures,
            env,
            cwd,
            stdin,
        );
    }
    let receipt_config = RuntimeReceiptConfig::default();
    let resolved = resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: parsed.receipt_dir.as_deref(),
        runtime_config: Some(&receipt_config),
        paths: &runx_runtime::RunxPaths::resolve(env, cwd),
    });
    let verifier = receipt_verifier(env, parsed.allow_local_development_signatures)?;
    let signature_mode = if verifier.is_some() {
        "production"
    } else {
        "local-development"
    };

    let (receipts, unreadable_files) = load_receipts(&resolved.path)?;
    let trees = group_trees(&receipts);

    let selected: Vec<&ReceiptTree> = match parsed.receipt_id.as_deref() {
        Some(receipt_id) => {
            let selected = trees
                .iter()
                .filter(|tree| tree.member_ids.contains(receipt_id))
                .collect::<Vec<_>>();
            if selected.is_empty() {
                return Err(VerifyCliError::InvalidArgs(format!(
                    "receipt {receipt_id} was not found in {}",
                    resolved.path.display()
                )));
            }
            selected
        }
        None => trees.iter().collect(),
    };

    let mut tree_reports = Vec::new();
    for tree in selected {
        let policy = match verifier.as_ref() {
            Some(verifier) => RuntimeReceiptSignaturePolicy::production(verifier),
            None => RuntimeReceiptSignaturePolicy::local_development(),
        };
        // Supplied receipts are the candidate children; the root itself is the
        // traversal anchor and must not be offered as its own child.
        let members: Vec<Receipt> = tree
            .member_ids
            .iter()
            .filter(|id| id.as_str() != tree.root.id.as_str())
            .filter_map(|id| receipts.iter().find(|receipt| receipt.id.as_str() == id))
            .cloned()
            .collect();
        let verification = verify_runtime_receipt_tree_with_policy(
            &tree.root,
            members,
            ReceiptTreeConfig::default(),
            policy,
        );
        let valid = verification.valid && tree.parent_missing.is_none();
        tree_reports.push(TreeReport {
            root_receipt_id: tree.root.id.to_string(),
            receipt_count: tree.member_ids.len(),
            parent_missing: tree.parent_missing.clone(),
            valid,
            findings: verification
                .findings
                .into_iter()
                .map(|finding| FindingReport {
                    code: format!("{:?}", finding.code),
                    path: finding.path,
                    message: finding.message,
                })
                .collect(),
        });
    }

    let valid = tree_reports.iter().all(|tree| tree.valid) && unreadable_files.is_empty();
    let report = VerifyReport {
        receipt_dir: resolved.path.display().to_string(),
        signature_mode,
        trees: tree_reports,
        unreadable_files,
        valid,
    };

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
            "--allow-local-development-signatures" => {
                parsed.allow_local_development_signatures = true;
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
    if parsed.notary.is_some() && parsed.allow_local_development_signatures {
        return Err(invalid_args(
            "--allow-local-development-signatures is only valid for receipt verification",
        ));
    }
    Ok(parsed)
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
    allow_local_development_signatures: bool,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    stdin: R,
) -> Result<VerifyCliResult, VerifyCliError> {
    let document = read_single_receipt_input(input, cwd, stdin)?;
    // A malformed document yields a parse-error verdict without requiring trust
    // keys: you do not need a verifier to report that bytes are not a receipt.
    // Keys are required only to verify the signature of a well-formed receipt.
    let verifier = if serde_json::from_slice::<Receipt>(&document).is_ok() {
        receipt_verifier(env, allow_local_development_signatures)?
    } else {
        None
    };
    let local_verifier = LocalDevelopmentReceiptVerifier;
    let (signature_mode, signature_verifier): (ReceiptVerifySignatureMode, &dyn SignatureVerifier) =
        match verifier.as_ref() {
            Some(verifier) => (ReceiptVerifySignatureMode::Production, verifier),
            None => (
                ReceiptVerifySignatureMode::LocalDevelopment,
                &local_verifier,
            ),
        };
    let context = ReceiptProofContext {
        signature_verifier: Some(signature_verifier),
        authority_verified: false,
        external_attestations_verified: false,
        verified_redaction_refs: BTreeSet::new(),
        verified_hash_commitments: BTreeSet::new(),
    };
    let verdict = verify_receipt_document_verdict(&document, &context, signature_mode);
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
    let verdict = verify_notary_document(&document, &trusted_keys);
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

// Function rationale: hosted notary verification traverses one
// public projection and accumulates all findings for operator diagnostics.
fn verify_notary_document(document: &[u8], trusted_keys: &[Vec<u8>]) -> NotaryVerifyVerdict {
    let mut findings = Vec::new();
    let root = match serde_json::from_slice::<serde_json::Value>(document) {
        Ok(value) => value,
        Err(error) => {
            findings.push(finding(
                "notary_parse_error",
                "$",
                format!("notary verification document is not valid JSON: {error}"),
            ));
            return notary_verdict(None, "missing", "missing", 0, findings);
        }
    };
    let Some(notary) = locate_notary_verification(&root) else {
        findings.push(finding(
            "notary_verification_missing",
            "$",
            "notary verification document must contain receipt.notary_verification",
        ));
        return notary_verdict(None, "missing", "missing", 0, findings);
    };
    let Some(counter_seal) = notary
        .get("counter_seal")
        .and_then(serde_json::Value::as_object)
    else {
        findings.push(finding(
            "counter_seal_missing",
            "notary_verification.counter_seal",
            "notary verification is missing counter_seal",
        ));
        return notary_verdict(None, "missing", "missing", trusted_keys.len(), findings);
    };
    let schema = counter_seal
        .get("schema")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned);
    let Some(payload) = counter_seal.get("payload") else {
        findings.push(finding(
            "counter_seal_payload_missing",
            "notary_verification.counter_seal.payload",
            "counter seal is missing its signed payload",
        ));
        return notary_verdict(schema, "missing", "missing", trusted_keys.len(), findings);
    };
    let canonical_payload = match serde_json::to_string(payload) {
        Ok(value) => value,
        Err(error) => {
            findings.push(finding(
                "counter_seal_payload_invalid",
                "notary_verification.counter_seal.payload",
                format!("counter seal payload cannot be canonicalized: {error}"),
            ));
            return notary_verdict(schema, "invalid", "missing", trusted_keys.len(), findings);
        }
    };
    let expected_digest = sha256_prefixed(canonical_payload.as_bytes());
    let digest_status = match counter_seal
        .get("digest")
        .and_then(serde_json::Value::as_str)
    {
        Some(actual) if actual == expected_digest => "valid",
        Some(_) => {
            findings.push(finding(
                "counter_seal_digest_mismatch",
                "notary_verification.counter_seal.digest",
                "counter seal digest does not match the canonical payload",
            ));
            "invalid"
        }
        None => {
            findings.push(finding(
                "counter_seal_digest_missing",
                "notary_verification.counter_seal.digest",
                "counter seal is missing digest",
            ));
            "missing"
        }
    };
    bind_counter_seal_to_projection(&root, payload, counter_seal, &mut findings);
    let signature_status = verify_counter_seal_signature(
        notary,
        counter_seal,
        &canonical_payload,
        trusted_keys,
        &mut findings,
    );
    let trusted_key_count = trusted_keys.len();
    notary_verdict(
        schema,
        digest_status,
        signature_status,
        trusted_key_count,
        findings,
    )
}

fn locate_notary_verification(
    root: &serde_json::Value,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    root.get("receipt")
        .and_then(|receipt| receipt.get("notary_verification"))
        .and_then(serde_json::Value::as_object)
}

// Function rationale: counter-seal validation intentionally binds
// digest, public key, signature, and trust-key diagnostics in one check.
fn verify_counter_seal_signature(
    notary: &serde_json::Map<String, serde_json::Value>,
    counter_seal: &serde_json::Map<String, serde_json::Value>,
    canonical_payload: &str,
    trusted_keys: &[Vec<u8>],
    findings: &mut Vec<FindingReport>,
) -> &'static str {
    let Some(signature) = counter_seal
        .get("signature")
        .and_then(serde_json::Value::as_object)
    else {
        findings.push(finding(
            "counter_seal_signature_missing",
            "notary_verification.counter_seal.signature",
            "counter seal is missing signature",
        ));
        return "missing";
    };
    if signature.get("alg").and_then(serde_json::Value::as_str) != Some("Ed25519") {
        findings.push(finding(
            "counter_seal_signature_algorithm_unsupported",
            "notary_verification.counter_seal.signature.alg",
            "counter seal signature algorithm must be Ed25519",
        ));
        return "invalid";
    }
    let Some(signature_value) = signature.get("value").and_then(serde_json::Value::as_str) else {
        findings.push(finding(
            "counter_seal_signature_value_missing",
            "notary_verification.counter_seal.signature.value",
            "counter seal signature value is missing",
        ));
        return "missing";
    };
    let signature_bytes = match decode_signature(signature_value) {
        Ok(bytes) if bytes.len() == 64 => bytes,
        Ok(_) | Err(_) => {
            findings.push(finding(
                "counter_seal_signature_malformed",
                "notary_verification.counter_seal.signature.value",
                "counter seal signature is not a valid Ed25519 signature",
            ));
            return "invalid";
        }
    };
    if trusted_keys.is_empty() {
        findings.push(finding(
            "notary_trusted_key_missing",
            "--notary-key",
            "notary verification requires at least one external trusted Ed25519 public key",
        ));
        return "missing";
    }
    record_embedded_key_mismatch(notary, trusted_keys, findings);
    if trusted_keys.iter().any(|key| {
        UnparsedPublicKey::new(&ED25519, key)
            .verify(canonical_payload.as_bytes(), &signature_bytes)
            .is_ok()
    }) {
        return "valid";
    }
    findings.push(finding(
        "counter_seal_signature_mismatch",
        "notary_verification.counter_seal.signature",
        "counter seal signature did not verify against any external trusted notary key",
    ));
    "invalid"
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

fn embedded_notary_keys(
    notary: &serde_json::Map<String, serde_json::Value>,
    findings: &mut Vec<FindingReport>,
) -> Vec<Vec<u8>> {
    let Some(keys) = notary
        .get("signer_public_keys")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let mut decoded = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let Some(pem) = key
            .get("public_key_pem")
            .and_then(serde_json::Value::as_str)
        else {
            findings.push(finding(
                "notary_trusted_key_missing_pem",
                format!("notary_verification.signer_public_keys[{index}].public_key_pem"),
                "trusted notary key is missing public_key_pem",
            ));
            continue;
        };
        match ed25519_public_key_from_spki_pem(pem) {
            Ok(raw) => decoded.push(raw),
            Err(message) => findings.push(finding(
                "notary_trusted_key_malformed",
                format!("notary_verification.signer_public_keys[{index}].public_key_pem"),
                message,
            )),
        }
    }
    decoded
}

fn record_embedded_key_mismatch(
    notary: &serde_json::Map<String, serde_json::Value>,
    trusted_keys: &[Vec<u8>],
    findings: &mut Vec<FindingReport>,
) {
    let embedded = embedded_notary_keys(notary, findings);
    if embedded.is_empty() {
        return;
    }
    let has_trusted_embedded_key = embedded
        .iter()
        .any(|candidate| trusted_keys.iter().any(|trusted| trusted == candidate));
    if !has_trusted_embedded_key {
        findings.push(finding(
            "notary_embedded_key_untrusted",
            "notary_verification.signer_public_keys",
            "embedded notary keys do not include any externally trusted key",
        ));
    }
}

fn bind_counter_seal_to_projection(
    root: &serde_json::Value,
    payload: &serde_json::Value,
    counter_seal: &serde_json::Map<String, serde_json::Value>,
    findings: &mut Vec<FindingReport>,
) {
    let Some(receipt) = root.get("receipt").and_then(serde_json::Value::as_object) else {
        return;
    };
    require_matching_text(
        receipt,
        "digest",
        payload,
        "digest",
        "receipt.digest",
        "notary_verification.counter_seal.payload.digest",
        findings,
    );
    require_matching_text(
        receipt,
        "mode",
        payload,
        "mode",
        "receipt.mode",
        "notary_verification.counter_seal.payload.mode",
        findings,
    );
    require_matching_text(
        receipt,
        "binary_version",
        payload,
        "binary_version",
        "receipt.binary_version",
        "notary_verification.counter_seal.payload.binary_version",
        findings,
    );
    if let Some(projected_payload_digest) = counter_seal
        .get("payload_digest")
        .and_then(serde_json::Value::as_str)
    {
        match serde_json::to_string(payload) {
            Ok(canonical_payload) => {
                let actual_payload_digest = sha256_prefixed(canonical_payload.as_bytes());
                if projected_payload_digest != actual_payload_digest {
                    findings.push(finding(
                        "counter_seal_payload_digest_mismatch",
                        "notary_verification.counter_seal.payload_digest",
                        "projected counter-seal payload digest does not match the signed payload",
                    ));
                }
            }
            Err(error) => findings.push(finding(
                "counter_seal_payload_digest_invalid",
                "notary_verification.counter_seal.payload_digest",
                format!("projected counter-seal payload cannot be canonicalized: {error}"),
            )),
        }
    }
}

fn require_matching_text(
    left: &serde_json::Map<String, serde_json::Value>,
    left_key: &str,
    right: &serde_json::Value,
    right_key: &str,
    left_path: &str,
    right_path: &str,
    findings: &mut Vec<FindingReport>,
) {
    let left_value = left.get(left_key).and_then(serde_json::Value::as_str);
    let right_value = right.get(right_key).and_then(serde_json::Value::as_str);
    if left_value != right_value {
        findings.push(finding(
            "notary_projection_binding_mismatch",
            format!("{left_path} <-> {right_path}"),
            "public projection field does not match the signed notary payload",
        ));
    }
}

fn ed25519_public_key_from_spki_pem(pem: &str) -> Result<Vec<u8>, String> {
    const ED25519_SPKI_PREFIX: &[u8] = &[
        0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00,
    ];
    let body = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("-----BEGIN ") && !line.starts_with("-----END "))
        .collect::<String>();
    let der = STANDARD
        .decode(body)
        .map_err(|_| "trusted notary key PEM is not valid base64".to_owned())?;
    if der.len() != ED25519_SPKI_PREFIX.len() + 32 || !der.starts_with(ED25519_SPKI_PREFIX) {
        return Err("trusted notary key PEM is not an Ed25519 SPKI public key".to_owned());
    }
    Ok(der[ED25519_SPKI_PREFIX.len()..].to_vec())
}

fn decode_signature(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    let encoded = value.strip_prefix("base64:").unwrap_or(value);
    URL_SAFE_NO_PAD
        .decode(encoded)
        .or_else(|_| STANDARD.decode(encoded))
}

fn notary_verdict(
    schema: Option<String>,
    digest_status: &'static str,
    signature_status: &'static str,
    trusted_key_count: usize,
    findings: Vec<FindingReport>,
) -> NotaryVerifyVerdict {
    NotaryVerifyVerdict {
        schema: "runx.notary_verify_verdict.v1",
        valid: findings.is_empty() && digest_status == "valid" && signature_status == "valid",
        counter_seal: NotaryCounterSealReport {
            schema,
            digest_status,
            signature_status,
            trusted_key_count,
        },
        findings,
    }
}

fn finding(
    code: impl Into<String>,
    path: impl Into<String>,
    message: impl Into<String>,
) -> FindingReport {
    FindingReport {
        code: code.into(),
        path: path.into(),
        message: message.into(),
    }
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

fn receipt_verifier(
    env: &BTreeMap<String, String>,
    allow_local_development_signatures: bool,
) -> Result<Option<Ed25519ReceiptVerifier>, VerifyCliError> {
    let verifier = receipt_verifier_from_env(env)
        .map(|resolved| resolved.map(|verifier| verifier.into_verifier()))
        .map_err(|error| VerifyCliError::InvalidReceiptVerifier(error.to_string()))?;
    if verifier.is_none() && !allow_local_development_signatures {
        return Err(VerifyCliError::InvalidReceiptVerifier(format!(
            "runx verify requires a trusted receipt verifier. Set both {RUNX_RECEIPT_VERIFY_KID_ENV} and {RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV}, configure a complete RUNX_RECEIPT_SIGN_* identity, or pass --allow-local-development-signatures for local fixture receipts only."
        )));
    }
    Ok(verifier)
}

fn load_receipts(root: &Path) -> Result<(Vec<Receipt>, Vec<FileIssue>), VerifyCliError> {
    let mut receipts = Vec::new();
    let mut issues = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            return Err(VerifyCliError::Store(format!(
                "failed to read receipt dir {}: {error}",
                root.display()
            )));
        }
    };
    for entry in entries {
        let entry = entry.map_err(|error| {
            VerifyCliError::Store(format!(
                "failed to read receipt dir {}: {error}",
                root.display()
            ))
        })?;
        let path = entry.path();
        if !is_content_addressed_receipt_path(&path) {
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(contents) => match serde_json::from_str::<Receipt>(&contents) {
                Ok(receipt) => receipts.push(receipt),
                Err(error) => issues.push(FileIssue {
                    file: path.display().to_string(),
                    message: format!("not a valid receipt: {error}"),
                }),
            },
            Err(error) => issues.push(FileIssue {
                file: path.display().to_string(),
                message: format!("unreadable: {error}"),
            }),
        }
    }
    receipts.sort_by(|left, right| left.id.cmp(&right.id));
    Ok((receipts, issues))
}

fn is_content_addressed_receipt_path(path: &Path) -> bool {
    path.extension().and_then(|value| value.to_str()) == Some("json")
        && path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(|stem| stem.strip_prefix("sha256-"))
            .is_some_and(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

#[derive(Clone, Debug)]
struct ReceiptTree {
    root: Receipt,
    member_ids: BTreeSet<String>,
    /// Set when the chain above this root points at a receipt id that is not
    /// present in the store; the tree is then verified from the highest
    /// available node but reported as incomplete.
    parent_missing: Option<String>,
}

fn group_trees(receipts: &[Receipt]) -> Vec<ReceiptTree> {
    let by_id: BTreeMap<&str, &Receipt> = receipts
        .iter()
        .map(|receipt| (receipt.id.as_str(), receipt))
        .collect();
    // Runtime receipts form an immutable parent -> child DAG. Signed
    // child-to-parent lineage links are valid input, but are never required to
    // discover a tree.
    // A child may be shared by more than one root, so each root owns its own
    // reachable member set instead of forcing every receipt into one group.
    let (children_by_parent, parents_by_child) = receipt_tree_edges(receipts);
    let root_ids = root_receipt_ids(receipts, &by_id, &parents_by_child);
    let mut trees = root_ids
        .iter()
        .filter_map(|root_id| receipt_tree_from_root(root_id, &by_id, &children_by_parent))
        .collect::<Vec<_>>();
    append_uncovered_cycle_trees(receipts, &by_id, &children_by_parent, &mut trees);
    trees.sort_by(|left, right| left.root.id.cmp(&right.root.id));
    trees
}

fn receipt_tree_edges(
    receipts: &[Receipt],
) -> (
    BTreeMap<String, BTreeSet<String>>,
    BTreeMap<String, BTreeSet<String>>,
) {
    let mut children_by_parent: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut parents_by_child: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for receipt in receipts {
        for child in receipt_edge_references(receipt) {
            if let Some(child_id) = referenced_receipt_id(child) {
                children_by_parent
                    .entry(receipt.id.to_string())
                    .or_default()
                    .insert(child_id.to_owned());
                parents_by_child
                    .entry(child_id.to_owned())
                    .or_default()
                    .insert(receipt.id.to_string());
            }
        }
        if let Some(lineage) = &receipt.lineage
            && let Some(parent_id) = lineage.parent.as_ref().and_then(referenced_receipt_id)
        {
            children_by_parent
                .entry(parent_id.to_owned())
                .or_default()
                .insert(receipt.id.to_string());
            parents_by_child
                .entry(receipt.id.to_string())
                .or_default()
                .insert(parent_id.to_owned());
        }
    }
    (children_by_parent, parents_by_child)
}

fn root_receipt_ids(
    receipts: &[Receipt],
    by_id: &BTreeMap<&str, &Receipt>,
    parents_by_child: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    receipts
        .iter()
        .filter(|receipt| {
            parents_by_child
                .get(receipt.id.as_str())
                .is_none_or(|parents| {
                    parents
                        .iter()
                        .all(|parent| !by_id.contains_key(parent.as_str()))
                })
        })
        .map(|receipt| receipt.id.to_string())
        .collect()
}

fn append_uncovered_cycle_trees(
    receipts: &[Receipt],
    by_id: &BTreeMap<&str, &Receipt>,
    children_by_parent: &BTreeMap<String, BTreeSet<String>>,
    trees: &mut Vec<ReceiptTree>,
) {
    // Malformed cycles have no natural root. Anchor each still-uncovered
    // component once so structural verification can report the cycle.
    let mut covered = trees
        .iter()
        .flat_map(|tree| tree.member_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    for receipt in receipts {
        if covered.contains(receipt.id.as_str()) {
            continue;
        }
        if let Some(tree) = receipt_tree_from_root(receipt.id.as_str(), by_id, children_by_parent) {
            covered.extend(tree.member_ids.iter().cloned());
            trees.push(tree);
        }
    }
}

fn receipt_tree_from_root(
    root_id: &str,
    by_id: &BTreeMap<&str, &Receipt>,
    children_by_parent: &BTreeMap<String, BTreeSet<String>>,
) -> Option<ReceiptTree> {
    let root = (*by_id.get(root_id)?).clone();
    let mut member_ids = BTreeSet::new();
    let mut pending = vec![root_id.to_owned()];
    while let Some(receipt_id) = pending.pop() {
        if !member_ids.insert(receipt_id.clone()) {
            continue;
        }
        if let Some(children) = children_by_parent.get(&receipt_id) {
            pending.extend(
                children
                    .iter()
                    .filter(|child| by_id.contains_key(child.as_str()))
                    .cloned(),
            );
        }
    }
    let parent_missing = root
        .lineage
        .as_ref()
        .and_then(|lineage| lineage.parent.as_ref())
        .and_then(referenced_receipt_id)
        .filter(|parent| !by_id.contains_key(*parent))
        .map(str::to_owned);
    Some(ReceiptTree {
        root,
        member_ids,
        parent_missing,
    })
}

fn referenced_receipt_id(reference: &Reference) -> Option<&str> {
    if reference.reference_type != ReferenceType::Receipt {
        return None;
    }
    reference
        .uri
        .strip_prefix(RECEIPT_REFERENCE_PREFIX)
        .filter(|id| !id.is_empty())
}

fn render_report(report: &VerifyReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "receipt dir: {}\nsignature mode: {}\n",
        report.receipt_dir, report.signature_mode
    ));
    if report.signature_mode == "local-development" {
        output.push_str(
            "note: local-development signatures were accepted only because --allow-local-development-signatures was set; set RUNX_RECEIPT_VERIFY_KID and RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64 to verify production signatures\n",
        );
    }
    if report.trees.is_empty() {
        output.push_str("no receipts found\n");
    }
    for tree in &report.trees {
        let status = if tree.valid { "ok" } else { "INVALID" };
        output.push_str(&format!(
            "tree {} ({} receipt{}): {status}\n",
            tree.root_receipt_id,
            tree.receipt_count,
            if tree.receipt_count == 1 { "" } else { "s" },
        ));
        if let Some(missing) = &tree.parent_missing {
            output.push_str(&format!("  missing parent receipt: {missing}\n"));
        }
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
        "signature: {} ({})\n",
        verdict.signature.status, verdict.signature.mode
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

struct LocalDevelopmentReceiptVerifier;

impl SignatureVerifier for LocalDevelopmentReceiptVerifier {
    fn verify(
        &self,
        _issuer: &runx_contracts::ReceiptIssuer,
        signature: &runx_contracts::ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure> {
        if !signature.value.starts_with("sig:sha256:") {
            return Err(SignatureVerificationFailure::MalformedSignature);
        }
        if signature.value == format!("sig:{body_digest}") {
            Ok(())
        } else {
            Err(SignatureVerificationFailure::SignatureMismatch)
        }
    }
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
    use ring::signature::KeyPair;
    use runx_contracts::ReceiptIssuerType;
    use runx_runtime::receipts::step_receipt_with_signature_policy;
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
        kid: String,
        public_key_base64: String,
    }

    #[derive(Debug, Deserialize)]
    struct CorpusCase {
        name: String,
        receipt: String,
        expected: String,
        signature_mode: String,
    }

    #[derive(Debug, Deserialize)]
    struct ReceiptCompositionFixture {
        outer: Receipt,
        inner_receipts: Vec<Receipt>,
    }

    #[test]
    fn verifies_production_signed_receipt_store_from_verifier_or_signer_environment()
    -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(io::Error::other)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        LocalReceiptStore::new(&receipt_dir)
            .write_receipt_with_policy(
                &receipt,
                RuntimeReceiptSignaturePolicy::production(&verifier),
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
            assert_eq!(report["signature_mode"], "production");
        }
        Ok(())
    }

    #[test]
    fn receipt_store_verification_ignores_non_receipt_state_json() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(io::Error::other)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        LocalReceiptStore::new(&receipt_dir)
            .write_receipt_with_policy(
                &receipt,
                RuntimeReceiptSignaturePolicy::production(&verifier),
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
    fn groups_runtime_tree_from_parent_child_refs_without_child_back_links() -> Result<(), io::Error>
    {
        let signer = fixture_signer().map_err(io::Error::other)?;
        let mut root = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let mut child = root.clone();
        child.id = "sha256:immutable-child".into();
        child.digest = "sha256:immutable-child-body".into();
        child.lineage.get_or_insert_default().parent = None;
        let mut child_ref =
            Reference::with_uri(ReferenceType::Receipt, format!("runx:receipt:{}", child.id));
        child_ref.locator = Some(child.digest.clone());
        root.lineage
            .get_or_insert_default()
            .children
            .push(child_ref);

        let trees = group_trees(&[root.clone(), child.clone()]);

        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].root.id, root.id);
        assert_eq!(
            trees[0].member_ids,
            BTreeSet::from([root.id.to_string(), child.id.to_string()])
        );
        assert!(trees[0].parent_missing.is_none());
        Ok(())
    }

    #[test]
    fn groups_and_verifies_composite_receipt_evidence() -> Result<(), io::Error> {
        let fixture: ReceiptCompositionFixture = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/contracts/receipt-composition/marketplace-composite.json"
        )))
        .map_err(io::Error::other)?;
        let mut receipts = vec![fixture.outer.clone()];
        receipts.extend(fixture.inner_receipts.clone());

        let trees = group_trees(&receipts);

        assert_eq!(trees.len(), 1);
        assert_eq!(trees[0].root.id, fixture.outer.id);
        assert_eq!(
            trees[0].member_ids,
            receipts
                .iter()
                .map(|receipt| receipt.id.to_string())
                .collect::<BTreeSet<_>>()
        );

        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        fs::create_dir_all(&receipt_dir)?;
        for receipt in &receipts {
            let digest = receipt
                .id
                .strip_prefix("sha256:")
                .ok_or_else(|| io::Error::other("fixture receipt id is not content-addressed"))?;
            fs::write(
                receipt_dir.join(format!("sha256-{digest}.json")),
                serde_json::to_vec(receipt).map_err(io::Error::other)?,
            )?;
        }
        let result = run_verify_command(
            &[
                "verify".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
                "--allow-local-development-signatures".into(),
                "--json".into(),
            ],
            &BTreeMap::new(),
            &temp,
        )
        .map_err(|error| io::Error::other(error.to_string()))?;
        assert!(!result.failed, "{}", result.output);
        let report: JsonValue = serde_json::from_str(&result.output).map_err(io::Error::other)?;
        assert_eq!(report["valid"], JsonValue::Bool(true));
        assert!(
            report["trees"].as_array().is_some_and(
                |trees| trees.len() == 1 && trees[0]["findings"] == test_json::json!([])
            )
        );
        Ok(())
    }

    #[test]
    fn flags_tampered_receipt_body() -> Result<(), io::Error> {
        let temp = tempfile_dir()?;
        let receipt_dir = temp.join("receipts");
        let signer = fixture_signer().map_err(io::Error::other)?;
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
        let receipt = production_signed_receipt(&signer)
            .map_err(|error| io::Error::other(error.to_string()))?;
        LocalReceiptStore::new(&receipt_dir)
            .write_receipt_with_policy(
                &receipt,
                RuntimeReceiptSignaturePolicy::production(&verifier),
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
        let error = match run_verify_command(
            &[
                "verify".into(),
                "--allow-local-development-signatures".into(),
                "receipt_missing".into(),
                "--receipt-dir".into(),
                receipt_dir.into_os_string(),
            ],
            &BTreeMap::new(),
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
        assert!(matches!(error, VerifyCliError::InvalidArgs(_)));
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
        assert_eq!(verdict["signature"]["mode"], "production");
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
            matches!(error, VerifyCliError::InvalidReceiptVerifier(message) if message.contains("requires a trusted receipt verifier"))
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
        let canonical_payload = serde_json::to_string(&payload).map_err(io::Error::other)?;
        let signature = key_pair.sign(canonical_payload.as_bytes());
        let notary = test_json::json!({
            "notary_verification": {
                "counter_seal": {
                    "schema": "runx.hosted_notary_counter_seal.v1",
                    "payload": payload,
                    "digest": sha256_prefixed(canonical_payload.as_bytes()),
                    "signature": {
                        "alg": "Ed25519",
                        "value": format!("base64:{}", base64_standard(signature.as_ref()))
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
        let canonical_payload = serde_json::to_string(&payload).map_err(io::Error::other)?;
        let signature = signer.sign(canonical_payload.as_bytes());
        let notary = test_json::json!({
            "notary_verification": {
                "counter_seal": {
                    "schema": "runx.hosted_notary_counter_seal.v1",
                    "payload": payload,
                    "digest": sha256_prefixed(canonical_payload.as_bytes()),
                    "signature": {
                        "alg": "Ed25519",
                        "value": format!("base64:{}", base64_standard(signature.as_ref()))
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
        let canonical_payload = serde_json::to_string(&payload).map_err(io::Error::other)?;
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
                            "value": format!("base64:{}", base64_standard(signature.as_ref()))
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
        let production_env = corpus_production_env(&root)?;
        for (case_dir, case) in corpus_cases(&root)? {
            let env = if case.signature_mode == "production" {
                production_env.clone()
            } else {
                BTreeMap::new()
            };
            let mut args = vec!["verify".into()];
            if case.signature_mode == "local-development" {
                args.push("--allow-local-development-signatures".into());
            }
            args.extend([
                "--receipt".into(),
                case_dir.join(&case.receipt).into_os_string(),
                "--json".into(),
            ]);
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

    fn corpus_production_env(root: &Path) -> Result<BTreeMap<String, String>, io::Error> {
        let verifier: CorpusVerifier =
            serde_json::from_str(&fs::read_to_string(root.join("verifier.json"))?)
                .map_err(io::Error::other)?;
        Ok(BTreeMap::from([
            (RUNX_RECEIPT_VERIFY_KID_ENV.to_owned(), verifier.kid),
            (
                RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV.to_owned(),
                verifier.public_key_base64,
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

    fn fixture_signer() -> Result<Ed25519ReceiptSigner, runx_runtime::RuntimeReceiptSigningError> {
        Ed25519ReceiptSigner::from_seed(FIXTURE_KID, ReceiptIssuerType::Hosted, &FIXTURE_SEED)
    }

    fn production_signed_receipt(signer: &Ed25519ReceiptSigner) -> Result<Receipt, RuntimeError> {
        let verifier = Ed25519ReceiptVerifier::new([signer.production_key()]);
        let output = InvocationOutput::runtime_success(
            runx_contracts::JsonValue::Object(BTreeMap::from([(
                "artifact".to_owned(),
                runx_contracts::JsonValue::Object(BTreeMap::from([
                    (
                        "artifact_id".to_owned(),
                        runx_contracts::JsonValue::String("artifact_cli_verify".to_owned()),
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
                    message: "verify fixture output must be an object".to_owned(),
                })?;
        step_receipt_with_signature_policy(
            runx_runtime::receipts::RunScope {
                run_id: "run_cli-verify",
                graph_name: "cli-verify",
            },
            "production-verified",
            1,
            &output,
            &claim,
            "2026-06-10T00:00:00Z",
            RuntimeReceiptSignaturePolicy::production_signing(signer, &verifier),
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
