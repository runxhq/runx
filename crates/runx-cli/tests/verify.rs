#[test]
fn verify_json_failure_envelope_keeps_stderr_clean() -> Result<(), Box<dyn std::error::Error>> {
    let output = crate::support::isolated_runx_command("verify-json-error")?
        .args(["verify", "--receipt", "missing.json", "--json"])
        .output()?;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "failure");
    assert_eq!(value["error"]["code"], "runtime_error");
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("failed to read receipt"))
    );

    Ok(())
}

#[test]
fn completed_invalid_verification_has_distinct_exit_code() -> Result<(), Box<dyn std::error::Error>>
{
    let receipt = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/receipt-verify/tampered-signature/receipt.json");
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/receipt-verify/verifier.json");
    let corpus: serde_json::Value = serde_json::from_slice(&std::fs::read(corpus)?)?;
    let hosted = corpus["keys"]
        .as_array()
        .and_then(|keys| keys.iter().find(|key| key["issuer_type"] == "hosted"))
        .ok_or("the corpus has no hosted key")?;
    let output = crate::support::isolated_runx_command("verify-invalid-verdict")?
        .env("RUNX_RECEIPT_VERIFY_ISSUER_TYPE", "hosted")
        .env(
            "RUNX_RECEIPT_VERIFY_KID",
            hosted["kid"].as_str().unwrap_or_default(),
        )
        .env(
            "RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64",
            hosted["public_key_base64"].as_str().unwrap_or_default(),
        )
        .arg("verify")
        .arg("--receipt")
        .arg(receipt)
        .arg("--json")
        .output()?;

    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8(output.stderr)?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["valid"], false);
    Ok(())
}
