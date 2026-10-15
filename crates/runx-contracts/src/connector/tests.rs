#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use serde_json::json;

use super::*;
use crate::{JsonObject, JsonValue};

const SLACK: &str = r#"{
  "schema": "runx.connector.v1",
  "id": "slack",
  "name": "Slack",
  "description": "Search, read and post messages in Slack workspaces.",
  "runtime_capability": false,
  "auth": {"delegated": {"principal": {"url": "https://slack.com/api/auth.test", "id": "user_id", "login": "user"}}},
  "placements": {"hosted": {"auth": "delegated"}},
  "operations": {
    "channel.post": {
      "description": "Post a message to a channel.",
      "verb": "write",
      "scopes": ["channel.post"],
      "target": {"template": {"$argument": "/channel_locator"}, "pattern": "slack://[^/]+/[^/]+"},
      "approval": "required",
      "idempotency_key_format": "uuid",
      "result_fields": ["message_locator"],
      "hosted": {"provider_scopes": ["chat:write"]}
    },
    "channel.post.read": {
      "description": "Read a posted message back.",
      "verb": "read",
      "scopes": ["channel.post.read"],
      "target": {"template": {"$argument": "/conversation_locator"}},
      "hosted": {"provider_scopes": ["channels:history"]}
    }
  }
}"#;

fn slack() -> ConnectorManifest {
    serde_json::from_str(SLACK).expect("slack manifest parses")
}

#[test]
fn identifiers_follow_their_grammar() {
    assert!(is_operation_name("channel.post"));
    assert!(is_operation_name("reports.run_realtime"));
    assert!(is_operation_name("sync.write_batch"));
    assert!(!is_operation_name("channel"));
    assert!(!is_operation_name("Channel.post"));
    assert!(!is_operation_name("channel..post"));
    assert!(!is_operation_name("_channel.post"));
    assert!(!is_operation_name(&format!("a.{}", "b".repeat(99))));

    // A connector name is a namespace, or a namespace and one surface; the
    // published pattern and the check agree on every case.
    let pattern = jsonschema::validator_for(&json!({
        "type": "string",
        "pattern": CONNECTOR_NAME_PATTERN,
        "maxLength": 129
    }))
    .unwrap();
    for (name, valid) in [
        ("stripe", true),
        ("github", true),
        ("google/calendar", true),
        ("google/search-console", true),
        ("wallet/evm", true),
        ("ausca/browser", true),
        ("x", true),
        ("4chan", true),
        ("acme/notion", true),
        ("national-weather-service", true),
        ("google/4k", false),
        ("google/c", false),
        ("google/calendar/events", false),
        ("google/", false),
        ("/calendar", false),
        ("versions", false),
        ("versions/evm", false),
        ("google/versions", false),
        ("versioned", true),
        ("google/versioning", true),
        ("Slack", false),
        ("slack-", false),
        ("-slack", false),
        ("slack_bot", false),
        ("", false),
    ] {
        assert_eq!(is_connector_name(name), valid, "check: {name}");
        assert_eq!(pattern.is_valid(&json!(name)), valid, "pattern: {name}");
    }
    let name = ConnectorName::new("google/calendar").unwrap();
    assert_eq!(
        (name.namespace(), name.surface()),
        ("google", Some("calendar"))
    );
    let leaf = ConnectorName::new("stripe").unwrap();
    assert_eq!((leaf.namespace(), leaf.surface()), ("stripe", None));

    assert!(Digest::new(format!("sha256:{}", "a".repeat(64))).is_some());
    assert!(Digest::new(format!("sha256:{}", "A".repeat(64))).is_none());
    assert!(InstanceId::new("slack-acme.prod").is_some());
    assert!(EnvName::new("NITROSEND_API_KEY").is_some());
    assert!(EnvName::new("lower").is_none());
}

