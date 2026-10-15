#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::*;

/// The runtime embeds exactly the connectors that declare themselves runtime
/// capabilities; every other runx-maintained connector is a published package.
#[test]
fn only_the_runtime_capabilities_are_embedded() {
    let connectors = ConnectorScope::runtime_capabilities().expect("runtime capabilities");
    assert_eq!(
        connectors
            .iter()
            .map(|connector| connector.name.as_str())
            .collect::<Vec<_>>(),
        ["artifact", "store", "web"]
    );
    assert!(connectors.iter().all(|connector| {
        connector.manifest.runtime_capability
            && connector.origin == ConnectorOrigin::RuntimeCapability
            && connector.tools.is_empty()
    }));
    let scope = ConnectorScope::runtime_capabilities_only().unwrap();
    let store = scope.connector("store").expect("store");
    let (name, _, digest) = store.operation("events.append").expect("events.append");
    assert_eq!(name.as_str(), "events.append");
    assert!(digest.as_str().starts_with("sha256:"));
    assert!(matches!(
        scope.connector("slack"),
        Err(ConnectorRegistryError::NotRuntimeCapability { .. })
    ));
}

/// Outside a plugin, a connector that is not a runtime capability is an
/// installed version: an operator reaches its one version by name, and a
/// skill reaches it only by pinning its manifest.
#[test]
fn a_connector_outside_a_plugin_is_an_installed_version_a_skill_pins() {
    let root = tempfile::tempdir().unwrap();
    // Not named connectors/, so the work directory is in no plugin.
    let home = root.path().join("installed");
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let scope = ConnectorScope::at(&work).unwrap().in_project(home.clone());

    let missing = scope.connector("github").unwrap_err();
    assert!(
        matches!(missing, ConnectorRegistryError::NoPlugin { .. })
            && missing.to_string().contains("runx registry install github"),
        "{missing}"
    );

    let first =
        load_installed(&install(&home, "github", "sha-000000000001", "Installed.")).unwrap();
    let installed = scope.connector("github").unwrap();
    assert!(matches!(
        installed.origin,
        ConnectorOrigin::Installed { .. }
    ));
    // A skill names it only by its manifest: without a pin, an installed
    // version is out of reach.
    let unpinned = scope
        .requirement(
            "github",
            &runx_parser::ConnectorRequirement {
                operations: vec![OperationName::new("record.read").unwrap()],
                manifest_digest: None,
                connector_from: None,
                allowed_connectors: Vec::new(),
            },
        )
        .unwrap_err();
    assert!(
        matches!(unpinned, ConnectorRegistryError::NoPlugin { .. })
            && unpinned.to_string().contains("pins its manifest_digest"),
        "{unpinned}"
    );
    assert_eq!(
        scope
            .requirement("github", &pinned(&first.manifest_digest))
            .unwrap()
            .connector_digest,
        installed.connector_digest
    );

    install(&home, "github", "sha-000000000002", "Newer.");
    assert!(matches!(
        scope.connector("github"),
        Err(ConnectorRegistryError::InstalledAmbiguous { .. })
    ));
}

/// What a run could bind: the runtime capabilities, the plugin's connectors,
/// and every installed version, each content once.
#[test]
fn the_reachable_connectors_include_every_installed_version_once() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("connectors");
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    install(&home, "acme/ops", "1.0.0", "First.");
    install(&home, "acme/ops", "1.1.0", "Second.");
    install(&home, "stripe", "1.0.0", "Leaf.");
    let scope = ConnectorScope::at(&work).unwrap().in_project(home.clone());
    // Resolving a runtime capability seeds its copy into the project.
    scope.connector("store").unwrap();

    let reachable = scope.reachable().unwrap();
    let names = reachable
        .iter()
        .map(|connector| connector.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        ["artifact", "store", "web", "acme/ops", "acme/ops", "stripe"]
    );
    assert!(
        scope
            .credential_variables()
            .unwrap()
            .contains("RUNX_STORE_PASSWORD")
    );
}

