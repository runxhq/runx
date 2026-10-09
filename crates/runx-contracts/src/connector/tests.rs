#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use super::*;
use crate::{JsonObject, JsonValue};

const SLACK: &str = r#"{
  "schema": "runx.connector.v1",
  "id": "slack",
  "name": "Slack",
  "description": "Search, read and post messages in Slack workspaces.",
  "auth": {"delegated": {}},
  "placements": {"hosted": {"auth": "delegated"}},
  "operations": {
    "channel.post": {
      "description": "Post a message to a channel.",
      "verb": "write",
      "scopes": ["channel.post"],
      "target": {"template": {"$argument": "/channel_locator"}, "pattern": "slack://[^/]+/[^/]+"},
      "approval": "required",
      "standing": {"kind": "notification", "target_field": "channel_locator", "text_field": "text"},
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

    assert!(is_connector_id("slack"));
    assert!(is_connector_id("google-calendar"));
    assert!(!is_connector_id("s"));
    assert!(!is_connector_id("slack-"));
    assert!(!is_connector_id("-slack"));
    assert!(!is_connector_id("Slack"));

    assert!(Digest::new(format!("sha256:{}", "a".repeat(64))).is_some());
    assert!(Digest::new(format!("sha256:{}", "A".repeat(64))).is_none());
    assert!(InstanceId::new("slack-acme.prod").is_some());
    assert!(EnvName::new("NITROSEND_API_KEY").is_some());
    assert!(EnvName::new("lower").is_none());
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
