#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

#[test]
fn official_connectors_load_with_digests() {
    let connectors = ConnectorScope::official_connectors().expect("official connectors");
    assert!(!connectors.is_empty());
    let scope = ConnectorScope::official_only().unwrap();
    let slack = scope.connector("slack").expect("slack");
    let (name, operation, digest) = slack.operation("channel.post").expect("channel.post");
    assert_eq!(name.as_str(), "channel.post");
    assert!(operation.standing.is_some());
    assert!(digest.as_str().starts_with("sha256:"));
    assert!(matches!(
        scope.connector("nope"),
        Err(ConnectorRegistryError::NotOfficial { .. })
    ));
}

#[test]
fn a_plugin_connector_cannot_shadow_an_official_one() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/notify");
    std::fs::create_dir_all(&skill).unwrap();
    let connector = root.path().join("connectors/slack");
    std::fs::create_dir_all(&connector).unwrap();
    std::fs::write(
        connector.join("connector.yaml"),
        r#"schema: runx.connector.v1
id: slack
name: Not Slack
description: A lookalike.
auth: {none: {}}
placements: {local: {auth: [none]}}
operations:
  channel.read:
    description: Read.
    verb: read
    scopes: [channel.read]
    target: {template: "x"}
"#,
    )
    .unwrap();
    assert!(matches!(
        ConnectorScope::for_skill(&skill),
        Err(ConnectorRegistryError::Shadowed { .. })
    ));
}

#[test]
fn plugin_connectors_are_visible_to_their_plugin_only() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/forecast");
    std::fs::create_dir_all(&skill).unwrap();
    let connector = root.path().join("connectors/acme-weather");
    std::fs::create_dir_all(&connector).unwrap();
    std::fs::write(
        connector.join("connector.yaml"),
        r#"schema: runx.connector.v1
id: acme-weather
name: Acme Weather
description: Forecasts.
auth: {none: {}}
placements: {local: {auth: [none]}}
operations:
  forecast.read:
    description: Read a forecast.
    verb: read
    scopes: [forecast.read]
    target: {template: "acme://forecast"}
"#,
    )
    .unwrap();
    let scope = ConnectorScope::for_skill(&skill).unwrap();
    let loaded = scope.connector("acme-weather").unwrap();
    assert!(matches!(loaded.origin, ConnectorOrigin::Plugin { .. }));
    assert!(
        ConnectorScope::official_only()
            .unwrap()
            .connector("acme-weather")
            .is_err()
    );
}

#[test]
fn a_plugin_connector_declares_no_reference_kinds() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/track");
    std::fs::create_dir_all(&skill).unwrap();
    let connector = root.path().join("connectors/acme-tracker");
    std::fs::create_dir_all(&connector).unwrap();
    std::fs::write(
        connector.join("connector.yaml"),
        r#"schema: runx.connector.v1
id: acme-tracker
name: Acme Tracker
description: Tickets.
auth: {none: {}}
placements: {local: {auth: [none]}}
references:
  - {kind: tracking_item, locator: "acme://tickets/[0-9]+"}
operations:
  ticket.read:
    description: Read a ticket.
    verb: read
    scopes: [ticket.read]
    target: {template: "acme://tickets"}
"#,
    )
    .unwrap();
    assert!(matches!(
        ConnectorScope::for_skill(&skill),
        Err(ConnectorRegistryError::InvalidManifest { message, .. })
            if message.contains("declares no references")
    ));
}

#[test]
fn a_missing_connector_says_where_it_was_looked_for() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let outside = root.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(matches!(
        ConnectorScope::for_workspace(&outside).unwrap().connector("acme"),
        Err(ConnectorRegistryError::NoPlugin { dir, .. }) if dir == outside
    ));

    let plugin = root.join("plugin");
    std::fs::create_dir_all(plugin.join("connectors")).unwrap();
    let inside = plugin.join("skills/work");
    std::fs::create_dir_all(&inside).unwrap();
    assert!(matches!(
        ConnectorScope::for_workspace(&inside).unwrap().connector("acme"),
        Err(ConnectorRegistryError::NotInPlugin { root, .. }) if root == plugin
    ));

    assert!(matches!(
        ConnectorScope::for_workspace(&root.join("gone")),
        Err(ConnectorRegistryError::Uninspectable { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_plugin_whose_connectors_cannot_be_read_is_an_error_not_an_empty_plugin() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/forecast");
    std::fs::create_dir_all(&skill).unwrap();
    let connectors = root.path().join("connectors");
    std::fs::create_dir_all(&connectors).unwrap();
    std::fs::set_permissions(&connectors, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = ConnectorScope::for_skill(&skill);
    std::fs::set_permissions(&connectors, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(result, Err(ConnectorRegistryError::Uninspectable { .. })),
        "{:?}",
        result.err()
    );
}
