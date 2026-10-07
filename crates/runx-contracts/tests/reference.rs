use runx_contracts::{PrincipalReference, Reference, ReferenceType, RunxPrincipalId};

#[test]
fn reference_type_as_str_is_stable_snake_case() {
    assert_eq!(ReferenceType::Receipt.as_str(), "receipt");
    assert_eq!(ReferenceType::Act.as_str(), "act");
    assert_eq!(ReferenceType::Verification.as_str(), "verification");
    assert_eq!(ReferenceType::Thread.as_str(), "thread");
    assert_eq!(ReferenceType::TrackingItem.as_str(), "tracking_item");
    assert_eq!(ReferenceType::ChangeRequest.as_str(), "change_request");
    assert_eq!(ReferenceType::Repository.as_str(), "repository");
    assert_eq!(ReferenceType::ExternalUrl.as_str(), "external_url");
}

#[test]
fn reference_runx_builds_canonical_scheme_uri() {
    let reference = Reference::runx(ReferenceType::Act, "abc");
    assert_eq!(reference.uri, "runx:act:abc");
    assert_eq!(reference.reference_type, ReferenceType::Act);
    assert!(reference.provider.is_none());
    assert!(reference.locator.is_none());
    assert!(reference.label.is_none());
    assert!(reference.proof_kind.is_none());
}

#[test]
fn reference_with_uri_preserves_explicit_uri() {
    let reference = Reference::with_uri(ReferenceType::Harness, "runx:harness:custom-id");
    assert_eq!(reference.uri, "runx:harness:custom-id");
    assert_eq!(reference.reference_type, ReferenceType::Harness);
    assert!(reference.provider.is_none());
    assert!(reference.proof_kind.is_none());
}

#[test]
fn runx_principal_id_accepts_only_the_current_hosted_grammar() {
    for value in [
        "user_1",
        "vendor-1",
        "service.prod",
        "claim:cr_123",
        "edge-key:sha256:abcdef0123456789",
        &"a".repeat(RunxPrincipalId::MAX_LENGTH),
    ] {
        assert!(
            RunxPrincipalId::new(value).is_some(),
            "expected valid: {value:?}"
        );
    }

    for value in [
        "",
        ".user",
        "-user",
        "_user",
        ":user",
        " user",
        "user ",
        "user name",
        "user/name",
        "user@example",
        "user+example",
        "usér",
        "user\nname",
        &"a".repeat(RunxPrincipalId::MAX_LENGTH + 1),
    ] {
        assert!(
            RunxPrincipalId::new(value).is_none(),
            "expected invalid: {value:?}"
        );
    }
}

#[test]
fn principal_reference_delegates_to_the_canonical_runx_writer() {
    let reference = RunxPrincipalId::new("edge-key:sha256:abcdef")
        .map(PrincipalReference::from_runx_principal_id);
    let expected = Reference::runx(ReferenceType::Principal, "edge-key:sha256:abcdef");

    assert_eq!(
        reference.as_ref().map(PrincipalReference::as_reference),
        Some(&expected),
    );
}
