use runx_contracts::AuthorityVerb;

use super::*;

#[cfg(all(feature = "catalog", unix))]
#[test]
fn local_github_identity_preflight_is_reused_within_one_effect_instance()
-> Result<(), Box<dyn std::error::Error>> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir()?;
    let gh = root.path().join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
dir=$(CDPATH= cd -- "$(/usr/bin/dirname -- "$0")" && pwd)
printf '%s\n' "$*" >> "$dir/argv.log"
/bin/cat >/dev/null
printf '%s\n' '{"data":{"viewer":{"id":"U_1","login":"operator"},"repository":{"nameWithOwner":"runxhq/runx","viewerPermission":"WRITE"}}}'
"#,
    )?;
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700))?;
    let env = BTreeMap::from([
        (
            "PATH".to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            crate::RUNX_CWD_ENV.to_owned(),
            root.path().to_string_lossy().into_owned(),
        ),
        (
            PROVIDER_PERMISSION_TRANSPORT_ENV.to_owned(),
            "local:github".to_owned(),
        ),
    ]);
    let effect = ProviderPermissionEffect::default();
    for operation in ["issue.read", "pullrequest.read"] {
        let mut step = native_step(PROVIDER_READ_TOOL, &["repo.read"], "read");
        policy_mut(&mut step).remove("grant_id");
        let inputs = JsonObject::from([
            (
                "expected_provider".to_owned(),
                JsonValue::String("github".to_owned()),
            ),
            (
                "operation".to_owned(),
                JsonValue::String(operation.to_owned()),
            ),
            (
                "target".to_owned(),
                JsonValue::String("runxhq/runx".to_owned()),
            ),
        ]);
        effect
            .admit(effect_request(&step, &inputs, &env))?
            .ok_or("provider admission")?;
    }
    let log = fs::read_to_string(root.path().join("argv.log"))?;
    assert_eq!(log.lines().count(), 1, "identity preflight must run once");
    Ok(())
}

#[test]
fn provider_capabilities_bind_idempotency_and_approval_to_mutation_only() {
    let effect = ProviderPermissionEffect::default();
    let read = effect
        .capabilities()
        .iter()
        .find(|tool| tool.definition().id == PROVIDER_READ_TOOL)
        .expect("provider.read");
    let mutate = effect
        .capabilities()
        .iter()
        .find(|tool| tool.definition().id == PROVIDER_MUTATE_TOOL)
        .expect("provider.mutate");

    assert!(
        !read
            .catalog_inputs()
            .expect("read inputs")
            .contains_key("idempotency_key")
    );
    assert!(mutate.catalog_inputs().expect("mutate inputs")["idempotency_key"].required);
    assert!(!mutate.catalog_inputs().expect("mutate inputs")["amount"].required);
    assert_eq!(read.definition().approval, crate::CapabilityApproval::None);
    assert_eq!(
        mutate.definition().approval,
        crate::CapabilityApproval::Effect
    );
}

