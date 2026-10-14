#![allow(clippy::expect_used)]

use super::*;

#[test]
fn debug_output_never_exposes_hosted_api_tokens() {
    let resolved = HostedApiEnvironment {
        base_url: "https://api.runx.test".to_owned(),
        token: Some("rxk_super_secret".to_owned()),
        expected_principal_id: Some("operator:test".to_owned()),
        stored_credential_environment_mismatch: false,
        conflicting_tokens: false,
    };
    let authenticated = AuthenticatedHostedApiEnvironment {
        base_url: "https://api.runx.test".to_owned(),
        token: "rxk_super_secret".to_owned(),
        principal_id: RunxPrincipalId::new("operator:test").expect("valid principal id"),
    };

    assert!(!format!("{resolved:?}").contains("rxk_super_secret"));
    assert!(!format!("{authenticated:?}").contains("rxk_super_secret"));
}

#[test]
fn authenticated_principal_id_remains_typed_at_the_auth_boundary() {
    let authenticated = AuthenticatedHostedApiEnvironment {
        base_url: "https://api.runx.test".to_owned(),
        token: "rxk_super_secret".to_owned(),
        principal_id: RunxPrincipalId::new("operator:test").expect("valid principal id"),
    };

    assert_eq!(authenticated.principal_id(), "operator:test");
    assert_eq!(authenticated.runx_principal_id().as_str(), "operator:test");
}

#[test]
fn storing_a_hosted_environment_rejects_noncanonical_principal_ids() {
    let workspace = tempfile::tempdir().expect("workspace");

    let error = store_authenticated_hosted_environment(
        &BTreeMap::new(),
        workspace.path(),
        HostedApiCredentialPurpose::Default,
        "https://api.runx.test",
        " operator:test ",
        "rxk_super_secret",
    )
    .expect_err("principal ids are never trimmed or rewritten");

    assert!(error.to_string().contains("principal_id must match"));
}

#[test]
fn stored_default_and_publish_credentials_keep_independent_environment_bindings() {
    let workspace = tempfile::tempdir().expect("workspace");
    let env = BTreeMap::from([(
        "RUNX_HOME".to_owned(),
        workspace.path().to_string_lossy().into_owned(),
    )]);

    store_authenticated_hosted_environment(
        &env,
        workspace.path(),
        HostedApiCredentialPurpose::Default,
        "https://operator.runx.test",
        "operator_1",
        "rxk_operator",
    )
    .expect("store operator credential");
    store_authenticated_hosted_environment(
        &env,
        workspace.path(),
        HostedApiCredentialPurpose::Publish,
        "https://publish.runx.test",
        "publisher_1",
        "rxk_publisher",
    )
    .expect("store publisher credential");

    let operator = HostedApiEnvironment::resolve(None, &env, workspace.path())
        .expect("resolve operator credential");
    let publisher = HostedApiEnvironment::resolve_publish(None, &env, workspace.path())
        .expect("resolve publisher credential");
    assert_eq!(operator.base_url(), "https://operator.runx.test");
    assert_eq!(
        operator.require_token().expect("operator token"),
        "rxk_operator"
    );
    assert_eq!(publisher.base_url(), "https://publish.runx.test");
    assert_eq!(
        publisher.require_token().expect("publisher token"),
        "rxk_publisher"
    );
}

#[test]
fn hosted_api_base_url_requires_https_outside_loopback() {
    let env = BTreeMap::from([(
        HOSTED_API_BASE_URL_ENV.to_owned(),
        "http://api.runx.test".to_owned(),
    )]);

    let error = HostedApiEnvironment::resolve(None, &env, Path::new("."))
        .expect_err("public HTTP must fail closed");

    assert!(
        error
            .to_string()
            .contains("HTTP is allowed only for loopback")
    );
}

#[test]
fn hosted_api_base_url_allows_loopback_http_for_explicit_local_development() {
    let env = BTreeMap::new();

    let environment =
        HostedApiEnvironment::resolve(Some("http://127.0.0.1:4317/"), &env, Path::new("."))
            .expect("loopback environment");

    assert_eq!(environment.base_url(), "http://127.0.0.1:4317");
}

#[test]
fn hosted_api_base_url_rejects_embedded_credentials_and_query_state() {
    let env = BTreeMap::new();

    for base_url in [
        "/",
        "https://user:secret@api.runx.test",
        "https://api.runx.test?tenant=other",
        "https://api.runx.test#fragment",
    ] {
        assert!(
            HostedApiEnvironment::resolve(Some(base_url), &env, Path::new(".")).is_err(),
            "{base_url} must fail closed",
        );
    }
}
