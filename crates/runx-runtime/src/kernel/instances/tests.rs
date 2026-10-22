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

fn chat() -> std::sync::Arc<LoadedConnector> {
    crate::kernel::connectors::fixture_scope()
        .connector("fixture/chat")
        .expect("fixture/chat")
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

fn no_variables() -> BTreeMap<String, String> {
    BTreeMap::new()
}

fn variables(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

fn fixture(name: &str) -> std::sync::Arc<LoadedConnector> {
    crate::kernel::connectors::fixture_scope()
        .connector(name)
        .expect(name)
}

/// A credential whose variable is set needs no setup: the run is bound to
/// the variable exactly as `runx connector use` would bind it, and nothing
/// is written.
#[test]
fn a_connector_whose_credential_variable_is_set_runs_as_it_with_no_setup() {
    let (_root, instances) = instances();
    let search = fixture("fixture/search");
    let resolved = instances
        .resolve(&search, &variables(&[("SEARCH_API_KEY", "a-key")]))
        .expect("the variable binds");
    assert_eq!(
        resolved.authority,
        InstanceAuthority::Environment {
            credential: runx_contracts::connector::CredentialName::new("api_key").unwrap(),
            variable: EnvName::new("SEARCH_API_KEY").unwrap(),
            scopes: vec!["search.read".into()],
        }
    );
    assert_eq!(
        resolved.instance_id,
        instance_id(&["environment", "fixture/search", "api_key", "SEARCH_API_KEY"]).unwrap(),
        "the instance `connector use` would confirm"
    );
    assert!(
        instances.list().expect("list").is_empty(),
        "nothing is written"
    );
    assert!(!format!("{resolved:?}").contains("a-key"));

    // A blank variable is no variable.
    assert!(matches!(
        instances.resolve(&search, &variables(&[("SEARCH_API_KEY", "  ")])),
        Err(InstanceError::NotSelected { .. })
    ));
}

#[test]
fn a_selection_wins_over_a_set_variable() {
    let (_root, instances) = instances();
    let search = fixture("fixture/search");
    let mut chosen = environment_instance(
        &search,
        &runx_contracts::connector::CredentialName::new("api_key").unwrap(),
        EnvName::new("SEARCH_API_KEY").unwrap(),
        BTreeSet::from(["search.read".to_owned()]),
    )
    .unwrap();
    chosen.instance_id = InstanceId::new("chosen").unwrap();
    instances.confirm(chosen, true).expect("confirm");
    let resolved = instances
        .resolve(&search, &variables(&[("SEARCH_API_KEY", "a-key")]))
        .expect("resolves");
    assert_eq!(resolved.instance_id.as_str(), "chosen");
}

/// Two credentials whose variables are both set could be two accounts: the
/// run refuses rather than choose; one set is taken.
#[test]
fn two_set_credential_variables_refuse_and_one_is_taken() {
    let (_root, instances) = instances();
    let social = fixture("fixture/social");
    let refused = instances
        .resolve(
            &social,
            &variables(&[("SOCIAL_USER_AUTH", "u"), ("SOCIAL_BEARER_TOKEN", "b")]),
        )
        .expect_err("refused");
    assert!(matches!(refused, InstanceError::SeveralVariables { .. }));
    assert!(
        refused
            .to_string()
            .contains("(SOCIAL_BEARER_TOKEN, SOCIAL_USER_AUTH)"),
        "{refused}"
    );

    let resolved = instances
        .resolve(&social, &variables(&[("SOCIAL_BEARER_TOKEN", "b")]))
        .expect("one binds");
    assert!(matches!(
        resolved.authority,
        InstanceAuthority::Environment { ref variable, .. } if variable.as_str() == "SOCIAL_BEARER_TOKEN"
    ));
}

#[test]
fn a_selected_instance_resolves_and_a_revoked_one_refuses() {
    let (_root, instances) = instances();
    let chat = chat();
    assert!(matches!(
        instances.resolve(&chat, &no_variables()),
        Err(InstanceError::NotSelected { .. })
    ));

    instances
        .confirm(hosted("slack-acme", &chat), true)
        .expect("confirm");
    let resolved = instances.resolve(&chat, &no_variables()).expect("resolves");
    assert_eq!(resolved.instance_id.as_str(), "slack-acme");
    assert!(
        instances
            .get("slack-acme")
            .expect("get")
            .expect("record")
            .selected
    );

    instances
        .confirm(hosted("slack-other", &chat), false)
        .expect("confirm");
    assert_eq!(
        instances
            .resolve(&chat, &no_variables())
            .expect("still selected")
            .instance_id
            .as_str(),
        "slack-acme"
    );

    instances.revoke("slack-acme").expect("revoke");
    assert!(matches!(
        instances.resolve(&chat, &no_variables()),
        Err(InstanceError::Revoked { .. })
    ));
    assert_eq!(instances.list().expect("list").len(), 2);
}

#[test]
fn an_instance_id_cannot_move_to_another_connector() {
    let (_root, instances) = instances();
    let chat = chat();
    instances
        .confirm(hosted("shared-id", &chat), false)
        .expect("confirm");
    let forge = crate::kernel::connectors::fixture_scope()
        .connector("fixture/forge")
        .unwrap();
    let mut moved = hosted("shared-id", &forge);
    moved.connector = forge.name.clone();
    assert!(matches!(
        instances.confirm(moved, false),
        Err(InstanceError::WrongConnector { .. })
    ));
}

#[test]
fn a_public_connector_needs_no_setup() {
    let (_root, instances) = instances();
    let public = crate::kernel::connectors::fixture_scope()
        .connector("fixture/weather")
        .unwrap();
    let resolved = instances.resolve(&public, &no_variables()).expect("public");
    assert_eq!(resolved.authority, InstanceAuthority::Public);
}
