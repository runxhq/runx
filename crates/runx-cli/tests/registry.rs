use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::registry_fixture::{LoopbackRegistry, Publication};

#[test]
fn registry_publish_search_install_json() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-publish");
    let skill_dir = root.join("skill");
    let install_dir = crate::support::added_skills();
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
    )?;
    // A publish records the version its run.yaml states.
    fs::write(
        skill_dir.join("run.yaml"),
        include_str!("../../../fixtures/registry/install/echo/run.yaml").replacen(
            "minimum_runx: 0.12.0\n",
            "version: \"1.0.0\"\nminimum_runx: 0.12.0\n",
            1,
        ),
    )?;
    write_echo_test(&skill_dir)?;
    let registry = LoopbackRegistry::serve()?;

    let publish = registry_command(&registry, &root)?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &publish,
        &["\"action\": \"publish\"", "\"skill_id\": \"acme/echo\""],
    )?;

    let search = registry_command(&registry, &root)?
        .args([
            "registry",
            "search",
            "echo",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &search,
        &["\"action\": \"search\"", "\"skill_id\": \"acme/echo\""],
    )?;

    let install = registry_command(&registry, &root)?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &install,
        &[
            "\"action\": \"install\"",
            "\"skill_id\": \"acme/echo\"",
            "\"status\": \"installed\"",
        ],
    )?;
    assert!(install_dir.join("acme/echo/1.0.0/SKILL.md").exists());

    Ok(())
}

