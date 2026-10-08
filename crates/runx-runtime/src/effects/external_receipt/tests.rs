use std::collections::BTreeMap;
use std::fs;

use runx_contracts::{JsonObject, JsonValue};

use super::execution::verify_external_receipt;
use crate::credentials::CredentialDelivery;
use crate::effects::{EffectAdmission, EffectToolRequest};

#[cfg(unix)]
#[test]
fn canonical_verifier_binds_exact_target_and_contract_without_shell_expansion()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir()?;
    let receipt_path = root.path().join("receipt.json");
    let target = "0123456789abcdef0123456789abcdef01234567";
    let contract_hex = "a".repeat(64);
    fs::write(
        &receipt_path,
        serde_json::to_vec(&serde_json::json!({
            "body": {
                "task_id": "issue-442",
                "verdict": "pass",
                "head_commit": target,
                "spec_fingerprint": contract_hex,
                "open_blockers": []
            },
            "signature": {"alg": "ed25519", "key_id": "fixture", "sig": "opaque"}
        }))?,
    )?;
    let verifier = root.path().join("scafld-fixture");
    fs::write(
        &verifier,
        "#!/bin/sh\n[ \"$1\" = verify ] || exit 20\n[ \"$3\" = --target ] || exit 21\n[ \"$4\" = 0123456789abcdef0123456789abcdef01234567 ] || exit 22\nprintf '{\"ok\":true,\"command\":\"verify\"}'\n",
    )?;
    fs::set_permissions(&verifier, fs::Permissions::from_mode(0o755))?;
    let env = BTreeMap::from([
        (
            crate::paths::RUNX_CWD_ENV.to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            "RUNX_SCAFLD_BIN".to_owned(),
            verifier.to_string_lossy().into_owned(),
        ),
    ]);
    let inputs = JsonObject::from([
        (
            "receipt_path".to_owned(),
            JsonValue::String("receipt.json".to_owned()),
        ),
        ("target".to_owned(), JsonValue::String(target.to_owned())),
        (
            "contract_digest".to_owned(),
            JsonValue::String(format!("sha256:{contract_hex}")),
        ),
        ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
    ]);
    let delivery = CredentialDelivery::none();
    let admission = EffectAdmission::new(
        super::EXTERNAL_RECEIPT_EFFECT_FAMILY,
        runx_contracts::AuthorityVerb::Read,
        runx_core::state_machine::AuthorityAdmissionWitness {
            verb: runx_contracts::AuthorityVerb::Read,
            parent_term_id: "fixture".to_owned(),
            child_term_id: "fixture:verify".to_owned(),
            idempotency_key: None,
            capability_ref: None,
        },
        (),
    );

    let result = verify_external_receipt(EffectToolRequest {
        tool_ref: super::EXTERNAL_RECEIPT_VERIFY_TOOL,
        observed_at: "2026-08-10T00:00:00Z",
        inputs: &inputs,
        env: &env,
        skill_directory: root.path(),
        credential_delivery: &delivery,
        admission: Some(&admission),
    })?;
    let verification = result
        .as_object()
        .and_then(|value| value.get("external_receipt_verification"))
        .and_then(JsonValue::as_object)
        .ok_or("verification packet")?;
    assert_eq!(
        verification.get("verified").and_then(JsonValue::as_bool),
        Some(true)
    );
    assert_eq!(
        verification.get("target").and_then(JsonValue::as_str),
        Some(target)
    );
    Ok(())
}

#[test]
fn forged_contract_binding_is_rejected_before_verifier_execution()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    fs::write(
        root.path().join("receipt.json"),
        serde_json::to_vec(&serde_json::json!({
            "body": {
                "task_id": "issue-442",
                "verdict": "pass",
                "head_commit": "abc123",
                "spec_fingerprint": "b".repeat(64),
                "open_blockers": []
            }
        }))?,
    )?;
    let env = BTreeMap::from([(
        crate::paths::RUNX_CWD_ENV.to_owned(),
        root.path().to_string_lossy().into_owned(),
    )]);
    let inputs = JsonObject::from([
        (
            "receipt_path".to_owned(),
            JsonValue::String("receipt.json".to_owned()),
        ),
        ("target".to_owned(), JsonValue::String("abc123".to_owned())),
        (
            "contract_digest".to_owned(),
            JsonValue::String(format!("sha256:{}", "a".repeat(64))),
        ),
        ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
    ]);
    let delivery = CredentialDelivery::none();
    let admission = EffectAdmission::new(
        super::EXTERNAL_RECEIPT_EFFECT_FAMILY,
        runx_contracts::AuthorityVerb::Read,
        runx_core::state_machine::AuthorityAdmissionWitness {
            verb: runx_contracts::AuthorityVerb::Read,
            parent_term_id: "fixture".to_owned(),
            child_term_id: "fixture:verify".to_owned(),
            idempotency_key: None,
            capability_ref: None,
        },
        (),
    );
    let error = match verify_external_receipt(EffectToolRequest {
        tool_ref: super::EXTERNAL_RECEIPT_VERIFY_TOOL,
        observed_at: "2026-08-10T00:00:00Z",
        inputs: &inputs,
        env: &env,
        skill_directory: root.path(),
        credential_delivery: &delivery,
        admission: Some(&admission),
    }) {
        Ok(_) => return Err("forged contract binding unexpectedly verified".into()),
        Err(error) => error,
    };
    assert!(error.to_string().contains("spec_fingerprint"));
    Ok(())
}
