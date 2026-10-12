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
    assert_eq!(
        operation.approval,
        runx_contracts::connector::ApprovalFloor::Required
    );
    assert!(digest.as_str().starts_with("sha256:"));
    assert!(matches!(
        scope.connector("nope"),
        Err(ConnectorRegistryError::NotOfficial { .. })
    ));
}

#[test]
fn a_plugin_connector_cannot_shadow_an_official_one() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"runx"}"#).unwrap();
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
runtime_capability: false
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
        ConnectorScope::at(&skill),
        Err(ConnectorRegistryError::Shadowed { .. })
    ));
}

#[test]
fn plugin_connectors_are_visible_to_their_plugin_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
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
runtime_capability: false
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
    let scope = ConnectorScope::at(&skill).unwrap();
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
fn a_plugin_connector_owns_its_reference_kinds() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
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
runtime_capability: false
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
    let loaded = ConnectorScope::at(&skill)
        .unwrap()
        .connector("acme/acme-tracker")
        .unwrap();
    assert_eq!(
        super::references::bound_reference_kind(&loaded.manifest, "acme://tickets/7"),
        runx_contracts::ReferenceType::TrackingItem
    );
}

#[test]
fn a_missing_connector_says_where_it_was_looked_for() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    let root = root.path().canonicalize().unwrap();
    let outside = root.join("elsewhere");
    std::fs::create_dir_all(&outside).unwrap();
    assert!(matches!(
        ConnectorScope::at(&outside).unwrap().connector("acme"),
        Err(ConnectorRegistryError::NoPlugin { dir, .. }) if dir == outside
    ));

    let plugin = root.join("plugin");
    std::fs::create_dir_all(plugin.join("connectors")).unwrap();
    let inside = plugin.join("skills/work");
    std::fs::create_dir_all(&inside).unwrap();
    assert!(matches!(
        ConnectorScope::at(&inside).unwrap().connector("acme"),
        Err(ConnectorRegistryError::NotInPlugin { root, .. }) if root == plugin
    ));

    assert!(matches!(
        ConnectorScope::at(&root.join("gone")),
        Err(ConnectorRegistryError::Uninspectable { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_plugin_whose_connectors_cannot_be_read_is_an_error_not_an_empty_plugin() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    let skill = root.path().join("skills/forecast");
    std::fs::create_dir_all(&skill).unwrap();
    let connectors = root.path().join("connectors");
    std::fs::create_dir_all(&connectors).unwrap();
    std::fs::set_permissions(&connectors, std::fs::Permissions::from_mode(0o000)).unwrap();
    let result = ConnectorScope::at(&skill);
    std::fs::set_permissions(&connectors, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(result, Err(ConnectorRegistryError::Uninspectable { .. })),
        "{:?}",
        result.err()
    );
}

fn write_plugin_connector(connectors: &Path, directory: &str, id: &str) {
    let dir = connectors.join(directory);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("connector.yaml"),
        format!(
            "schema: runx.connector.v1\nid: {id}\nname: Acme\ndescription: Records.\nruntime_capability: false\nauth: {{none: {{}}}}\nplacements: {{local: {{auth: [none]}}}}\noperations:\n  record.read:\n    description: Read a record.\n    verb: read\n    scopes: [record.read]\n    target: {{template: \"acme://records\"}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_bundled_package_sees_only_the_snapshots_its_bundle_binds() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    std::fs::write(root.join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    // A source plugin above the bundle is never reached from inside it.
    write_plugin_connector(&root.join("connectors"), "acme-above", "acme-above");
    let package = root.join("installed");
    let dependency = package.join("dependencies/beta");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(package.join("SKILL.md"), "# Root\n").unwrap();
    std::fs::write(dependency.join("SKILL.md"), "# Beta\n").unwrap();
    write_plugin_connector(&package.join("connectors/acme"), "acme", "acme");
    let snapshot = load_directory(
        &package.join("connectors/acme/acme"),
        ConnectorOrigin::Plugin {
            root: package.clone(),
        },
        &PublisherNamespace::new("acme").unwrap(),
        false,
    )
    .unwrap();
    let descriptor = serde_json::json!({
        "schema": "runx.registry.package_bundle.v1",
        "dependencies": [],
        "connectors": [{
            "from": ".",
            "connector": "acme/acme",
            "manifest_digest": snapshot.manifest_digest,
            "connector_digest": snapshot.connector_digest,
        }],
    });
    std::fs::write(package.join("runx.package.json"), descriptor.to_string()).unwrap();

    let own = ConnectorScope::at(&package).unwrap();
    assert_eq!(
        own.connector("acme/acme").unwrap().connector_digest,
        snapshot.connector_digest
    );
    assert!(matches!(
        own.connector("acme-above"),
        Err(ConnectorRegistryError::Unavailable { .. })
    ));
    // The dependency's own bindings are none; its composer's are not its own.
    assert!(matches!(
        ConnectorScope::at(&dependency)
            .unwrap()
            .connector("acme/acme"),
        Err(ConnectorRegistryError::Unavailable { .. })
    ));
}

#[test]
fn a_plugin_connector_lives_in_the_directory_its_id_names() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    let skill = root.path().join("skills/records");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&root.path().join("connectors"), "acme", "acme-records");
    assert!(matches!(
        ConnectorScope::at(&skill),
        Err(ConnectorRegistryError::InvalidManifest { message, .. })
            if message.contains("lives in connectors/acme-records/")
    ));
}

/// Leave `publisher/id` installed as `version` under `home`, as a registry
/// install does: the package's own `plugin.json` names its publisher.
fn install(home: &Path, publisher: &str, id: &str, version: &str, description: &str) -> PathBuf {
    let directory = home.join(publisher).join(id).join(version);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("plugin.json"),
        format!(r#"{{"namespace":"{publisher}"}}"#),
    )
    .unwrap();
    std::fs::write(
        directory.join("connector.yaml"),
        format!(
            "schema: runx.connector.v1\nid: {id}\nname: Ops\ndescription: {description}\nruntime_capability: false\nauth: {{none: {{}}}}\nplacements: {{local: {{auth: [none]}}}}\noperations:\n  record.read:\n    description: Read a record.\n    verb: read\n    scopes: [record.read]\n    target: {{template: \"ops://records\"}}\n"
        ),
    )
    .unwrap();
    directory
}

fn pinned(digest: &Digest) -> runx_parser::ConnectorRequirement {
    runx_parser::ConnectorRequirement {
        operations: vec![OperationName::new("record.read").unwrap()],
        manifest_digest: Some(digest.clone()),
        connector_from: None,
        allowed_connectors: Vec::new(),
    }
}

#[test]
fn a_pinned_requirement_resolves_only_the_installed_version_publishing_its_manifest() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("connectors");
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let first = install(&home, "acme", "ops-api", "1.0.0", "First.");
    install(&home, "acme", "ops-api", "1.1.0", "Second.");
    // An install in progress is never read.
    std::fs::create_dir_all(home.join("acme/ops-api/.staging-1-2")).unwrap();
    let first = load_installed(&first).unwrap();
    let scope = ConnectorScope::at(&work)
        .unwrap()
        .with_installed(home.clone());

    let resolved = scope
        .requirement("acme/ops-api", &pinned(&first.manifest_digest))
        .unwrap();
    assert_eq!(resolved.manifest_digest, first.manifest_digest);
    assert!(matches!(resolved.origin, ConnectorOrigin::Installed { .. }));

    let unpublished = Digest::new(format!("sha256:{}", "0".repeat(64))).unwrap();
    let refused = scope
        .requirement("acme/ops-api", &pinned(&unpublished))
        .unwrap_err();
    assert!(
        matches!(refused, ConnectorRegistryError::NotInstalled { .. })
            && refused
                .to_string()
                .contains("runx registry connector install acme/ops-api@<version>"),
        "{refused}"
    );

    // The installed versions are read only through a scope given them.
    assert!(matches!(
        ConnectorScope::at(&work)
            .unwrap()
            .requirement("acme/ops-api", &pinned(&first.manifest_digest)),
        Err(ConnectorRegistryError::Unavailable { .. })
    ));

    // An operator names one exact version; two installed versions of a
    // bare ref are not chosen between.
    assert_eq!(
        scope
            .named("acme/ops-api@1.1.0")
            .unwrap()
            .manifest
            .description
            .as_str(),
        "Second."
    );
    assert!(matches!(
        scope.named("acme/ops-api"),
        Err(ConnectorRegistryError::InstalledAmbiguous { versions, .. })
            if versions == ["1.0.0", "1.1.0"]
    ));
    assert!(matches!(
        scope.named("acme/ops-api@2.0.0"),
        Err(ConnectorRegistryError::VersionNotInstalled { .. })
    ));
}

#[test]
fn an_installed_version_of_an_official_connector_is_its_own_revision() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("connectors");
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let installed =
        load_installed(&install(&home, "runx", "open-meteo", "9.0.0", "Next.")).unwrap();
    let official = ConnectorScope::official_only()
        .unwrap()
        .connector("open-meteo")
        .unwrap();
    assert_ne!(installed.manifest_digest, official.manifest_digest);

    let scope = ConnectorScope::at(&work)
        .unwrap()
        .with_installed(home.clone());
    let resolved = scope
        .requirement("runx/open-meteo", &pinned(&installed.manifest_digest))
        .unwrap();
    assert_eq!(resolved.manifest_digest, installed.manifest_digest);
    // A requirement pinned to the embedded revision still binds it.
    assert!(matches!(
        scope
            .requirement("runx/open-meteo", &pinned(&official.manifest_digest))
            .unwrap()
            .origin,
        ConnectorOrigin::Official
    ));
}