#[test]
fn a_secret_reference_names_a_manager_runx_reads_and_never_a_value() {
    let reference = SecretReference::new("aws-sm://team/keys#TOKEN").unwrap();
    assert_eq!(
        (reference.scheme(), reference.locator(), reference.field()),
        ("aws-sm", "team/keys", Some("TOKEN"))
    );
    let arn = SecretReference::new("aws-ps://arn:aws:ssm:us-east-1:1:parameter/x").unwrap();
    assert_eq!(
        (arn.locator(), arn.field()),
        ("arn:aws:ssm:us-east-1:1:parameter/x", None)
    );
    // 1Password's own references name items as people do, spaces and all.
    let item = SecretReference::new("op://Private/Apollo API/credential").unwrap();
    assert_eq!(
        (item.scheme(), item.locator(), item.field()),
        ("op", "Private/Apollo API/credential", None)
    );
    for refused in [
        "sk-live-123",
        "gcp-sm://projects/p/secrets/s",
        "aws-sm://",
        "aws-sm://name#",
        "aws-sm://name#a#b",
        "aws-sm://name#a b",
        "vault://secret/x\ty",
        "vault://secret/x\ny",
        "op:// Private/item/field",
        "op://Private/item/field ",
        "keychain://",
    ] {
        assert!(SecretReference::new(refused).is_none(), "{refused}");
    }
    // The schema's pattern admits exactly the schemes the parser does, and a
    // refusal names each.
    for scheme in SECRET_SCHEMES {
        assert!(
            SECRET_REFERENCE_EXPECTED.contains(&format!("{scheme}://")),
            "{scheme}"
        );
    }
    assert!(SECRET_REFERENCE_PATTERN.starts_with(&format!("^({})://", SECRET_SCHEMES.join("|"))));
}

#[test]
fn manifest_round_trips_and_rejects_unknown_fields() {
    let manifest = slack();
    let operation = manifest.operation("channel.post").expect("operation");
    assert_eq!(operation.access(), OperationAccess::Mutate);
    assert_eq!(operation.approval, ApprovalFloor::Required);
    assert!(operation.implemented_by(Placement::Hosted));
    assert!(!operation.implemented_by(Placement::Local));

    let encoded = serde_json::to_string(&manifest).expect("encode");
    let decoded: ConnectorManifest = serde_json::from_str(&encoded).expect("decode");
    assert_eq!(decoded, manifest);

    let unknown = SLACK.replace(
        "\"name\": \"Slack\",",
        "\"name\": \"Slack\", \"lane\": \"x\",",
    );
    assert!(serde_json::from_str::<ConnectorManifest>(&unknown).is_err());
    let bad_operation = SLACK.replace("\"channel.post.read\": {", "\"Channel.Read\": {");
    assert!(serde_json::from_str::<ConnectorManifest>(&bad_operation).is_err());
}

#[test]
fn operation_digest_binds_authority_not_copy_or_implementation() {
    let manifest = slack();
    let name = OperationName::new("channel.post").expect("name");
    let digest = |manifest: &ConnectorManifest| {
        operation_digest(
            &manifest.id,
            &name,
            manifest.operation("channel.post").expect("op"),
        )
        .expect("digest")
    };
    let base = digest(&manifest);

    let mut copy_edit = manifest.clone();
    copy_edit
        .operations
        .values_mut()
        .for_each(|operation| operation.description = "Different words.".into());
    assert_eq!(digest(&copy_edit), base);

    let mut provider_scopes = manifest.clone();
    provider_scopes
        .operations
        .values_mut()
        .for_each(|operation| operation.hosted = Some(HostedImplementation::default()));
    assert_eq!(digest(&provider_scopes), base);

    let mut widened = manifest.clone();
    widened.operations.values_mut().for_each(|operation| {
        operation.scopes = vec!["channel.post".into(), "admin".into()].into();
    });
    assert_ne!(digest(&widened), base);

    let mut floor = manifest.clone();
    floor
        .operations
        .values_mut()
        .for_each(|operation| operation.approval = ApprovalFloor::None);
    assert_ne!(digest(&floor), base);

    assert_ne!(
        manifest_digest(&copy_edit).expect("m"),
        manifest_digest(&manifest).expect("m")
    );
}