#[test]
fn assistant_mutations_require_one_native_human_gate_even_when_skill_omits_approval() {
    let effect = ProviderPermissionEffect::default();
    let step = native_step(PROVIDER_MUTATE_TOOL, &["thread.reply"], "write");
    let inputs = provider_inputs("thread.reply");
    let mut env = provider_env("github-mcp-read", "thread.reply");

    let ordinary = effect
        .admit(effect_request(&step, &inputs, &env))
        .expect("ordinary provider admission")
        .expect("owned provider effect");
    let ordinary = ordinary
        .context::<ProviderPermissionAdmission>()
        .expect("ordinary provider context");
    assert!(ordinary.approval_request.is_none());
    assert!(
        !ordinary
            .provider_effect
            .as_ref()
            .expect("effect")
            .intent()
            .requires_approval()
    );

    env.insert(
        ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "required".to_owned(),
    );
    env.insert(ASSISTANT_CONFIDENTIAL_TERMS_ENV.to_owned(), "[]".to_owned());
    let assistant = effect
        .admit(effect_request(&step, &inputs, &env))
        .expect("assistant provider admission")
        .expect("owned provider effect");
    let assistant = assistant
        .context::<ProviderPermissionAdmission>()
        .expect("assistant provider context");
    assert_eq!(
        assistant
            .approval_request
            .as_ref()
            .and_then(|request| request.gate_type.as_deref()),
        Some("assistant_outward")
    );
    assert!(
        assistant
            .provider_effect
            .as_ref()
            .expect("effect")
            .intent()
            .requires_approval()
    );

    let mut declared = inputs.clone();
    declared.insert(
        "approval".to_owned(),
        JsonValue::Object(JsonObject::from([
            (
                "reason".to_owned(),
                JsonValue::String("Approve the exact reply.".to_owned()),
            ),
            (
                "type".to_owned(),
                JsonValue::String("slack_message".to_owned()),
            ),
        ])),
    );
    let existing = effect
        .admit(effect_request(&step, &declared, &env))
        .expect("declared approval admission")
        .expect("owned provider effect");
    let existing = existing
        .context::<ProviderPermissionAdmission>()
        .expect("declared provider context");
    assert_eq!(
        existing
            .approval_request
            .as_ref()
            .and_then(|request| request.gate_type.as_deref()),
        Some("slack_message"),
    );
}

#[test]
fn assistant_egress_refuses_protected_payloads_before_provider_effect_admission() {
    let effect = ProviderPermissionEffect::default();
    let step = native_step(PROVIDER_MUTATE_TOOL, &["thread.reply"], "write");
    let mut env = provider_env("github-mcp-read", "thread.reply");
    env.insert(
        ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "required".to_owned(),
    );
    let mut inputs = provider_inputs("thread.reply");
    assert!(effect.admit(effect_request(&step, &inputs, &env)).is_err());

    env.insert(
        ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "disabled".to_owned(),
    );
    assert!(effect.admit(effect_request(&step, &inputs, &env)).is_err());
    env.insert(
        ASSISTANT_REQUIRE_MUTATION_APPROVAL_ENV.to_owned(),
        "required".to_owned(),
    );

    env.insert(
        ASSISTANT_CONFIDENTIAL_TERMS_ENV.to_owned(),
        "[\"private.example.test\"]".to_owned(),
    );
    env.insert(
        "SERVICE_API_KEY".to_owned(),
        "private-token-value".to_owned(),
    );
    for text in [
        "Look in /Users/operator/dev/project",
        "Read /etc/hosts",
        "Path=/mnt/work/report",
        "deploy log at '/opt/app/logs'",
        "see \"/srv/data/report\"",
        "see\r\n/opt/data",
        "The file is D:\\dev\\repo\\secret",
        "See %252fprivate%252ftmp%252fresult",
        "Use SERVICE_API_KEY",
        "Use $HOME for the working directory",
        "The value is private-token-value",
        "Check private.example.test",
    ] {
        inputs.insert(
            "input".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "text".to_owned(),
                JsonValue::String(text.to_owned()),
            )])),
        );
        let error = effect
            .admit(effect_request(&step, &inputs, &env))
            .expect_err("protected text must be rejected before approval or dispatch");
        assert!(matches!(error, RuntimeEffectError::Denied { .. }));
        assert!(!error.to_string().contains(text));
    }
    for field in ["operation", "target", "idempotency_key"] {
        let mut protected = provider_inputs("thread.reply");
        protected.insert(
            field.to_owned(),
            JsonValue::String("value '/opt/app/logs'".to_owned()),
        );
        assert!(matches!(
            effect.admit(effect_request(&step, &protected, &env)),
            Err(RuntimeEffectError::Denied { .. })
        ));
    }
    let mut nested = provider_inputs("thread.reply");
    nested.insert(
        "input".to_owned(),
        JsonValue::Object(JsonObject::from([(
            "'/opt/app/logs'".to_owned(),
            JsonValue::Array(vec![JsonValue::String("safe".to_owned())]),
        )])),
    );
    assert!(matches!(
        effect.admit(effect_request(&step, &nested, &env)),
        Err(RuntimeEffectError::Denied { .. })
    ));
    let mut protected_scope_step = step.clone();
    protected_scope_step.scopes = vec!["thread.reply '/opt/app/logs'".to_owned()];
    let mut protected_scope_env = env.clone();
    protected_scope_env.insert(
        PROVIDER_PERMISSION_GRANTED_SCOPES_ENV.to_owned(),
        encode_provider_scopes_env(&protected_scope_step.scopes).expect("scope transport"),
    );
    assert!(matches!(
        effect.admit(effect_request(
            &protected_scope_step,
            &inputs,
            &protected_scope_env
        )),
        Err(RuntimeEffectError::Denied { .. })
    ));
    inputs.insert(
        "input".to_owned(),
        JsonValue::Object(JsonObject::from([(
            "text".to_owned(),
            JsonValue::String("I will review https://example.com/issues/1 today.".to_owned()),
        )])),
    );
    assert!(effect.admit(effect_request(&step, &inputs, &env)).is_ok());
}

