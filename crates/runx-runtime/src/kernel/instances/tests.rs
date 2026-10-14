#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeMap;

use runx_contracts::connector::{InstanceId, Placement};
use runx_contracts::connector_instance::ConnectorInstanceSchema;
use runx_contracts::schema::IsoDateTime;
use runx_contracts::{ConnectorInstance, InstanceAuthority};

use super::*;
use crate::kernel::store::KernelStore;

fn instances() -> (tempfile::TempDir, Instances) {
    let root = tempfile::tempdir().expect("tempdir");
    let store = KernelStore::open(root.path()).expect("store");
    (root, Instances::new(store))
}

fn slack() -> std::sync::Arc<LoadedConnector> {
    crate::kernel::connectors::source_scope()
        .connector("slack")
        .expect("slack")
}

fn hosted(id: &str, connector: &LoadedConnector) -> ConnectorInstance {
    ConnectorInstance {
        schema: ConnectorInstanceSchema::V1,
        instance_id: InstanceId::new(id).expect("id"),
        connector: connector.name.clone(),
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
    assert!(matches!(
        instances.resolve(&slack),
        Err(InstanceError::NotSelected { .. })
    ));

    instances
        .confirm(hosted("slack-acme", &slack), true)
        .expect("confirm");
    let resolved = instances.resolve(&slack).expect("resolves");
    assert_eq!(resolved.instance_id.as_str(), "slack-acme");
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
            .resolve(&slack)
            .expect("still selected")
            .instance_id
            .as_str(),
        "slack-acme"
    );

    instances.revoke("slack-acme").expect("revoke");
    assert!(matches!(
        instances.resolve(&slack),
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
    let github = crate::kernel::connectors::source_scope()
        .connector("github")
        .unwrap();
    let mut moved = hosted("shared-id", &github);
    moved.connector = github.name.clone();
    assert!(matches!(
        instances.confirm(moved, false),
        Err(InstanceError::WrongConnector { .. })
    ));
}

#[test]
fn a_public_connector_needs_no_setup() {
    let (_root, instances) = instances();
    let public = crate::kernel::connectors::source_scope()
        .connector("open-meteo")
        .unwrap();
    let resolved = instances.resolve(&public).expect("public");
    assert_eq!(resolved.authority, InstanceAuthority::Public);
}