#[test]
fn a_plugin_connector_cannot_shadow_a_runtime_capability() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/notify");
    std::fs::create_dir_all(&skill).unwrap();
    let connector = root.path().join("connectors/store");
    std::fs::create_dir_all(&connector).unwrap();
    std::fs::write(
        connector.join("connector.yaml"),
        r#"schema: runx.connector.v1
id: store
name: Not Store
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
fn files_beside_a_plugins_connectors_are_not_connectors() {
    let root = tempfile::tempdir().unwrap();
    write_plugin_connector(&root.path().join("connectors"), "acme-weather");
    // A lock, a readme and a desktop file beside the connector directories.
    std::fs::write(
        root.path().join("connectors/runx-maintained.lock.json"),
        "{}",
    )
    .unwrap();
    std::fs::write(root.path().join("connectors/README.md"), "# Connectors\n").unwrap();
    std::fs::write(root.path().join("connectors/.DS_Store"), [0u8; 8]).unwrap();
    let scope = ConnectorScope::at(&root.path().join("connectors/acme-weather")).unwrap();
    assert!(matches!(
        scope.connector("acme-weather").unwrap().origin,
        ConnectorOrigin::Plugin { .. }
    ));
}

#[test]
fn plugin_connectors_are_visible_to_their_plugin_only() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/forecast");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&root.path().join("connectors"), "acme-weather");
    let scope = ConnectorScope::at(&skill).unwrap();
    let loaded = scope.connector("acme-weather").unwrap();
    assert!(matches!(loaded.origin, ConnectorOrigin::Plugin { .. }));
    assert!(
        ConnectorScope::runtime_capabilities_only()
            .unwrap()
            .connector("acme-weather")
            .is_err()
    );
}

/// A parent namespace holds surfaces, each at `connectors/<namespace>/<surface>/`.
#[test]
fn a_plugin_reaches_the_surfaces_of_a_parent_namespace_by_name() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/track");
    std::fs::create_dir_all(&skill).unwrap();
    let connector = root.path().join("connectors/acme/tracker");
    std::fs::create_dir_all(&connector).unwrap();
    std::fs::write(
        connector.join("connector.yaml"),
        r#"schema: runx.connector.v1
id: acme/tracker
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
    write_plugin_connector(&root.path().join("connectors"), "acme/billing");
    let scope = ConnectorScope::at(&skill).unwrap();
    let loaded = scope.connector("acme/tracker").unwrap();
    assert_eq!(
        super::references::bound_reference_kind(&loaded.manifest, &[], "acme://tickets/7"),
        runx_contracts::ReferenceType::TrackingItem
    );
    assert_eq!(
        scope.connector("acme/billing").unwrap().name.as_str(),
        "acme/billing"
    );
    // The namespace alone names no connector.
    assert!(matches!(
        scope.connector("acme"),
        Err(ConnectorRegistryError::NotInPlugin { .. })
    ));
}

/// A provider namespace is a connector or a parent of surfaces, never both.
#[test]
fn a_namespace_holding_a_connector_and_surfaces_is_refused() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/work");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&root.path().join("connectors"), "acme");
    write_plugin_connector(&root.path().join("connectors"), "acme/billing");
    assert!(matches!(
        ConnectorScope::at(&skill),
        Err(ConnectorRegistryError::LeafAndParent { namespace, .. }) if namespace == "acme"
    ));
}

/// A project's own connectors and the versions installed into it share one
/// directory; an installed version is never mistaken for a surface.
#[test]
fn installed_versions_and_a_plugins_own_connectors_share_the_directory() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("connectors");
    let skill = root.path().join("skills/work");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&home, "acme/billing");
    let leaf = load_installed(&install(&home, "stripe", "sha-000000000001", "Leaf.")).unwrap();
    let surface = load_installed(&install(
        &home,
        "acme/billing",
        "sha-000000000002",
        "Installed.",
    ))
    .unwrap();
    let scope = ConnectorScope::at(&skill).unwrap().in_project(home.clone());

    assert_eq!(
        scope
            .connectors()
            .unwrap()
            .iter()
            .filter(|connector| matches!(connector.origin, ConnectorOrigin::Plugin { .. }))
            .map(|connector| connector.name.as_str())
            .collect::<Vec<_>>(),
        ["acme/billing"]
    );
    assert_eq!(
        scope
            .requirement("stripe", &pinned(&leaf.manifest_digest))
            .unwrap()
            .manifest_digest,
        leaf.manifest_digest
    );
    // The plugin's own `acme/billing` wins unpinned; the pin reaches the
    // installed revision.
    assert!(matches!(
        scope.connector("acme/billing").unwrap().origin,
        ConnectorOrigin::Plugin { .. }
    ));
    assert!(matches!(
        scope
            .requirement("acme/billing", &pinned(&surface.manifest_digest))
            .unwrap()
            .origin,
        ConnectorOrigin::Installed { .. }
    ));
}

