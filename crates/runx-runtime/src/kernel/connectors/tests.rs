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

/// The paths of a project whose workspace is `work` and whose project
/// directory is `project`.
fn project(work: &Path, project: &Path) -> RunxPaths {
    RunxPaths::resolve(
        &BTreeMap::from([
            (
                crate::paths::RUNX_CWD_ENV.to_owned(),
                work.to_string_lossy().into_owned(),
            ),
            (
                crate::paths::RUNX_PROJECT_DIR_ENV.to_owned(),
                project.to_string_lossy().into_owned(),
            ),
        ]),
        work,
    )
}

/// Outside a plugin, a connector that is not a runtime capability is a
/// version runx added: an operator reaches its one version by name, and a
/// skill reaches it only by pinning its manifest.
#[test]
fn a_connector_outside_a_plugin_is_an_added_version_a_skill_pins() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    // The project directory holds no connectors/, so the work directory is in no plugin.
    let paths = project(&work, &root.path().join("project"));
    let home = paths.added_connectors();
    let scope = ConnectorScope::in_project(&work, &paths).unwrap();

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
/// and every added version, each content once.
#[test]
fn the_reachable_connectors_include_every_added_version_once() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let paths = project(&work, &root.path().join("project"));
    let home = paths.added_connectors();
    install(&home, "acme/ops", "1.0.0", "First.");
    install(&home, "acme/ops", "1.1.0", "Second.");
    install(&home, "stripe", "1.0.0", "Leaf.");
    let scope = ConnectorScope::in_project(&work, &paths).unwrap();
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

/// The project's own connectors sit beside its source and the versions runx
/// added sit in its project directory, and the project's own answers a
/// requirement whatever version it pins (core-shape §6.5).
#[test]
fn the_projects_own_connector_takes_precedence_over_an_added_version() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("skills/work");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&root.path().join("connectors"), "acme/billing");
    let paths = project(root.path(), &root.path().join(".runx"));
    let home = paths.added_connectors();
    let leaf = load_installed(&install(&home, "stripe", "sha-000000000001", "Leaf.")).unwrap();
    let surface = load_installed(&install(
        &home,
        "acme/billing",
        "sha-000000000002",
        "Installed.",
    ))
    .unwrap();
    let scope = ConnectorScope::in_project(&skill, &paths).unwrap();

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
    // The project's own `acme/billing` wins unpinned, and pinned to the
    // manifest the added version publishes: a local copy composes.
    assert!(matches!(
        scope.connector("acme/billing").unwrap().origin,
        ConnectorOrigin::Plugin { .. }
    ));
    let pinned_to_added = scope
        .requirement("acme/billing", &pinned(&surface.manifest_digest))
        .unwrap();
    assert!(matches!(
        pinned_to_added.origin,
        ConnectorOrigin::Plugin { .. }
    ));
    assert_ne!(pinned_to_added.manifest_digest, surface.manifest_digest);
}

/// A skill runx added sits in the project directory and has no plugin of its
/// own: its plugin is the project's, wherever the project directory is.
#[test]
fn a_skill_runx_added_has_the_projects_plugin() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let workspace = root.join("workspace");
    write_plugin_connector(&workspace.join("connectors"), "acme");
    for project_dir in [workspace.join(".runx"), root.join("elsewhere/project")] {
        let paths = project(&workspace, &project_dir);
        let added = paths.added_skills().join("acme/records/1.0.0");
        std::fs::create_dir_all(&added).unwrap();
        let scope = ConnectorScope::in_project(&added, &paths).unwrap();
        assert_eq!(scope.plugin_root(), Some(workspace.as_path()));
        assert!(matches!(
            scope.connector("acme").unwrap().origin,
            ConnectorOrigin::Plugin { .. }
        ));
    }
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

/// A plugin nothing else marks as a project is its own project, and seeding a
/// runtime capability writes it among the project's added connectors: never
/// beside the source, so never into the user's own connectors and never into
/// a nearer directory that would shadow the plugin.
#[test]
fn seeding_a_runtime_capability_writes_into_the_project_directory() {
    let root = tempfile::tempdir().unwrap();
    let plugin = root.path().canonicalize().unwrap().join("plugin");
    let skill = plugin.join("work");
    std::fs::create_dir_all(&skill).unwrap();
    write_plugin_connector(&plugin.join("connectors"), "acme");
    let paths = crate::paths::RunxPaths::resolve(&BTreeMap::new(), &skill);
    assert_eq!(paths.workspace(), plugin.as_path());

    ConnectorScope::in_project(&skill, &paths)
        .unwrap()
        .connector("artifact")
        .unwrap();

    assert!(paths.added_connectors().join("artifact").is_dir());
    assert!(!plugin.join("connectors/artifact").exists());
    assert!(!skill.join("connectors").exists());
    let again = ConnectorScope::at(&skill).unwrap();
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
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let paths = project(&work, &root.path().join("project"));
    let home = paths.added_connectors();
    install(&home, "acme/ops", "1.0.0", "First.");
    let broken = home.join("acme").join("broken").join("1.0.0");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("connector.yaml"), "not: [a connector").unwrap();
    let scope = ConnectorScope::in_project(&work, &paths).unwrap();

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
fn a_pinned_requirement_resolves_only_the_added_version_publishing_its_manifest() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let paths = project(&work, &root.path().join("project"));
    let home = paths.added_connectors();
    let first = install(&home, "acme/ops", "1.0.0", "First.");
    install(&home, "acme/ops", "1.1.0", "Second.");
    // An install in progress is never read.
    std::fs::create_dir_all(home.join("acme/ops/.staging-1-2")).unwrap();
    let first = load_installed(&first).unwrap();
    let scope = ConnectorScope::in_project(&work, &paths).unwrap();

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

    // The added versions are read only through a scope in the project; the
    // directory holding them is no plugin.
    assert!(matches!(
        ConnectorScope::at(&work)
            .unwrap()
            .requirement("acme/ops", &pinned(&first.manifest_digest)),
        Err(ConnectorRegistryError::NoPlugin { .. })
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
fn an_added_version_of_a_runtime_capability_is_its_own_revision() {
    let root = tempfile::tempdir().unwrap();
    let work = root.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let paths = project(&work, &root.path().join("project"));
    let installed =
        load_installed(&install(&paths.added_connectors(), "web", "9.0.0", "Next.")).unwrap();
    let embedded = ConnectorScope::runtime_capabilities_only()
        .unwrap()
        .connector("web")
        .unwrap();
    assert_ne!(installed.manifest_digest, embedded.manifest_digest);

    let scope = ConnectorScope::in_project(&work, &paths).unwrap();
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