#[test]
fn templates_bind_arguments_parameters_and_runtime_values() {
    let template: Template = serde_json::from_str(
        r#"{"url": {"$concat": ["https://api.example.com/", {"$parameter": "/workspace"}, "/", {"$argument": "/id"}]},
            "key": {"$runx": "idempotency_key"},
            "ack": {"$response": "write.json.id"},
            "literal": [1, true, null]}"#,
    )
    .expect("template parses");
    let arguments: JsonValue = serde_json::from_str(r#"{"id": 42}"#).expect("args");
    let parameters: JsonObject =
        BTreeMap::from([("workspace".to_owned(), JsonValue::String("acme".to_owned()))]);
    let responses = BTreeMap::from([(
        "write".to_owned(),
        BTreeMap::from([(
            "json".to_owned(),
            serde_json::from_str::<JsonValue>(r#"{"id": "m-1"}"#).expect("response"),
        )]),
    )]);
    let rendered = template
        .render(&TemplateInputs {
            arguments: Some(&arguments),
            parameters: Some(&parameters),
            responses: Some(&responses),
            idempotency_key: Some("runx:sha256:abc"),
            ..TemplateInputs::default()
        })
        .expect("renders");
    let expected: JsonValue = serde_json::from_str(
        r#"{"url": "https://api.example.com/acme/42", "key": "runx:sha256:abc", "ack": "m-1", "literal": [1, true, null]}"#,
    )
    .expect("expected");
    assert_eq!(rendered, expected);

    let missing = template.render(&TemplateInputs {
        arguments: Some(&arguments),
        ..TemplateInputs::default()
    });
    assert!(matches!(missing, Err(TemplateError::Unavailable { .. })));
}

#[test]
fn an_item_binding_renders_only_for_a_repeated_request() {
    let template: Template =
        serde_json::from_str(r#"{"$concat": ["https://api.example.com/vulns/", {"$item": ""}]}"#)
            .expect("template");
    assert!(template.binds_item());
    let item = JsonValue::String("GHSA-1".to_owned());
    assert_eq!(
        template
            .render_string(&TemplateInputs {
                item: Some(&item),
                ..TemplateInputs::default()
            })
            .expect("renders"),
        "https://api.example.com/vulns/GHSA-1"
    );
    assert!(matches!(
        template.render(&TemplateInputs::default()),
        Err(TemplateError::Unavailable { .. })
    ));
    let plain: Template = serde_json::from_str(r#"{"$argument": "/id"}"#).expect("template");
    assert!(!plain.binds_item());
}

/// The TS renderer answers the same (packages/contracts/src/connector.test.ts).
#[test]
fn the_paid_invocation_renders_only_where_it_is_supplied() {
    let template: Template = serde_json::from_str(
        r#"{"invocation": {"$runx": "invocation"}, "key": {"$runx": "idempotency_key"}}"#,
    )
    .expect("template");
    assert!(template.binds_invocation());
    assert!(template.binds_idempotency_key());
    let invocation: Template =
        serde_json::from_str(r#"{"$runx": "invocation"}"#).expect("template");
    assert!(!invocation.binds_idempotency_key());
    let expected: JsonValue =
        serde_json::from_str(r#"{"invocation": "pinv_1", "key": "runx:sha256:abc"}"#)
            .expect("expected");
    assert_eq!(
        template
            .render(&TemplateInputs {
                idempotency_key: Some("runx:sha256:abc"),
                invocation_id: Some("pinv_1"),
                ..TemplateInputs::default()
            })
            .expect("renders"),
        expected
    );
    // The kernel holds the call's key but never the paid invocation.
    assert!(matches!(
        template.render(&TemplateInputs {
            idempotency_key: Some("runx:sha256:abc"),
            ..TemplateInputs::default()
        }),
        Err(TemplateError::Unavailable { binding: "$runx", reference }) if reference == "invocation"
    ));
}

/// The TS renderer answers the same (packages/contracts/src/connector.test.ts).
#[test]
fn an_effect_reads_the_call_result_and_a_repeated_element() {
    let locator: Template = serde_json::from_str(
        r#"{"$concat": ["github://", {"$argument": "/repository"}, "/issues/", {"$item": "/number"}]}"#,
    )
    .expect("template");
    let arguments: JsonValue =
        serde_json::from_str(r#"{"repository": "acme/app"}"#).expect("arguments");
    let item: JsonValue = serde_json::from_str(r#"{"number": 7}"#).expect("item");
    assert_eq!(
        locator
            .render_string(&TemplateInputs {
                arguments: Some(&arguments),
                item: Some(&item),
                ..TemplateInputs::default()
            })
            .expect("renders"),
        "github://acme/app/issues/7"
    );
    let id: Template = serde_json::from_str(r#"{"$result": "/post/id"}"#).expect("template");
    assert!(id.binds_result());
    assert_eq!(id.result_pointer(), Some("/post/id"));
    let result: JsonValue = serde_json::from_str(r#"{"post": {"id": "19"}}"#).expect("result");
    assert_eq!(
        id.render_string(&TemplateInputs {
            result: Some(&result),
            ..TemplateInputs::default()
        })
        .expect("renders"),
        "19"
    );
    assert!(matches!(
        id.render(&TemplateInputs::default()),
        Err(TemplateError::Unavailable { .. })
    ));
}

#[test]
fn malformed_templates_fail_to_parse() {
    for malformed in [
        r#"{"$argument": 3}"#,
        r#"{"$argument": "no-slash"}"#,
        r#"{"$runx": "secret"}"#,
        r#"{"$concat": []}"#,
        r#"{"$response": "write"}"#,
        r#"{"$argument": "/a", "extra": 1}"#,
        r#"{"$unknown": "/a"}"#,
    ] {
        assert!(
            serde_json::from_str::<Template>(malformed).is_err(),
            "{malformed} should not parse"
        );
    }
}

#[test]
fn literal_fields_expose_mcp_discriminators() {
    let arguments: Template =
        serde_json::from_str(r#"{"action": "status", "plan": {"$argument": "/plan"}}"#)
            .expect("arguments");
    let literal = arguments.literal_fields();
    assert_eq!(literal.len(), 1);
    assert_eq!(
        literal.get("action"),
        Some(&&JsonValue::String("status".to_owned()))
    );
}

#[test]
fn connector_digest_binds_the_manifest_and_each_tool_it_runs() {
    let manifest = Digest::of(b"manifest");
    let push = Digest::of(b"push tool");
    let read = Digest::of(b"read tool");
    let base = connector_digest(&manifest, [("push", &push), ("read", &read)]).expect("digest");

    // The tool list is a set by name; the order it arrives in is not identity.
    assert_eq!(
        connector_digest(&manifest, [("read", &read), ("push", &push)]).expect("digest"),
        base
    );
    // A changed tool or an added one changes the connector; so does its manifest.
    let changed = Digest::of(b"push tool, edited");
    assert_ne!(
        connector_digest(&manifest, [("push", &changed), ("read", &read)]).expect("digest"),
        base
    );
    assert_ne!(
        connector_digest(&manifest, [("push", &push)]).expect("digest"),
        base
    );
    assert_ne!(
        connector_digest(&Digest::of(b"other"), [("push", &push), ("read", &read)])
            .expect("digest"),
        base
    );
    // A tool-less connector is its manifest alone, still not the manifest digest.
    let bare = connector_digest(&manifest, []).expect("digest");
    assert_ne!(bare, manifest);
    assert_eq!(
        bare.as_str(),
        stable_digest(&serde_json::json!({ "manifest": manifest, "tools": {} }))
            .expect("digest")
            .as_str()
    );
    assert!(connector_digest(&manifest, [("push", &push), ("push", &read)]).is_err());
}