#[test]
fn registry_publish_install_executes_bundled_sibling() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-bundled-sibling");
    let source = root.join("source");
    let skill_dir = source.join("root");
    let sibling_dir = source.join("sibling");
    let leaf_dir = source.join("leaf");
    let registry = LoopbackRegistry::serve()?;
    let install_dir = crate::support::added_skills();
    fs::create_dir_all(skill_dir.join("packets"))?;
    fs::create_dir_all(skill_dir.join("tests"))?;
    fs::create_dir_all(&sibling_dir)?;
    fs::create_dir_all(&leaf_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: bundled-root\ndescription: Registry bundle integration fixture.\n---\n# Bundled root\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"version: "1.0.0"
minimum_runx: 0.12.0
methods:
  default:
    result_from: [sibling]
    steps:
      - id: sibling
        skill: ../sibling
        outputs:
          sibling_packet: {packet: acme/bundled-root.sibling.v1}
        result_as: sibling_packet
"#,
    )?;
    fs::write(
        skill_dir.join("tests/sibling.yaml"),
        "name: sibling\nmethod: default\nexpect:\n  status: sealed\n",
    )?;
    fs::write(
        skill_dir.join("packets/sibling.v1.schema.json"),
        "{\"$id\":\"https://schemas.runx.ai/packages/acme/bundled-root/sibling/v1.json\",\"x-runx-packet-id\":\"acme/bundled-root.sibling.v1\",\"type\":\"object\"}\n",
    )?;
    fs::write(
        sibling_dir.join("SKILL.md"),
        "---\nname: bundled-sibling\ndescription: Bundled dependency fixture.\n---\n# Bundled sibling\n",
    )?;
    fs::write(
        sibling_dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  default:
    result_from: [leaf]
    steps:
      - id: leaf
        skill: ../leaf
"#,
    )?;
    fs::write(
        leaf_dir.join("SKILL.md"),
        "---\nname: bundled-leaf\ndescription: Transitive bundled dependency fixture.\n---\n# Bundled leaf\n",
    )?;
    fs::write(
        leaf_dir.join("run.yaml"),
        r#"minimum_runx: 0.12.0
methods:
  default:
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: installed-leaf
"#,
    )?;

    let package = runx_command()?
        .args([
            "registry",
            "package",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &package,
        &[
            "\"action\": \"package\"",
            "\"path\": \"runx.package.json\"",
            "\"path\": \"dependencies/1/run.yaml\"",
            "\"path\": \"dependencies/2/run.yaml\"",
        ],
    )?;

    let publish = registry_command(&registry, &root)?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;

    let install = registry_command(&registry, &root)?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/bundled-root@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(&install, &["\"status\": \"installed\""])?;

    let installed = install_dir.join("acme").join("bundled-root").join("1.0.0");
    assert!(installed.join("runx.package.json").is_file());
    assert!(installed.join("dependencies/1/run.yaml").is_file());
    assert!(installed.join("dependencies/2/run.yaml").is_file());
    // The installed package binds by its signature, so the run trusts the
    // registry key that signed it, as the operator who installed it does.
    let run = crate::support::isolated_runx_command_with_inherited_cwd("registry-bundle-run")
        .current_dir(&root)
        .env("RUNX_HOME", root.join("home"))
        .envs(registry.env()?)
        .args([
            "skill",
            installed.to_str().ok_or("non-utf8 installed skill dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(
        run.status.code(),
        Some(0),
        "stderr={}\nstdout={}",
        String::from_utf8_lossy(&run.stderr),
        String::from_utf8_lossy(&run.stdout)
    );
    let result = serde_json::from_slice::<serde_json::Value>(&run.stdout)?;
    assert_eq!(result["status"], "sealed");
    assert_eq!(
        result["result"]["sibling_packet"]["data"]["digest_result"]["data"]["digest"],
        "sha256:e225fda0645d7ec6cfdbd8216a0e81604271cb0a900ea4e2989187e3d4ecd3d1"
    );

    Ok(())
}

/// Adding a skill adds the connectors its package carries to the project, so
/// the project can bind their instances by qualified reference.
#[test]
fn adding_a_skill_adds_the_connectors_it_carries() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-carried-connectors");
    let source = root.join("source");
    let skill_dir = source.join("carrier");
    let registry = LoopbackRegistry::serve()?;
    let workspace = root.join("workspace");
    crate::support::plugin_with_fixture_connectors(&source, &["fixture/forge"])?;
    fs::create_dir_all(skill_dir.join("tests"))?;
    fs::create_dir_all(&workspace)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: carrier\ndescription: Carries the fixture/forge connector.\n---\n# Carrier\n",
    )?;
    // A package carries the connectors its steps call, so its `read` method
    // calls fixture/forge's issue.read, as the requirement it declared did.
    fs::write(
        skill_dir.join("run.yaml"),
        r#"version: "1.0.0"
minimum_runx: 0.12.0
default_method: default
methods:
  default:
    description: Digest a fixed value.
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: carried
  read:
    description: Read one issue through fixture/forge.
    inputs:
      repository: {type: string}
      number: {type: integer}
    steps:
      - id: read
        operation: fixture/forge/issue.read
        inputs:
          repository: $input.repository
          number: $input.number
"#,
    )?;
    fs::write(
        skill_dir.join("tests/digest.yaml"),
        "name: digest\nmethod: default\nexpect:\n  status: sealed\n",
    )?;

    let publish = registry_command(&registry, &root)?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;

    let add = registry_command(&registry, &root)?
        .current_dir(&workspace)
        .args([
            "registry",
            "install",
            "acme/carrier@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(&add, &["\"status\": \"installed\""])?;

    let versions = fs::read_dir(crate::support::added_connectors().join("fixture/forge"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert!(versions[0].join("connector.yaml").is_file());

    // The workspace is in no plugin; the carried copy is reached by its ref.
    let inspect = registry_command(&registry, &root)?
        .current_dir(&workspace)
        .args(["connector", "inspect", "fixture/forge", "--json"])
        .output()?;
    assert_success_contains(&inspect, &["\"connector\": \"fixture/forge\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn registry_package_identity_covers_files_not_only_manual_and_profile()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-package-identity");
    fs::create_dir_all(root.join("references"))?;
    fs::write(
        root.join("SKILL.md"),
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
    )?;
    fs::write(
        root.join("run.yaml"),
        include_str!("../../../fixtures/registry/install/echo/run.yaml"),
    )?;
    let package = || -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let result = runx_command()?
            .args([
                "registry",
                "package",
                root.to_str().ok_or("non-utf8 root")?,
                "--json",
            ])
            .output()?;
        assert_eq!(
            result.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        Ok(serde_json::from_slice(&result.stdout)?)
    };
    let first = package()?;
    fs::write(
        root.join("references/context.md"),
        "A relevant operating constraint.\n",
    )?;
    let second = package()?;
    assert_eq!(
        first["registry"]["package"]["markdown"],
        second["registry"]["package"]["markdown"]
    );
    assert_eq!(
        first["registry"]["package"]["profile_document"],
        second["registry"]["package"]["profile_document"]
    );
    assert_ne!(
        first["registry"]["content_digest"],
        second["registry"]["content_digest"]
    );
    let prepared: runx_runtime::registry::RegistryPublishPackageParts =
        serde_json::from_value(second["registry"]["package"].clone())?;
    let installed = temp_root("registry-package-exported");
    fs::create_dir_all(&installed)?;
    fs::write(installed.as_path().join("SKILL.md"), &prepared.markdown)?;
    if let Some(profile) = &prepared.profile_document {
        fs::write(installed.as_path().join("run.yaml"), profile)?;
    }
    for file in &prepared.package_files {
        let target = installed.as_path().join(&file.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, &file.content)?;
    }
    let loaded = runx_runtime::load_validated_skill_package(installed.as_path())?;
    assert_eq!(
        second["registry"]["package_digest"],
        json!(loaded.package.package_digest)
    );
    assert_ne!(
        first["registry"]["package_digest"],
        second["registry"]["package_digest"]
    );
    assert_eq!(
        second["registry"]["content_digest"],
        json!(runx_runtime::registry::skill_content_digest(
            &prepared.markdown,
            prepared.profile_document.as_deref(),
            &prepared.package_files
        )?)
    );
    assert_eq!(
        second["registry"]["content_digest"],
        package()?["registry"]["content_digest"]
    );
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn registry_package_metadata_names_the_declared_connectors()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-package-connectors");
    crate::support::plugin_with_fixture_connectors(
        &root,
        &["fixture/forge", "fixture/hooks", "fixture/automation"],
    )?;
    let skill_dir = root.join("connected");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: connected\ndescription: Declares a fixed and a chosen connector.\n---\n# Connected\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        // A package calls a connector from a step, so `sync`'s steps call
        // what the package declared: forge's two reads, in that order, and
        // the webhook chosen per run.
        r#"minimum_runx: 0.12.0
default_method: default
connectors:
  webhook:
    choose: $input.provider
    from: [fixture/automation, fixture/hooks]
methods:
  default:
    description: Digest a fixed value.
    inputs:
      provider: {type: string}
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: connected
  sync:
    description: List and read forge issues, then deliver a handoff to the webhook chosen for the run.
    inputs:
      provider: {type: string}
    steps:
      - id: list
        operation: fixture/forge/issues.read
        inputs: {repository: acme/app, numbers: [1]}
      - id: read
        operation: fixture/forge/issue.read
        inputs: {repository: acme/app, number: 1}
      - id: deliver
        operation: webhook/handoff.deliver
        idempotency_key: connected-delivery
        inputs:
          webhook_host: acme.hooks.fixture.invalid
          delivery: {event_id: evt-1, handoff_scope: orchestrator.hooks.workflow.invoke, handoff_audience: "hooks:acme", idempotency_key: connected-delivery, payload: {}}
"#,
    )?;

    let package = runx_command()?
        .args([
            "registry",
            "package",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(
        package.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&package.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&package.stdout)?;
    // The package states what it declares, so a registry checks a
    // presentation's references against it: its inputs and child skills.
    assert_eq!(
        output["registry"]["package"]["metadata"]["input_names"],
        json!(["provider"])
    );
    assert_eq!(
        output["registry"]["package"]["metadata"]["skills"],
        json!([])
    );
    // A run-time choice names each connector it may choose. Connectors are
    // sorted, and so are the operations its steps call. Each connector is
    // named by its name and the digests a closure binds it by.
    let connectors = output["registry"]["package"]["metadata"]["connectors"]
        .as_array()
        .ok_or("connectors missing")?;
    let named = connectors
        .iter()
        .map(|connector| {
            json!({
                "connector": connector["connector"],
                "runtime_capability": connector["runtime_capability"],
                "operations": connector["operations"],
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        named,
        [
            json!({"connector": "fixture/automation", "runtime_capability": false, "operations": ["handoff.deliver"]}),
            json!({"connector": "fixture/forge", "runtime_capability": false, "operations": ["issue.read", "issues.read"]}),
            json!({"connector": "fixture/hooks", "runtime_capability": false, "operations": ["handoff.deliver"]}),
        ]
    );
    for connector in connectors {
        for digest in ["manifest_digest", "connector_digest"] {
            assert!(
                connector[digest]
                    .as_str()
                    .is_some_and(|value| value.starts_with("sha256:") && value.len() == 71),
                "{digest} of {connector}"
            );
        }
        assert_ne!(connector["manifest_digest"], connector["connector_digest"]);
    }
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn registry_connector_package_projects_a_connector_on_its_own()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-connector-package");
    let connector = root.join("acme-ops");
    fs::create_dir_all(&connector)?;
    fs::write(
        connector.join("connector.yaml"),
        "schema: runx.connector.v1\nname: acme/ops\ntitle: Acme Ops\ndescription: Reads Acme items.\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations:\n  item.read:\n    description: Read an item.\n    verb: read\n    scopes: [item.read]\n    target: {template: \"acme://items\"}\n",
    )?;
    fs::write(connector.join("README.md"), "# Acme Ops\n")?;

    let package = runx_command()?
        .args([
            "registry",
            "package",
            connector.to_str().ok_or("non-utf8 connector dir")?,
            "--json",
        ])
        .output()?;
    assert_eq!(
        package.status.code(),
        Some(0),
        "stderr={}",
        String::from_utf8_lossy(&package.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&package.stdout)?;
    let packaged = &output["registry"]["connector"];
    assert_eq!(output["registry"]["action"], "connector_package");
    assert_eq!(packaged["connector"], "acme/ops");
    assert_eq!(packaged["manifest"]["name"], "acme/ops");
    // The connector names itself, so its package carries no namespace file.
    let files = packaged["files"].as_array().ok_or("files missing")?;
    assert_eq!(
        files
            .iter()
            .map(|file| file["path"].clone())
            .collect::<Vec<_>>(),
        [json!("README.md"), json!("connector.yaml")]
    );
    assert!(
        packaged["digests"]["item.read"]["operation"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );

    let refused = runx_command()?
        .args([
            "registry",
            "package",
            connector.to_str().ok_or("non-utf8 connector dir")?,
            "--publisher",
            "acme",
        ])
        .output()?;
    assert_ne!(refused.status.code(), Some(0));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn registry_connector_install_verifies_and_caches_a_published_version()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-connector-install");
    let connector = root.join("acme-ops");
    fs::create_dir_all(&connector)?;
    fs::write(
        connector.join("connector.yaml"),
        "schema: runx.connector.v1\nname: acme/ops\ntitle: Acme Ops\ndescription: Reads Acme items.\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations:\n  item.read:\n    description: Read an item.\n    verb: read\n    scopes: [item.read]\n    target: {template: \"acme://items\"}\n",
    )?;
    fs::write(connector.join("README.md"), "# Acme Ops\n")?;
    // No skill holds the name, so `registry install` installs the connector.
    let registry = LoopbackRegistry::serve()?;
    let published =
        registry.publish_connector(&connector, "1.0.0", runx_contracts::TrustTier::Community)?;
    let content_digest = published["content_digest"]
        .as_str()
        .ok_or("the registry states the content digest")?
        .to_owned();
    let home = root.join("home");
    // An installed connector is added to the project's added connectors.
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    let connector_command =
        |args: &[&str]| -> Result<std::process::Output, Box<dyn std::error::Error>> {
            Ok(runx_command()?
                .envs(registry.env()?)
                .env("RUNX_HOME", &home)
                .env("RUNX_CWD", &project)
                .args(["registry"])
                .args(args)
                .output()?)
        };

    let install =
        connector_command(&["install", "acme/ops", "--registry", &registry.url, "--json"])?;
    assert_success_contains(
        &install,
        &[
            "\"action\": \"connector_install\"",
            "\"status\": \"installed\"",
        ],
    )?;
    let destination = crate::support::added_connectors().join("acme/ops/1.0.0");
    let output: serde_json::Value = serde_json::from_slice(&install.stdout)?;
    assert_eq!(
        output["registry"]["install"]["destination"],
        json!(destination.to_str().ok_or("non-utf8 destination")?)
    );
    assert_eq!(
        output["registry"]["install"]["content_digest"],
        json!(content_digest)
    );
    assert_eq!(
        fs::read_to_string(destination.join("README.md"))?,
        "# Acme Ops\n"
    );

    let again = connector_command(&[
        "install",
        "acme/ops@1.0.0",
        "--registry",
        &registry.url,
        "--digest",
        &content_digest,
        "--json",
    ])?;
    assert_success_contains(&again, &["\"status\": \"unchanged\""])?;

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn a_root_name_installs_the_canonical_skill_it_addresses() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-root-name");
    let registry = LoopbackRegistry::serve()?;
    registry.publish(Publication::new(
        "acme",
        "1.0.0",
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
        Some(include_str!(
            "../../../fixtures/registry/install/echo/run.yaml"
        )),
    ))?;
    // The registry resolves the root name `echo` to acme's skill.
    registry.root("echo", "acme/echo");
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    let install = |subject: &str| -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(runx_command()?
            .envs(registry.env()?)
            .env("RUNX_HOME", root.join("home"))
            .env("RUNX_CWD", &project)
            .args(["registry", "install", subject, "--registry", &registry.url])
            .arg("--json")
            .output()?)
    };

    let installed = install("echo")?;
    assert_success_contains(&installed, &["\"action\": \"install\""])?;
    // runx adds it to the project directory, never beside the source.
    assert!(
        crate::support::added_skills()
            .join("acme/echo/1.0.0/SKILL.md")
            .is_file()
    );
    assert!(!project.join("skills").exists());

    // A name no skill holds as a root name is a connector's to answer, and
    // never a search of every publisher for a skill of that name.
    let unheld = install("ghost")?;
    assert!(!unheld.status.success());
    assert!(String::from_utf8_lossy(&unheld.stdout).contains("ghost"));

    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn registry_install_versions_are_side_by_side() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-side-by-side");
    let install_dir = crate::support::added_skills();
    let registry = LoopbackRegistry::serve()?;
    publish_echo(&registry, "1.0.0", "Echo")?;
    publish_echo(&registry, "2.0.0", "Echo v2")?;

    for subject in ["acme/echo@1.0.0", "acme/echo@2.0.0"] {
        let install = registry_command(&registry, &root)?
            .env("RUNX_CWD", &root)
            .args([
                "registry",
                "install",
                subject,
                "--registry",
                &registry.url,
                "--json",
            ])
            .output()?;
        assert_success_contains(&install, &["\"action\": \"install\""])?;
    }

    let installed = |version: &str| install_dir.join("acme/echo").join(version).join("SKILL.md");
    assert!(installed("1.0.0").exists());
    assert!(installed("2.0.0").exists());
    assert_ne!(
        fs::read_to_string(installed("1.0.0"))?,
        fs::read_to_string(installed("2.0.0"))?
    );

    Ok(())
}

#[test]
fn registry_install_reports_typed_signature_errors() -> Result<(), Box<dyn std::error::Error>> {
    type VersionMutator = fn(&mut serde_json::Value);
    let cases: [(&str, VersionMutator); 3] = [
        ("unknown_key", |version| {
            version["signed_manifest"]["signer"]["key_id"] = json!("unknown-key");
        }),
        ("invalid_signature", |version| {
            version["signed_manifest"]["signature"]["value"] = json!("base64:invalid");
        }),
        ("digest_mismatch", |version| {
            version["markdown"] =
                json!("---\nname: echo\ndescription: echo fixture.\n---\n# Tampered\n");
        }),
    ];

    for (error_kind, mutate) in cases {
        let root = temp_root(&format!("registry-{error_kind}"));
        let registry = LoopbackRegistry::serve()?;
        publish_echo(&registry, "1.0.0", "Echo")?;
        registry.tamper("acme/echo", "1.0.0", mutate)?;
        let install_dir = crate::support::added_skills();

        let install = registry_command(&registry, &root)?
            .env("RUNX_CWD", &root)
            .args([
                "registry",
                "install",
                "acme/echo@1.0.0",
                "--registry",
                &registry.url,
                "--json",
            ])
            .output()?;

        assert_json_failure_contains(&install, &format!("registry install {error_kind}:"))?;
        assert!(
            !install_dir.exists(),
            "{error_kind} should leave no install dir"
        );
    }

    Ok(())
}

/// A registry is a hosted service, named by its URL: a directory, named by
/// --registry or RUNX_REGISTRY_URL, is a usage error before anything is
/// read.
#[test]
fn a_registry_that_is_not_a_url_is_a_usage_error() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-not-a-url");
    fs::create_dir_all(root.join("registry"))?;
    let named = runx_command()?
        .current_dir(&root)
        .args([
            "registry",
            "search",
            "echo",
            "--registry",
            "registry",
            "--json",
        ])
        .output()?;
    assert_json_failure_contains(&named, "--registry names a registry by its URL")?;
    assert_eq!(named.status.code(), Some(64));

    let configured = runx_command()?
        .current_dir(&root)
        .env("RUNX_REGISTRY_URL", root.join("registry"))
        .args(["registry", "install", "acme/echo@1.0.0", "--json"])
        .output()?;
    assert_json_failure_contains(&configured, "RUNX_REGISTRY_URL names a registry by its URL")?;
    assert_eq!(configured.status.code(), Some(64));
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn registry_json_parse_failure_uses_failure_envelope() -> Result<(), Box<dyn std::error::Error>> {
    let output = runx_command()?
        .args(["registry", "search", "--json"])
        .output()?;

    assert_json_failure_contains(&output, "runx registry search requires a query")?;
    assert_eq!(output.status.code(), Some(64));
    Ok(())
}

#[test]
fn registry_human_output_names_selected_version_and_digest()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-human");
    let registry = LoopbackRegistry::serve()?;
    publish_echo(&registry, "1.0.0", "Echo")?;
    publish_echo(&registry, "2.0.0", "Echo v2")?;

    let install_v1 = registry_command(&registry, &root)?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            &registry.url,
        ])
        .output()?;
    assert_success_contains(
        &install_v1,
        &[
            "registry install acme/echo@1.0.0",
            "source           remote",
            "status           installed",
            "version          1.0.0",
            "digest           sha256:",
            &format!("signed           yes ({})", crate::registry_fixture::KEY_ID),
            "destination      ",
        ],
    )?;

    Ok(())
}

#[test]
fn registry_human_output_names_search_results() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-search-human");
    let registry = LoopbackRegistry::serve()?;
    publish_echo(&registry, "1.0.0", "Echo")?;

    let search = registry_command(&registry, &root)?
        .args(["registry", "search", "echo", "--registry", &registry.url])
        .output()?;
    assert_success_contains(
        &search,
        &[
            "registry search  echo",
            "source           remote",
            "results          1",
            "- acme/echo@1.0.0",
            "digest   sha256:",
            "trust    community",
            "install  runx registry install acme/echo@1.0.0",
            "run      runx skill acme/echo@1.0.0",
        ],
    )?;

    Ok(())
}

/// An operator holding the registry's publish credential publishes through
/// the operator route, into the namespace it names with --publisher and only
/// that, and what it publishes installs as the registry signed it.
#[test]
fn an_operator_publish_goes_into_the_namespace_it_names() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-operator-publish");
    let skill_dir = root.join("skill");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        include_str!("../../../fixtures/registry/install/echo/run.yaml"),
    )?;
    write_echo_test(&skill_dir)?;
    let registry = LoopbackRegistry::serve()?;
    let skill = skill_dir.to_str().ok_or("non-utf8 skill dir")?;
    let operator = |args: &[&str]| -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(runx_command()?
            .envs(registry.env_trusting("partner")?)
            .env("RUNX_HOME", root.join("home"))
            .env("RUNX_CWD", &root)
            .env("RUNX_HOSTED_REGISTRY_PUBLISH_TOKEN", "operator-token")
            .args(args)
            .output()?)
    };

    let unnamed = operator(&[
        "registry",
        "publish",
        skill,
        "--registry",
        &registry.url,
        "--json",
    ])?;
    assert_json_failure_contains(
        &unnamed,
        "names the namespace it publishes into with --publisher",
    )?;

    let published = operator(&[
        "registry",
        "publish",
        skill,
        "--registry",
        &registry.url,
        "--publisher",
        "partner",
        "--json",
    ])?;
    assert_success_contains(&published, &["\"skill_id\": \"partner/echo\""])?;
    let output: serde_json::Value = serde_json::from_slice(&published.stdout)?;
    let version = output["registry"]["publish"]["version"]
        .as_str()
        .ok_or("the publish names its version")?
        .to_owned();
    let asked = registry.asked();
    assert!(
        asked
            .iter()
            .any(|line| line.starts_with("POST /v1/admin/registry/publish ")),
        "{asked:?}"
    );
    assert!(
        asked
            .iter()
            .all(|line| !line.starts_with("POST /v1/skills ")),
        "{asked:?}"
    );

    let install = operator(&[
        "registry",
        "install",
        &format!("partner/echo@{version}"),
        "--registry",
        &registry.url,
        "--json",
    ])?;
    assert_success_contains(&install, &["\"status\": \"installed\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// What `runx registry install --records` with `args` prints, given the
/// signed version records `records` on stdin or in a file in `root`, the
/// workspace, as `--records` reads them.
fn install_records(
    registry: &LoopbackRegistry,
    root: &std::path::Path,
    records: &serde_json::Value,
    through_stdin: bool,
    env: &[(&str, &str)],
    args: &[&str],
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    use std::io::Write;

    let mut command = runx_command()?;
    command
        .env("RUNX_HOME", root.join("home"))
        .env("RUNX_CWD", root)
        .envs(env.iter().copied())
        .args(["registry", "install", "--registry", &registry.url, "--json"])
        .args(args);
    if !through_stdin {
        fs::create_dir_all(root)?;
        fs::write(root.join("records.json"), serde_json::to_vec(records)?)?;
        return Ok(command.args(["--records", "records.json"]).output()?);
    }
    let mut child = command
        .args(["--records", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("the install's stdin")?
        .write_all(&serde_json::to_vec(records)?)?;
    Ok(child.wait_with_output()?)
}

/// The environment that trusts `registry`'s key, as name and value pairs.
fn trust_env(
    registry: &LoopbackRegistry,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    Ok(registry.env()?.into_iter().collect())
}

/// Signed version records a host already holds, as Cloud holds a closure's,
/// install verified, under the keys trusted for the registry --registry
/// names, through a file or stdin; a run then reads the installed copy and
/// asks the registry nothing.
#[test]
fn records_a_host_holds_install_verified_and_a_run_reads_them()
-> Result<(), Box<dyn std::error::Error>> {
    let registry = LoopbackRegistry::serve()?;
    let record = registry.publish(Publication::new(
        "acme",
        "1.0.0",
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
        Some(include_str!(
            "../../../fixtures/registry/install/echo/run.yaml"
        )),
    ))?;
    let trust = trust_env(&registry)?;
    let trust = trust
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    for through_stdin in [false, true] {
        let root = temp_root("registry-install-records");
        fs::create_dir_all(&root)?;
        let installed = install_records(
            &registry,
            &root,
            &json!([record]),
            through_stdin,
            &trust,
            &[],
        )?;
        assert_success_contains(&installed, &["\"action\": \"record_install\""])?;
        let output: serde_json::Value = serde_json::from_slice(&installed.stdout)?;
        let install = &output["registry"]["installs"][0]["install"];
        assert_eq!(
            output["registry"]["installs"][0]["package"],
            "acme/echo@1.0.0"
        );
        assert_eq!(install["skill_id"], "acme/echo");
        assert_eq!(install["version"], "1.0.0");
        assert_eq!(install["trust_tier"], "community");
        assert!(
            install["digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{install}"
        );
        assert!(
            install["profile_digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{install}"
        );
        let destination = crate::support::added_skills().join("acme/echo/1.0.0/SKILL.md");
        assert_eq!(
            install["destination"],
            json!(destination.to_str().ok_or("non-utf8 destination")?)
        );
        assert!(
            ["installed", "unchanged"].contains(&install["status"].as_str().unwrap_or_default()),
            "{install}"
        );
        fs::remove_dir_all(&root)?;
    }

    // A run reads the installed copy: the registry is asked nothing.
    let root = temp_root("registry-records-run");
    let parent = root.join("parent");
    fs::create_dir_all(&parent)?;
    fs::write(
        parent.join("SKILL.md"),
        "---\nname: parent\ndescription: Runs the echo registry skill as a step.\n---\n# Parent\n",
    )?;
    fs::write(
        parent.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: echo\n        skill: registry:acme/echo@1.0.0\n",
    )?;
    let asked = registry.asked().len();
    let run = runx_command()?
        .envs(trust.iter().copied())
        .env("RUNX_HOME", root.join("home"))
        .env("RUNX_CWD", &root)
        .args([
            "skill",
            parent.to_str().ok_or("non-utf8 parent dir")?,
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_eq!(
        run.status.code(),
        Some(2),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(registry.asked().len(), asked, "{:?}", registry.asked());
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A record that does not verify installs nothing: tampered files, a key
/// this runx does not trust for the registry named, an empty array, and a
/// ref beside the records are each refused.
#[test]
fn records_that_do_not_verify_install_nothing() -> Result<(), Box<dyn std::error::Error>> {
    let registry = LoopbackRegistry::serve()?;
    let record = registry.publish(Publication::new(
        "acme",
        "1.0.0",
        include_str!("../../../fixtures/registry/install/echo/SKILL.md"),
        Some(include_str!(
            "../../../fixtures/registry/install/echo/run.yaml"
        )),
    ))?;
    let trust = trust_env(&registry)?;
    let trust = trust
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let mut tampered = record.clone();
    tampered["markdown"] = json!("---\nname: echo\ndescription: Echo fixture.\n---\n# Tampered\n");
    let root = temp_root("registry-records-refused");
    fs::create_dir_all(&root)?;
    let installed = || crate::support::added_skills().join("acme/echo").exists();

    let refused = install_records(&registry, &root, &json!([tampered]), true, &trust, &[])?;
    assert_json_failure_contains(&refused, "registry install digest_mismatch:")?;
    assert!(!installed());

    // No key trusted for this registry signs it.
    let untrusted = install_records(&registry, &root, &json!([record]), true, &[], &[])?;
    assert_json_failure_contains(&untrusted, "registry install unknown_key:")?;
    assert!(!installed());

    let empty = install_records(&registry, &root, &json!([]), false, &trust, &[])?;
    assert_json_failure_contains(&empty, "the array is empty")?;
    assert_eq!(empty.status.code(), Some(64));

    let with_ref = install_records(
        &registry,
        &root,
        &json!([record]),
        false,
        &trust,
        &["acme/echo@1.0.0"],
    )?;
    assert_json_failure_contains(&with_ref, "--records takes no ref")?;
    assert_eq!(with_ref.status.code(), Some(64));
    assert!(!installed());
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// Publishes the echo fixture as acme's at `version`, its manual titled
/// `title`.
fn publish_echo(
    registry: &LoopbackRegistry,
    version: &str,
    title: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let markdown = include_str!("../../../fixtures/registry/install/echo/SKILL.md")
        .replace("# Echo", &format!("# {title}"));
    registry.publish(Publication::new(
        "acme",
        version,
        &markdown,
        Some(include_str!(
            "../../../fixtures/registry/install/echo/run.yaml"
        )),
    ))?;
    Ok(())
}

/// `runx` reaching `registry` as its publisher, trusting its key, with its
/// own home under `root`.
/// The echo fixture's one test, which a publish runs, beside its manual and
/// run.yaml in `skill_dir`.
fn write_echo_test(skill_dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(skill_dir.join("tests"))?;
    fs::write(
        skill_dir.join("tests/agent-boundary.yaml"),
        include_str!("../../../fixtures/registry/install/echo/tests/agent-boundary.yaml"),
    )?;
    Ok(())
}

fn registry_command(
    registry: &LoopbackRegistry,
    root: &std::path::Path,
) -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = runx_command()?;
    command
        .envs(registry.publisher_env()?)
        .env("RUNX_HOME", root.join("home"));
    Ok(command)
}

/// Makes `root` the hosted-records plugin: one hosted connector,
/// `fixture/records`, and the `records` skill that reads it.
fn hosted_records(
    root: &std::path::Path,
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    crate::support::copy_dir(
        &crate::support::repo_root()?.join("fixtures/journeys/hosted-records"),
        root,
    )?;
    Ok(root.join("skills/records"))
}

/// The one provider answer every test below reads through, a test's
/// `providers`: the fixture grant, and an empty `records.list`.
fn records_providers() -> String {
    [
        "providers:",
        "  hosted:",
        "    principal_id: operator-fixture",
        "    grant:",
        "      grant_id: records-fixture",
        "      provider: fixture/records",
        "      scopes: [records.read]",
        "    operations:",
        "      - operation: records.list",
        "        target: \"fixture-records://records\"",
        "        access: read",
        "        result:",
        "          records: []",
    ]
    .iter()
    .map(|line| format!("{line}\n"))
    .collect()
}

/// Publish the package at `skill_dir` to `registry` as its publisher, from
/// the directory above it, into `publisher`'s namespace when named.
fn publish(
    skill_dir: &std::path::Path,
    registry: &LoopbackRegistry,
    publisher: Option<&str>,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    let parent = skill_dir.parent().ok_or("skill has no parent")?;
    let mut command = registry_command(registry, &temp_root("registry-publisher"))?;
    command.current_dir(parent).args([
        "registry",
        "publish",
        skill_dir.to_str().ok_or("non-utf8 skill dir")?,
        "--registry",
        &registry.url,
    ]);
    if let Some(publisher) = publisher {
        command.args(["--publisher", publisher]);
    }
    Ok(command.arg("--json").output()?)
}

/// A publish runs the test on the package as the registry holds it: a
/// bundle that is its own plugin, its cases reaching the connectors it
/// carries.
#[test]
fn a_skill_whose_test_grants_a_plugin_connector_publishes() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-publish-plugin-test");
    let skill_dir = hosted_records(&root)?;
    let published = publish(&skill_dir, &LoopbackRegistry::serve()?, None)?;
    assert_success_contains(&published, &["\"action\": \"publish\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A case runs its whole closure, so its provider grant reaches a connector
/// the package's composed skill carries, not only its own.
#[test]
fn a_case_grants_a_connector_its_composed_skill_carries() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-publish-composed-grant");
    hosted_records(&root)?;
    // A skill that binds no connector itself: its case reaches the records
    // grant only through the skill it composes.
    let skill_dir = root.join("skills/records-digest");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: records-digest\ndescription: Digest the records a composed skill lists.\n---\n# Records digest\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  digest:\n    steps:\n      - id: records\n        skill: ../records#list\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: $steps.records\n",
    )?;
    fs::create_dir_all(skill_dir.join("tests"))?;
    fs::write(
        skill_dir.join("tests/digest-reaches-the-composed-records-grant.yaml"),
        format!(
            "name: digest-reaches-the-composed-records-grant\n{}expect:\n  status: sealed\n  steps: [records, digest]\n",
            records_providers(),
        ),
    )?;
    let published = publish(&skill_dir, &LoopbackRegistry::serve()?, None)?;
    assert_success_contains(&published, &["\"action\": \"publish\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// Publishes the records skill as acme's at each version, each carrying its
/// own revision of the fixture/records connector.
fn publish_records_revisions(
    root: &std::path::Path,
    versions: &[&str],
) -> Result<LoopbackRegistry, Box<dyn std::error::Error>> {
    let skill_dir = hosted_records(&root.join("source"))?;
    let manifest = root.join("source/connectors/fixture/records/connector.yaml");
    let profile = skill_dir.join("run.yaml");
    let registry = LoopbackRegistry::serve()?;
    let original = fs::read_to_string(&manifest)?;
    let original_profile = fs::read_to_string(&profile)?;
    for version in versions {
        fs::write(
            &manifest,
            original.replacen(
                "description: A hosted fixture connector",
                &format!("description: Revision {version} of a hosted fixture connector"),
                1,
            ),
        )?;
        fs::write(
            &profile,
            original_profile.replacen("version: \"0.1.0\"", &format!("version: \"{version}\""), 1),
        )?;
        let published = publish(&skill_dir, &registry, Some("acme"))?;
        assert_success_contains(&published, &["\"action\": \"publish\""])?;
    }
    Ok(registry)
}

/// A package in no plugin whose one test lists records through each named
/// version of acme's records skill, granting fixture/records.
fn records_consumer(
    root: &std::path::Path,
    versions: &[&str],
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let consumer = root.join("consumer/ledger");
    fs::create_dir_all(consumer.join("tests"))?;
    fs::write(
        consumer.join("SKILL.md"),
        "---\nname: ledger\ndescription: Lists records through a published records skill.\n---\n# Ledger\n",
    )?;
    let steps = versions
        .iter()
        .enumerate()
        .map(|(index, version)| {
            format!(
                "      - id: records-{index}\n        skill: registry:acme/records@{version}#list\n"
            )
        })
        .collect::<String>();
    fs::write(
        consumer.join("run.yaml"),
        format!(
            "version: \"1.0.0\"\nminimum_runx: 0.12.0\nmethods:\n  default:\n    result_from: [records-0]\n    steps:\n{steps}"
        ),
    )?;
    fs::write(
        consumer.join("tests/reads-through-registry-child.yaml"),
        format!(
            "name: ledger-reads-through-registry-child\nmethod: default\n{}expect:\n  status: sealed\n",
            records_providers()
        ),
    )?;
    Ok(consumer)
}

/// A case's provider grant reaches a connector only a registry child carries:
/// the case's package names the child, the child's bundle carries the
/// connector, and the case's run binds it from there.
#[test]
fn a_case_grants_a_connector_its_registry_child_carries() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-child-grant");
    let registry = publish_records_revisions(&root, &["1.0.0"])?;
    let consumer = records_consumer(&root, &["1.0.0"])?;
    let published = publish(&consumer, &registry, Some("consumer"))?;
    assert_success_contains(&published, &["\"action\": \"publish\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A closure binds one revision of a name (S15): a package whose registry
/// children carry two manifests of the same connector is refused at
/// publication, never settled by whichever child was found first.
#[test]
fn a_case_grant_refuses_a_connector_its_closure_carries_twice()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-child-grant-ambiguous");
    let registry = publish_records_revisions(&root, &["1.0.0", "2.0.0"])?;
    let consumer = records_consumer(&root, &["1.0.0", "2.0.0"])?;
    let published = publish(&consumer, &registry, Some("consumer"))?;
    assert_ne!(published.status.code(), Some(0));
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&published.stdout),
        String::from_utf8_lossy(&published.stderr)
    );
    assert!(
        output.contains("the closure reaches two revisions of connector fixture/records"),
        "{output}"
    );
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A package's tests run each case in a project of its own, which holds the
/// registry children the package's project holds: a case reaches a child the
/// project installed, as a run in that project does.
#[test]
fn a_packages_tests_read_the_registry_children_its_project_holds()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-test-children");
    let registry = LoopbackRegistry::serve()?;
    publish_echo(&registry, "1.0.0", "Echo")?;
    let parent = root.join("parent");
    fs::create_dir_all(parent.join("tests"))?;
    fs::write(
        parent.join("SKILL.md"),
        "---\nname: parent\ndescription: Runs the echo registry skill as a step.\n---\n# Parent\n",
    )?;
    fs::write(
        parent.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  default:\n    steps:\n      - id: echo\n        skill: registry:acme/echo@1.0.0\n",
    )?;
    fs::write(
        parent.join("tests/reaches-the-child.yaml"),
        "name: reaches-the-child\nmethod: default\nexpect:\n  status: needs_agent\n",
    )?;
    let test = || -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(registry_command(&registry, &root)?
            .env("RUNX_CWD", &root)
            .args([
                "test",
                parent.to_str().ok_or("non-utf8 parent dir")?,
                "--json",
            ])
            .output()?)
    };

    let unheld = test()?;
    assert!(
        String::from_utf8_lossy(&unheld.stdout).contains("is not installed in this project"),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&unheld.stdout),
        String::from_utf8_lossy(&unheld.stderr)
    );

    let install = registry_command(&registry, &root)?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            &registry.url,
            "--json",
        ])
        .output()?;
    assert_success_contains(&install, &["\"status\": \"installed\""])?;

    let held = test()?;
    assert_success_contains(&held, &["\"status\": \"passed\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A test of a fixture package runs its composed skill's operations under
/// the provider grant its providers answer for, bound from the fixture
/// package's directory.
#[test]
fn a_workflow_case_binds_the_grant_its_composed_skill_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("test-workflow-binding");
    let skill_dir = hosted_records(&root)?;
    let live = skill_dir.join("fixtures/live");
    fs::create_dir_all(&live)?;
    fs::write(
        live.join("SKILL.md"),
        "---\nname: records-live\ndescription: A test fixture that composes records with the packages it calls.\n---\n\nUsed only by records's tests.\n",
    )?;
    fs::write(
        live.join("run.yaml"),
        "minimum_runx: 0.12.0\ncatalog:\n  visibility: internal\nmethods:\n  run:\n    steps:\n      - id: records\n        skill: ../..#list\n",
    )?;
    let case = skill_dir.join("tests/workflow-binds-grant.yaml");
    fs::write(
        &case,
        format!(
            "name: records-workflow-binds-grant\npackage: fixtures/live\n{}expect:\n  status: sealed\n",
            records_providers()
        ),
    )?;
    let run = runx_command()?
        .current_dir(&root)
        .args(["test", case.to_str().ok_or("non-utf8 case")?, "--json"])
        .output()?;
    assert_eq!(
        run.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    fs::remove_dir_all(&root)?;
    Ok(())
}

fn runx_command() -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env("NO_COLOR", "1");
    crate::support::isolate_project(&mut command);
    Ok(command)
}

fn assert_success_contains(
    output: &std::process::Output,
    needles: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        output.status.success(),
        "status={:?}\nstderr={}\nstdout={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let stdout = String::from_utf8(output.stdout.clone())?;
    for needle in needles {
        assert!(
            stdout.contains(needle),
            "missing {needle} in stdout:\n{stdout}"
        );
    }
    assert_eq!(String::from_utf8(output.stderr.clone())?, "");
    Ok(())
}

fn assert_json_failure_contains(
    output: &std::process::Output,
    needle: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    assert!(
        !output.status.success(),
        "status={:?}\nstderr={}\nstdout={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(String::from_utf8(output.stderr.clone())?, "");
    let value = serde_json::from_slice::<serde_json::Value>(&output.stdout)?;
    assert_eq!(value["status"], "failure");
    let message = value["error"]["message"]
        .as_str()
        .ok_or("missing message")?;
    assert!(
        message.contains(needle),
        "missing {needle} in JSON error message:\n{message}"
    );
    assert!(value["error"]["code"].as_str().is_some());
    Ok(())
}

fn temp_root(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let root = std::env::temp_dir().join(format!("{name}-{}-{nanos}", std::process::id()));
    if root.exists() {
        let _ignored = fs::remove_dir_all(&root);
    }
    root
}