#[test]
fn a_missing_connector_says_where_it_was_looked_for() {
    let root = tempfile::tempdir().unwrap();
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

/// A plugin nothing else marks as a project is its own project, so seeding a
/// runtime capability writes into the plugin's connectors, never into a
/// nearer directory that would shadow the plugin.
#[test]
fn seeding_a_runtime_capability_never_opens_a_nearer_plugin() {
    let root = tempfile::tempdir().unwrap();
    let plugin = root.path().canonicalize().unwrap().join("plugin");
    let skill = plugin.join("work");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&plugin.join("connectors"), "acme");
    let project = crate::paths::RunxPaths::resolve(&BTreeMap::new(), &skill).connectors();
    assert_eq!(project, plugin.join("connectors"));

    ConnectorScope::at(&skill)
        .unwrap()
        .in_project(project)
        .connector("artifact")
        .unwrap();

    let again = ConnectorScope::at(&skill).unwrap();
    assert!(!skill.join("connectors").exists());
    assert_eq!(again.plugin_root(), Some(plugin.as_path()));
    assert!(matches!(
        again.connector("acme").unwrap().origin,
        ConnectorOrigin::Plugin { .. }
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
    let result = ConnectorScope::at(&skill);
    std::fs::set_permissions(&connectors, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(result, Err(ConnectorRegistryError::Uninspectable { .. })),
        "{:?}",
        result.err()
    );
}

/// A plugin connector named `name`, at `connectors/<name>/`.
fn write_plugin_connector(connectors: &Path, name: &str) {
    write_connector(connectors, name, name);
}

fn write_connector(connectors: &Path, directory: &str, name: &str) {
    let dir = connectors.join(directory);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("connector.yaml"),
        format!(
            "schema: runx.connector.v1\nid: {name}\nname: Acme\ndescription: Records.\nruntime_capability: false\nauth: {{none: {{}}}}\nplacements: {{local: {{auth: [none]}}}}\noperations:\n  record.read:\n    description: Read a record.\n    verb: read\n    scopes: [record.read]\n    target: {{template: \"acme://records\"}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn a_bundled_package_sees_only_the_snapshots_its_bundle_binds() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    // A source plugin above the bundle is never reached from inside it.
    write_plugin_connector(&root.join("connectors"), "acme-above");
    let package = root.join("installed");
    let dependency = package.join("dependencies/beta");
    std::fs::create_dir_all(&dependency).unwrap();
    std::fs::write(package.join("SKILL.md"), "# Root\n").unwrap();
    std::fs::write(dependency.join("SKILL.md"), "# Beta\n").unwrap();
    write_plugin_connector(&package.join("connectors"), "acme/records");
    let snapshot = load_directory(
        &package.join("connectors/acme/records"),
        ConnectorOrigin::Plugin {
            root: package.clone(),
        },
        false,
    )
    .unwrap();
    let descriptor = serde_json::json!({
        "schema": "runx.registry.package_bundle.v1",
        "dependencies": [],
        "connectors": [{
            "from": ".",
            "connector": "acme/records",
            "manifest_digest": snapshot.manifest_digest,
            "connector_digest": snapshot.connector_digest,
        }],
    });
    std::fs::write(package.join("runx.package.json"), descriptor.to_string()).unwrap();

    let own = ConnectorScope::at(&package).unwrap();
    assert_eq!(
        own.connector("acme/records").unwrap().connector_digest,
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
            .connector("acme/records"),
        Err(ConnectorRegistryError::Unavailable { .. })
    ));
}

#[test]
fn a_plugin_connector_lives_in_the_directory_its_name_names() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/records");
    std::fs::create_dir_all(&skill).unwrap();
    write_connector(&root.path().join("connectors"), "acme", "acme-records");
    assert!(matches!(
        ConnectorScope::at(&skill),
        Err(ConnectorRegistryError::InvalidManifest { message, .. })
            if message.contains("lives in connectors/acme-records/")
    ));
}

/// A project install that does not load is skipped when the credential
/// variables in reach are read: nothing can bind it, and a skill that only
/// declares ordinary environment must not fail on it.
#[test]
fn credential_variables_skip_an_install_that_does_not_load() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("connectors");
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    install(&home, "acme/ops", "1.0.0", "First.");
    let broken = home.join("acme").join("broken").join("1.0.0");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("connector.yaml"), "not: [a connector").unwrap();
    let scope = ConnectorScope::at(&work).unwrap().in_project(home.clone());

    assert!(scope.reachable().is_err());
    assert!(
        scope
            .credential_variables()
            .unwrap()
            .contains("RUNX_STORE_PASSWORD")
    );
}