#[test]
fn admitted_verbs_bind_grant_and_scope_evidence() {
    for (verb, expected) in [
        ("read", AuthorityVerb::Read),
        ("revoke", AuthorityVerb::Revoke),
    ] {
        let effect = ProviderPermissionEffect::default();
        let step = test_step("provider", &["repo.read"], verb);
        let inputs = JsonObject::new();
        let env = provider_env("github-mcp-read", "repo.read");
        let admission = effect
            .admit(effect_request(&step, &inputs, &env))
            .expect("admission")
            .expect("owned provider effect");

        assert_eq!(admission.verb(), expected);
        assert_eq!(
            effect.authority_grant_refs(&admission).expect("grant refs")[0].reference_type,
            ReferenceType::Grant
        );
        assert_eq!(
            effect.authority_scope_refs(&admission).expect("scope refs")[0].reference_type,
            ReferenceType::ScopeAdmission
        );
    }
}

#[test]
fn authority_term_identity_cannot_collide_across_opaque_scope_lists() {
    let effect = ProviderPermissionEffect::default();
    let inputs = JsonObject::new();
    let scopes = ["a+b".to_owned(), "a".to_owned(), "b".to_owned()];
    let env = BTreeMap::from([
        (
            PROVIDER_PERMISSION_GRANT_ID_ENV.to_owned(),
            "github-mcp-read".to_owned(),
        ),
        (
            PROVIDER_PERMISSION_GRANTED_SCOPES_ENV.to_owned(),
            encode_provider_scopes_env(&scopes).expect("scope transport"),
        ),
    ]);
    let admission = |required: &[&str]| {
        let step = test_step("provider", required, "read");
        effect
            .admit(effect_request(&step, &inputs, &env))
            .expect("admission")
            .expect("owned provider effect")
            .witness()
            .child_term_id
            .clone()
    };

    assert_ne!(admission(&["a+b"]), admission(&["a", "b"]));
}

#[test]
fn admission_rejects_missing_scope_grant_and_self_attestation() {
    let effect = ProviderPermissionEffect::default();
    let inputs = JsonObject::new();

    let missing_scope = test_step("write", &["repo.write"], "write");
    let scope_env = provider_env("github-mcp-read", "repo.read");
    assert!(matches!(
        effect.admit(effect_request(&missing_scope, &inputs, &scope_env)),
        Err(RuntimeEffectError::Denied { ref message, .. }) if message.contains("repo.write")
    ));

    let missing_grant = BTreeMap::from([(
        PROVIDER_PERMISSION_GRANTED_SCOPES_ENV.to_owned(),
        encode_provider_scopes_env(&["repo.read".to_owned()]).expect("scope transport"),
    )]);
    let read = test_step("read", &["repo.read"], "read");
    assert!(matches!(
        effect.admit(effect_request(&read, &inputs, &missing_grant)),
        Err(RuntimeEffectError::Denied { ref message, .. }) if message.contains(PROVIDER_PERMISSION_GRANT_ID_ENV)
    ));

    let mut self_attested = test_step("read", &["repo.read"], "read");
    policy_mut(&mut self_attested).insert(
        "granted_scopes".to_owned(),
        JsonValue::Array(vec![JsonValue::String("repo.read".to_owned())]),
    );
    assert!(matches!(
        effect.admit(effect_request(&self_attested, &inputs, &scope_env)),
        Err(RuntimeEffectError::Denied { ref message, .. }) if message.contains("self-attested")
    ));
}

