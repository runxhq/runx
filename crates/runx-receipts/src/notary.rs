//! A hosted notary's counter-seal over a receipt's public projection.
//!
//! The notary signs the canonical JSON of its payload with Ed25519. This
//! module checks the payload digest, binds the signed payload to the public
//! projection beside it, and checks the signature against keys the verifier
//! trusts from outside the document; keys the document embeds are only
//! reported against those. The signature primitive is the caller's, as a
//! receipt's is.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use runx_contracts::sha256_prefixed;
use serde::Serialize;
use serde_json::{Map, Value};

pub const NOTARY_VERIFY_VERDICT_SCHEMA: &str = "runx.notary_verify_verdict.v1";

/// Checks an Ed25519 signature over `message` with a raw 32-byte public key.
pub trait CounterSealVerifier {
    fn verifies(&self, public_key: &[u8], message: &[u8], signature: &[u8]) -> bool;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NotaryVerifyVerdict {
    pub schema: &'static str,
    pub valid: bool,
    pub counter_seal: NotaryCounterSealReport,
    pub findings: Vec<NotaryFinding>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NotaryCounterSealReport {
    pub schema: Option<String>,
    pub digest_status: &'static str,
    pub signature_status: &'static str,
    pub trusted_key_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NotaryFinding {
    pub code: String,
    pub path: String,
    pub message: String,
}

// Function rationale: hosted notary verification traverses one
// public projection and accumulates all findings for operator diagnostics.
/// Verify a notary verification document against `trusted_keys`, raw
/// Ed25519 public keys the caller trusts.
#[must_use]
pub fn verify_notary_document(
    document: &[u8],
    trusted_keys: &[Vec<u8>],
    verifier: &dyn CounterSealVerifier,
) -> NotaryVerifyVerdict {
    let mut findings = Vec::new();
    let root = match serde_json::from_slice::<Value>(document) {
        Ok(value) => value,
        Err(error) => {
            findings.push(finding(
                "notary_parse_error",
                "$",
                format!("notary verification document is not valid JSON: {error}"),
            ));
            return verdict(None, "missing", "missing", 0, findings);
        }
    };
    let Some(notary) = root
        .get("receipt")
        .and_then(|receipt| receipt.get("notary_verification"))
        .and_then(Value::as_object)
    else {
        findings.push(finding(
            "notary_verification_missing",
            "$",
            "notary verification document must contain receipt.notary_verification",
        ));
        return verdict(None, "missing", "missing", 0, findings);
    };
    let Some(counter_seal) = notary.get("counter_seal").and_then(Value::as_object) else {
        findings.push(finding(
            "counter_seal_missing",
            "notary_verification.counter_seal",
            "notary verification is missing counter_seal",
        ));
        return verdict(None, "missing", "missing", trusted_keys.len(), findings);
    };
    let schema = counter_seal
        .get("schema")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let Some(payload) = counter_seal.get("payload") else {
        findings.push(finding(
            "counter_seal_payload_missing",
            "notary_verification.counter_seal.payload",
            "counter seal is missing its signed payload",
        ));
        return verdict(schema, "missing", "missing", trusted_keys.len(), findings);
    };
    let canonical_payload = match runx_contracts::canonical_stable_json(payload) {
        Ok(value) => value,
        Err(error) => {
            findings.push(finding(
                "counter_seal_payload_invalid",
                "notary_verification.counter_seal.payload",
                format!("counter seal payload cannot be canonicalized: {error}"),
            ));
            return verdict(schema, "invalid", "missing", trusted_keys.len(), findings);
        }
    };
    let expected_digest = sha256_prefixed(canonical_payload.as_bytes());
    let digest_status = match counter_seal.get("digest").and_then(Value::as_str) {
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
        verifier,
        &mut findings,
    );
    verdict(
        schema,
        digest_status,
        signature_status,
        trusted_keys.len(),
        findings,
    )
}

// Function rationale: counter-seal validation intentionally binds
// digest, public key, signature, and trust-key diagnostics in one check.
fn verify_counter_seal_signature(
    notary: &Map<String, Value>,
    counter_seal: &Map<String, Value>,
    canonical_payload: &str,
    trusted_keys: &[Vec<u8>],
    verifier: &dyn CounterSealVerifier,
    findings: &mut Vec<NotaryFinding>,
) -> &'static str {
    let Some(signature) = counter_seal.get("signature").and_then(Value::as_object) else {
        findings.push(finding(
            "counter_seal_signature_missing",
            "notary_verification.counter_seal.signature",
            "counter seal is missing signature",
        ));
        return "missing";
    };
    if signature.get("alg").and_then(Value::as_str) != Some("Ed25519") {
        findings.push(finding(
            "counter_seal_signature_algorithm_unsupported",
            "notary_verification.counter_seal.signature.alg",
            "counter seal signature algorithm must be Ed25519",
        ));
        return "invalid";
    }
    let Some(signature_value) = signature.get("value").and_then(Value::as_str) else {
        findings.push(finding(
            "counter_seal_signature_value_missing",
            "notary_verification.counter_seal.signature.value",
            "counter seal signature value is missing",
        ));
        return "missing";
    };
    let signature_bytes = match decode_signature(signature_value) {
        Some(bytes) if bytes.len() == 64 => bytes,
        Some(_) | None => {
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
    if trusted_keys
        .iter()
        .any(|key| verifier.verifies(key, canonical_payload.as_bytes(), &signature_bytes))
    {
        return "valid";
    }
    findings.push(finding(
        "counter_seal_signature_mismatch",
        "notary_verification.counter_seal.signature",
        "counter seal signature did not verify against any external trusted notary key",
    ));
    "invalid"
}

fn embedded_notary_keys(
    notary: &Map<String, Value>,
    findings: &mut Vec<NotaryFinding>,
) -> Vec<Vec<u8>> {
    let Some(keys) = notary.get("signer_public_keys").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut decoded = Vec::new();
    for (index, key) in keys.iter().enumerate() {
        let Some(pem) = key.get("public_key_pem").and_then(Value::as_str) else {
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
    notary: &Map<String, Value>,
    trusted_keys: &[Vec<u8>],
    findings: &mut Vec<NotaryFinding>,
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
    root: &Value,
    payload: &Value,
    counter_seal: &Map<String, Value>,
    findings: &mut Vec<NotaryFinding>,
) {
    let Some(receipt) = root.get("receipt").and_then(Value::as_object) else {
        return;
    };
    for field in ["digest", "mode", "runx_version"] {
        if receipt.get(field).and_then(Value::as_str) != payload.get(field).and_then(Value::as_str)
        {
            findings.push(finding(
                "notary_projection_binding_mismatch",
                format!("receipt.{field} <-> notary_verification.counter_seal.payload.{field}"),
                "public projection field does not match the signed notary payload",
            ));
        }
    }
    if let Some(projected_payload_digest) =
        counter_seal.get("payload_digest").and_then(Value::as_str)
    {
        match serde_json::to_string(payload) {
            Ok(canonical_payload) => {
                if projected_payload_digest != sha256_prefixed(canonical_payload.as_bytes()) {
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

/// The raw 32-byte Ed25519 key of an SPKI public key in PEM.
///
/// # Errors
///
/// Returns why the PEM is not an Ed25519 SPKI public key.
pub fn ed25519_public_key_from_spki_pem(pem: &str) -> Result<Vec<u8>, String> {
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

/// A counter seal's signature is `base64:` and unpadded base64url, as the
/// notary writes it and as a receipt's is; no other spelling is read.
fn decode_signature(value: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(value.strip_prefix("base64:")?).ok()
}

fn verdict(
    schema: Option<String>,
    digest_status: &'static str,
    signature_status: &'static str,
    trusted_key_count: usize,
    findings: Vec<NotaryFinding>,
) -> NotaryVerifyVerdict {
    NotaryVerifyVerdict {
        schema: NOTARY_VERIFY_VERDICT_SCHEMA,
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
) -> NotaryFinding {
    NotaryFinding {
        code: code.into(),
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine;
    use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

    use super::decode_signature;

    #[test]
    fn a_counter_seal_signature_has_one_spelling() {
        let signature = [7_u8; 64];
        assert_eq!(
            decode_signature(&format!("base64:{}", URL_SAFE_NO_PAD.encode(signature))),
            Some(signature.to_vec())
        );
        for other in [
            URL_SAFE_NO_PAD.encode(signature),
            format!("base64:{}", STANDARD.encode(signature)),
        ] {
            assert_eq!(decode_signature(&other), None, "{other}");
        }
    }
}