/// Leave connector `name` installed as `version` under `home`, as a registry
/// install does: at `<name>/<version>/`.
fn install(home: &Path, name: &str, version: &str, description: &str) -> PathBuf {
    let directory = name
        .split('/')
        .fold(home.to_path_buf(), |path, segment| path.join(segment))
        .join(version);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("connector.yaml"),
        format!(
            "schema: runx.connector.v1\nid: {name}\nname: Ops\ndescription: {description}\nruntime_capability: false\nauth: {{none: {{}}}}\nplacements: {{local: {{auth: [none]}}}}\noperations:\n  record.read:\n    description: Read a record.\n    verb: read\n    scopes: [record.read]\n    target: {{template: \"ops://records\"}}\n"
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
    let first = install(&home, "acme/ops", "1.0.0", "First.");
    install(&home, "acme/ops", "1.1.0", "Second.");
    // An install in progress is never read.
    std::fs::create_dir_all(home.join("acme/ops/.staging-1-2")).unwrap();
    let first = load_installed(&first).unwrap();
    let scope = ConnectorScope::at(&work).unwrap().in_project(home.clone());

    let resolved = scope
        .requirement("acme/ops", &pinned(&first.manifest_digest))
        .unwrap();
    assert_eq!(resolved.manifest_digest, first.manifest_digest);
    assert!(matches!(resolved.origin, ConnectorOrigin::Installed { .. }));

    let unpublished = Digest::new(format!("sha256:{}", "0".repeat(64))).unwrap();
    let refused = scope
        .requirement("acme/ops", &pinned(&unpublished))
        .unwrap_err();
    assert!(
        matches!(refused, ConnectorRegistryError::NotInstalled { .. })
            && refused
                .to_string()
                .contains("runx registry install acme/ops@<version>"),
        "{refused}"
    );

    // The installed versions are read only through a scope given them; the
    // directory holding them is no plugin connector.
    assert!(matches!(
        ConnectorScope::at(&work)
            .unwrap()
            .requirement("acme/ops", &pinned(&first.manifest_digest)),
        Err(ConnectorRegistryError::NotInPlugin { .. })
    ));

    // An operator names one exact version; two installed versions of one
    // name are not chosen between.
    assert_eq!(
        scope
            .named("acme/ops@1.1.0")
            .unwrap()
            .manifest
            .description
            .as_str(),
        "Second."
    );
    assert!(matches!(
        scope.named("acme/ops"),
        Err(ConnectorRegistryError::InstalledAmbiguous { versions, .. })
            if versions == ["1.0.0", "1.1.0"]
    ));
    assert!(matches!(
        scope.named("acme/ops@2.0.0"),
        Err(ConnectorRegistryError::VersionNotInstalled { .. })
    ));
}

#[test]
fn an_installed_version_of_a_runtime_capability_is_its_own_revision() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("connectors");
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let installed = load_installed(&install(&home, "web", "9.0.0", "Next.")).unwrap();
    let embedded = ConnectorScope::runtime_capabilities_only()
        .unwrap()
        .connector("web")
        .unwrap();
    assert_ne!(installed.manifest_digest, embedded.manifest_digest);

    let scope = ConnectorScope::at(&work).unwrap().in_project(home.clone());
    let resolved = scope
        .requirement("web", &pinned(&installed.manifest_digest))
        .unwrap();
    assert_eq!(resolved.manifest_digest, installed.manifest_digest);
    // A requirement pinned to the embedded revision still binds it.
    assert!(matches!(
        scope
            .requirement("web", &pinned(&embedded.manifest_digest))
            .unwrap()
            .origin,
        ConnectorOrigin::RuntimeCapability
    ));
}