#[test]
fn malformed_policy_fails_closed() {
    let effect = ProviderPermissionEffect::default();
    let inputs = JsonObject::new();
    let env = provider_env("github-mcp-read", "repo.read");

    let mut missing_verb = test_step("read", &["repo.read"], "read");
    policy_mut(&mut missing_verb).remove("verb");
    assert_policy_error(
        effect
            .admit(effect_request(&missing_verb, &inputs, &env))
            .expect_err("missing verb"),
        "verb is required",
    );

    let unknown_verb = test_step("read", &["repo.read"], "publish");
    assert_policy_error(
        effect
            .admit(effect_request(&unknown_verb, &inputs, &env))
            .expect_err("unknown verb"),
        "not supported",
    );

    let mut malformed_scope = test_step("read", &["repo.read"], "read");
    policy_mut(&mut malformed_scope).insert(
        "required_scopes".to_owned(),
        JsonValue::Array(vec![JsonValue::Bool(false)]),
    );
    assert_policy_error(
        effect
            .admit(effect_request(&malformed_scope, &inputs, &env))
            .expect_err("malformed scope"),
        "required_scopes[0] must be a string",
    );
}

#[test]
fn native_provider_tools_require_matching_access_policy_and_explicit_identity() {
    let effect = ProviderPermissionEffect::default();
    let inputs = provider_inputs("channel.post");
    let env = provider_env("github-mcp-read", "channel.post");

    let mut missing_policy = native_step(PROVIDER_READ_TOOL, &["channel.post"], "read");
    missing_policy.policy = None;
    assert!(
        effect
            .admit(effect_request(&missing_policy, &inputs, &env))
            .is_err()
    );

    let read_with_write = native_step(PROVIDER_READ_TOOL, &["channel.post"], "write");
    assert!(matches!(
        effect.admit(effect_request(&read_with_write, &inputs, &env)),
        Err(RuntimeEffectError::Denied { .. })
    ));

    let mutate_with_read = native_step(PROVIDER_MUTATE_TOOL, &["channel.post"], "read");
    assert!(matches!(
        effect.admit(effect_request(&mutate_with_read, &inputs, &env)),
        Err(RuntimeEffectError::Denied { .. })
    ));

    let step = native_step(PROVIDER_MUTATE_TOOL, &["channel.post"], "write");
    let admission = effect
        .admit(effect_request(&step, &inputs, &env))
        .expect("native admission")
        .expect("owned provider effect");
    assert_eq!(
        admission
            .context::<ProviderPermissionAdmission>()
            .map(|context| context.grant_id.as_str()),
        Some("github-mcp-read")
    );
}

#[test]
fn hosted_provider_explicit_binding_and_unambiguous_fallback_remain_supported() {
    let effect = ProviderPermissionEffect::default();
    let inputs = provider_inputs("messages.search");
    let env = provider_env("github-mcp-read", "messages.search");
    let step = native_step(PROVIDER_READ_TOOL, &["messages.search"], "read");
    let admission = effect
        .admit(effect_request(&step, &inputs, &env))
        .expect("explicit hosted identity")
        .expect("provider admission");
    assert_eq!(
        admission
            .context::<ProviderPermissionAdmission>()
            .map(|context| context.grant_id.as_str()),
        Some("github-mcp-read")
    );

    let github_scopes = ["repo.read".to_owned()];
    let slack_scopes = ["messages.search".to_owned()];
    let grants = [
        HostedGrantView {
            grant_id: "grant_other",
            provider: "github",
            scopes: &github_scopes,
            status: "active",
            target_locator: None,
        },
        HostedGrantView {
            grant_id: "grant_only",
            provider: "slack",
            scopes: &slack_scopes,
            status: "active",
            target_locator: None,
        },
    ];
    let selected = select_hosted_provider_grant_index(
        &grants,
        "slack",
        &["messages.search".to_owned()],
        None,
        None,
    )
    .expect("unambiguous hosted fallback");
    assert_eq!(grants[selected].grant_id, "grant_only");
}

