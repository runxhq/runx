#![allow(clippy::expect_used)]

#[test]
fn credential_delivery_keeps_material_in_memory_and_redacts_output() {
    const SECRET: &str = "provider-secret-must-not-escape";
    let delivery = runx_runtime::CredentialDelivery::process_env(
        "slack",
        "bearer",
        "SLACK_TOKEN",
        "credential:slack:operator",
        SECRET,
    )
    .expect("local credential delivery");

    assert_eq!(delivery.secret_env().get("SLACK_TOKEN"), Some(SECRET));
    assert!(!format!("{delivery:?}").contains(SECRET));
    assert_eq!(
        delivery.redact_text(format!("authorization: Bearer {SECRET}")),
        "authorization: Bearer [redacted-credential]"
    );
    assert!(
        !serde_json::to_string(delivery.public_observation().expect("public observation"))
            .expect("observation JSON")
            .contains(SECRET)
    );
}
