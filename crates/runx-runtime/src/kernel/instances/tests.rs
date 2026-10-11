#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::connector::{InstanceId, Placement};
use runx_contracts::connector_instance::ConnectorInstanceSchema;
use runx_contracts::schema::IsoDateTime;
use runx_contracts::{ConnectorInstance, InstanceAuthority};

use super::*;
use crate::kernel::connectors::ConnectorScope;
use crate::kernel::store::KernelStore;

fn instances() -> (tempfile::TempDir, Instances) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("store");
    (root, Instances::new(store))
}

fn slack() -> std::sync::Arc<LoadedConnector> {
    ConnectorScope::official_only()
        .expect("scope")
        .connector("slack")
        .expect("slack")
}

fn hosted(id: &str, connector: &LoadedConnector) -> ConnectorInstance {
    ConnectorInstance {
        schema: ConnectorInstanceSchema::V1,
        instance_id: InstanceId::new(id).expect("id"),
        connector: connector.connector_ref.clone(),
        manifest_digest: connector.manifest_digest.clone(),
        connector_digest: connector.connector_digest.clone(),
        placement: Placement::Hosted,
        principal_ref: "runx:principal:operator".into(),
        account_ref: Some("T123/U456".into()),
        authority: InstanceAuthority::HostedGrant {
            grant_id: "grant_slack".into(),
        },
        parameters: BTreeMap::new(),
        credential_declaration_digest: None,
        operation_digests: connector.operation_digests.clone(),
        confirmed_at: IsoDateTime::from("2026-10-06T00:00:00Z"),
    }
}

#[test]
fn a_selected_instance_resolves_and_a_revoked_one_refuses() {
    let (_root, instances) = instances();
    let slack = slack();
    let env = BTreeMap::new();
    assert!(matches!(
        instances.resolve(&slack, &env),
        Err(InstanceError::NotSelected { .. })
    ));

    instances
        .confirm(hosted("slack-acme", &slack), true)
        .expect("confirm");
    let resolved = instances.resolve(&slack, &env).expect("resolves");
    assert_eq!(resolved.instance.instance_id.as_str(), "slack-acme");
    assert_eq!(resolved.source, InstanceSource::Selected);
    assert!(
        instances
            .get("slack-acme")
            .expect("get")
            .expect("record")
            .selected
    );

    instances
        .confirm(hosted("slack-other", &slack), false)
        .expect("confirm");
    assert_eq!(
        instances
            .resolve(&slack, &env)
            .expect("still selected")
            .instance
            .instance_id
            .as_str(),
        "slack-acme"
    );

    instances.revoke("slack-acme").expect("revoke");
    assert!(matches!(
        instances.resolve(&slack, &env),
        Err(InstanceError::Revoked { .. })
    ));
    assert_eq!(instances.list().expect("list").len(), 2);
}

#[test]
fn an_instance_id_cannot_move_to_another_connector() {
    let (_root, instances) = instances();
    let slack = slack();
    instances
        .confirm(hosted("shared-id", &slack), false)
        .expect("confirm");
    let github = ConnectorScope::official_only()
        .unwrap()
        .connector("github")
        .unwrap();
    let mut moved = hosted("shared-id", &github);
    moved.connector = github.connector_ref.clone();
    assert!(matches!(
        instances.confirm(moved, false),
        Err(InstanceError::WrongConnector { .. })
    ));
}

#[test]
fn a_public_connector_needs_no_setup() {
    let (_root, instances) = instances();
    let public = ConnectorScope::official_only()
        .unwrap()
        .connector("open-meteo")
        .unwrap();
    let resolved = instances
        .resolve(&public, &BTreeMap::new())
        .expect("public");
    assert_eq!(resolved.source, InstanceSource::Public);
    assert_eq!(resolved.instance.authority, InstanceAuthority::Public);
}

#[test]
fn an_injected_grant_wins_and_must_be_complete() {
    let (_root, instances) = instances();
    let slack = slack();
    instances
        .confirm(hosted("slack-acme", &slack), true)
        .expect("confirm");
    let mut env = BTreeMap::from([
        (INJECTED_GRANT_ID_ENV.to_owned(), "grant_hosted".to_owned()),
        (
            INJECTED_CONNECTOR_BINDING_ENV.to_owned(),
            serde_json::to_string(&slack.binding()).unwrap(),
        ),
        (
            INJECTED_GRANTED_SCOPES_ENV.to_owned(),
            encode_injected_scopes(&["channel.post".to_owned()]).unwrap(),
        ),
        (
            INJECTED_PRINCIPAL_REF_ENV.to_owned(),
            "runx:principal:worker".to_owned(),
        ),
    ]);
    let resolved = instances.resolve(&slack, &env).expect("injected");
    assert_eq!(
        resolved.source,
        InstanceSource::Injected {
            granted_scopes: vec!["channel.post".to_owned()]
        }
    );
    assert_eq!(
        resolved.instance.instance_id,
        instance_id(&["injected", slack.connector_ref.as_str()]).unwrap()
    );

    // A hosted grant cannot stand for a connector with no hosted placement.
    let web = ConnectorScope::official_only()
        .unwrap()
        .connector("web")
        .unwrap();
    assert_eq!(
        instances.resolve(&web, &env).expect("public").source,
        InstanceSource::Public
    );

    env.remove(INJECTED_PRINCIPAL_REF_ENV);
    assert!(matches!(
        instances.resolve(&slack, &env),
        Err(InstanceError::InjectedIncomplete(_))
    ));
}

#[test]
fn injected_scopes_travel_as_a_json_array() {
    let scopes = vec!["a,b".to_owned(), "opaque scope".to_owned()];
    let encoded = encode_injected_scopes(&scopes).unwrap();
    assert_eq!(decode_injected_scopes(&encoded), Some(scopes));
    assert_eq!(decode_injected_scopes("repo.read,repo.write"), None);
    assert_eq!(decode_injected_scopes(r#"["repo.read", " "]"#), None);
    assert_eq!(encode_injected_scopes(&[]), None);
}