#[test]
fn hosted_provider_grants_use_bounded_delegated_wildcards() {
    let delegated_scopes = ["repo:*".to_owned()];
    let universal_scopes = ["*".to_owned()];
    let grants = [
        HostedGrantView {
            grant_id: "grant_delegated",
            provider: "github",
            scopes: &delegated_scopes,
            status: "active",
            target_locator: None,
        },
        HostedGrantView {
            grant_id: "grant_universal",
            provider: "github",
            scopes: &universal_scopes,
            status: "active",
            target_locator: None,
        },
    ];

    let selected = select_hosted_provider_grant_index(
        &grants,
        "github",
        &["repo:read".to_owned()],
        None,
        None,
    )
    .expect("bounded delegated grant");
    assert_eq!(grants[selected].grant_id, "grant_delegated");
    assert!(
        select_hosted_provider_grant_index(
            &grants,
            "github",
            &["repo:admin:keys".to_owned()],
            None,
            None,
        )
        .is_err()
    );
}

#[cfg(feature = "catalog")]
#[test]
fn host_injected_provider_authority_rejects_conflicting_transport_bindings() {
    let effect = ProviderPermissionEffect::default();
    let inputs = JsonObject::from([
        (
            "expected_provider".to_owned(),
            JsonValue::String("github".to_owned()),
        ),
        (
            "operation".to_owned(),
            JsonValue::String("issues.read".to_owned()),
        ),
        (
            "target".to_owned(),
            JsonValue::String("nitrosend/nitrosend".to_owned()),
        ),
    ]);
    let step = native_step(PROVIDER_READ_TOOL, &["repo.read"], "read");

    let mut local_env = provider_env("github-mcp-read", "repo.read");
    local_env.insert(
        PROVIDER_PERMISSION_TRANSPORT_ENV.to_owned(),
        "local:github".to_owned(),
    );
    let local_error = effect
        .admit(effect_request(&step, &inputs, &local_env))
        .expect_err("hosted authority must not override a local transport binding");
    assert!(
        matches!(local_error, RuntimeEffectError::Denied { ref message, .. }
            if message.contains("conflicts with explicit local:github")
                && message.contains(PROVIDER_PERMISSION_GRANT_ID_ENV)),
        "unexpected local transport conflict: {local_error:?}"
    );

    let mut hosted_env = provider_env("github-mcp-read", "repo.read");
    hosted_env.insert(
        PROVIDER_PERMISSION_TRANSPORT_ENV.to_owned(),
        "hosted:different-grant".to_owned(),
    );
    let hosted_error = effect
        .admit(effect_request(&step, &inputs, &hosted_env))
        .expect_err("hosted authority must match an exact hosted binding");
    assert!(
        matches!(hosted_error, RuntimeEffectError::Denied { ref message, .. }
            if message.contains("does not match the explicit hosted:different-grant")),
        "unexpected hosted transport conflict: {hosted_error:?}"
    );
}

#[cfg(feature = "catalog")]
#[test]
fn missing_hosted_provider_auth_is_an_actionable_denial_not_receipt_corruption() {
    let effect = ProviderPermissionEffect::default();
    let inputs = provider_inputs("messages.search");
    let env = BTreeMap::from([(
        "RUNX_HOME".to_owned(),
        ".runx/tests/provider-missing-auth".to_owned(),
    )]);
    let step = native_step(PROVIDER_READ_TOOL, &["messages.search"], "read");

    let error = effect
        .admit(effect_request(&step, &inputs, &env))
        .expect_err("missing hosted authentication must stop in preflight");

    assert!(
        matches!(error, RuntimeEffectError::Denied { verb: AuthorityVerb::Read, ref message, .. }
            if message.contains("missing public API token") && message.contains("runx login")),
        "unexpected hosted preflight error: {error:?}"
    );
}
