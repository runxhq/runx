use runx_contracts::{AttestationDecision, JsonObject, JsonValue, ProviderNotCalled};

use super::{AttestationInput, build};

fn digest(character: char) -> String {
    format!("sha256:{}", character.to_string().repeat(64))
}

fn evidence_item(locator: &str) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        ("locator".to_owned(), JsonValue::String(locator.to_owned())),
        ("digest".to_owned(), JsonValue::String(digest('a'))),
        (
            "proves".to_owned(),
            JsonValue::String("provider recorded refund".to_owned()),
        ),
    ]))
}

fn ready_inputs() -> AttestationInput {
    AttestationInput {
        action: "issued a refund".to_owned(),
        principal: "ops:jordan".to_owned(),
        claim: "refund completed".to_owned(),
        evidence: vec![evidence_item("provider:refund:1")],
        scope: None,
    }
}

#[test]
fn prepares_a_digest_bound_attestation_without_claiming_external_verification() {
    let attestation = build(&ready_inputs()).expect("attestation");
    assert_eq!(attestation.decision, AttestationDecision::ReadyToSeal);
    assert!(attestation.gaps.is_empty());
    assert_eq!(
        attestation.proof_boundary.provider_status,
        ProviderNotCalled::NotCalled
    );
    let packet = JsonValue::from_serialize(&attestation).expect("serialize");
    assert_eq!(
        packet
            .as_object()
            .and_then(|packet| packet.get("proof_boundary"))
            .and_then(JsonValue::as_object)
            .and_then(|proof| proof.get("external_action_verified")),
        Some(&JsonValue::Bool(false))
    );
}

#[test]
fn the_digest_binds_the_claim() {
    let first = build(&ready_inputs()).expect("attestation");
    let mut changed = ready_inputs();
    changed.claim = "refund partially completed".to_owned();
    let second = build(&changed).expect("attestation");
    assert_ne!(first.attestation_digest, second.attestation_digest);
    assert_eq!(
        first.attestation_digest,
        build(&ready_inputs())
            .expect("attestation")
            .attestation_digest
    );
}

#[test]
fn missing_subject_needs_an_agent() {
    let attestation = build(&AttestationInput {
        action: String::new(),
        principal: String::new(),
        claim: String::new(),
        evidence: Vec::new(),
        scope: None,
    })
    .expect("attestation");
    assert_eq!(attestation.decision, AttestationDecision::NeedsAgent);
    assert_eq!(attestation.action, None);
}

#[test]
fn raw_fields_and_duplicate_references_are_refused() {
    let mut inputs = ready_inputs();
    let evidence = inputs.evidence[0].clone();
    let mut raw = evidence.as_object().expect("item").clone();
    raw.insert(
        "raw_value".to_owned(),
        JsonValue::String("secret".to_owned()),
    );
    inputs.evidence = vec![JsonValue::Object(raw), evidence];
    let attestation = build(&inputs).expect("attestation");
    assert_eq!(attestation.decision, AttestationDecision::NeedsMoreEvidence);
    let codes = attestation
        .gaps
        .iter()
        .map(|gap| gap.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"attestation.evidence.raw_fields"));
    assert!(codes.contains(&"attestation.evidence.duplicate"));
}

#[test]
fn a_secret_shaped_locator_never_reaches_the_packet() {
    let mut inputs = ready_inputs();
    inputs.evidence = vec![evidence_item("sk-live-0123456789")];
    let attestation = build(&inputs).expect("attestation");
    assert_eq!(attestation.decision, AttestationDecision::NeedsMoreEvidence);
    assert_eq!(attestation.evidence[0].locator, None);
    let packet = JsonValue::from_serialize(&attestation).expect("serialize");
    assert!(
        !canonical(&packet).contains("sk-live"),
        "the refused locator must not be sealed"
    );
}

fn canonical(value: &JsonValue) -> String {
    runx_contracts::canonical_stable_json(value).expect("canonical")
}
