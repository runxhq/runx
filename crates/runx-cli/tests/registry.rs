use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::signature::KeyPair;
use serde_json::json;

const TEST_MANIFEST_KEY_ID: &str = "runx-registry-test-key";
const TEST_MANIFEST_SIGNER_ID: &str = "runx-registry-test-signer";
const TEST_MANIFEST_SEED: [u8; 32] = [7; 32];

#[test]
fn registry_local_publish_search_resolve_install_json() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-local");
    let skill_dir = root.join("skill");
    let registry_dir = root.join("registry");
    let install_dir = root.join("installed");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        include_str!("../../../fixtures/registry/install/echo-SKILL.md"),
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        include_str!("../../../fixtures/registry/install/echo-run.yaml"),
    )?;

    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--owner",
            "acme",
            "--version",
            "1.0.0",
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &publish,
        &["\"action\": \"publish\"", "\"skill_id\": \"acme/echo\""],
    )?;

    let search = runx_command()?
        .args([
            "registry",
            "search",
            "echo",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &search,
        &["\"action\": \"search\"", "\"skill_id\": \"acme/echo\""],
    )?;

    let resolve = runx_command()?
        .args([
            "registry",
            "resolve",
            "registry:echo",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &resolve,
        &[
            "\"action\": \"resolve\"",
            "\"kind\": \"local\"",
            "\"skill_id\": \"acme/echo\"",
        ],
    )?;

    mutate_registry_version(&registry_dir, insert_signed_manifest)?;
    let install = runx_command()?
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--to",
            install_dir.to_str().ok_or("non-utf8 install dir")?,
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
    assert!(
        install_dir
            .join("acme")
            .join("echo")
            .join("1.0.0")
            .join("SKILL.md")
            .exists()
    );

    Ok(())
}

#[test]
fn registry_local_publish_install_executes_bundled_sibling()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-bundled-sibling");
    let source = root.join("source");
    let skill_dir = source.join("root");
    let sibling_dir = source.join("sibling");
    let leaf_dir = source.join("leaf");
    let registry_dir = root.join("registry");
    let install_dir = root.join("installed");
    fs::create_dir_all(skill_dir.join("packets"))?;
    fs::create_dir_all(&sibling_dir)?;
    fs::create_dir_all(&leaf_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: bundled-root\ndescription: Registry bundle integration fixture.\n---\n# Bundled root\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"skill: bundled-root
harness:
  cases:
    - name: sibling
      runner: default
      expect:
        status: sealed
runners:
  default:
    default: true
    result_from: [sibling]
    steps:
      - id: sibling
        skill: ../sibling
        artifacts:
          wrap_as: sibling_packet
          packet: runx.test.registry-sibling.v1
"#,
    )?;
    fs::write(
        skill_dir.join("packets/sibling.schema.json"),
        "{\"x-runx-packet-id\":\"runx.test.registry-sibling.v1\",\"type\":\"object\"}\n",
    )?;
    fs::write(
        sibling_dir.join("SKILL.md"),
        "---\nname: bundled-sibling\ndescription: Bundled dependency fixture.\n---\n# Bundled sibling\n",
    )?;
    fs::write(
        sibling_dir.join("run.yaml"),
        r#"skill: bundled-sibling
runners:
  default:
    default: true
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
        r#"skill: bundled-leaf
runners:
  default:
    default: true
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

    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--owner",
            "acme",
            "--version",
            "1.0.0",
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;

    let install = runx_command()?
        .args([
            "registry",
            "install",
            "acme/bundled-root@1.0.0",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--to",
            install_dir.to_str().ok_or("non-utf8 install dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(&install, &["\"status\": \"installed\""])?;

    let installed = install_dir.join("acme").join("bundled-root").join("1.0.0");
    assert!(installed.join("runx.package.json").is_file());
    assert!(installed.join("dependencies/1/run.yaml").is_file());
    assert!(installed.join("dependencies/2/run.yaml").is_file());
    let run = crate::support::isolated_runx_command_with_inherited_cwd("registry-bundle-run")
        .current_dir(&root)
        .env("RUNX_HOME", root.join("home"))
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

#[test]
fn registry_package_metadata_names_the_declared_connectors()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-package-connectors");
    let skill_dir = root.join("connected");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: connected\ndescription: Declares a fixed and a chosen connector.\n---\n# Connected\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"skill: connected
connectors:
  github:
    operations: [issues.read, issue.read]
  reply:
    operations: [thread.reply, thread.reply.read]
    connector_from: $input.provider
    allowed_connectors: [slack, nitrosend]
runners:
  default:
    default: true
    inputs:
      provider: { type: string, required: true }
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: connected
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
    // A run-time choice names each connector it may choose; refs are sorted
    // and operations keep the order the package lists them. Each connector is
    // named by its publisher and the digests a closure binds it by.
    let connectors = output["registry"]["package"]["metadata"]["connectors"]
        .as_array()
        .ok_or("connectors missing")?;
    let named = connectors
        .iter()
        .map(|connector| {
            json!({
                "ref": connector["ref"],
                "runtime_capability": connector["runtime_capability"],
                "operations": connector["operations"],
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        named,
        [
            json!({"ref": "runx/github", "runtime_capability": false, "operations": ["issues.read", "issue.read"]}),
            json!({"ref": "runx/nitrosend", "runtime_capability": false, "operations": ["thread.reply", "thread.reply.read"]}),
            json!({"ref": "runx/slack", "runtime_capability": false, "operations": ["thread.reply", "thread.reply.read"]}),
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
        "schema: runx.connector.v1\nid: acme-ops\nname: Acme Ops\ndescription: Reads Acme items.\nruntime_capability: false\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations:\n  item.read:\n    description: Read an item.\n    verb: read\n    scopes: [item.read]\n    target: {template: \"acme://items\"}\n",
    )?;
    fs::write(connector.join("README.md"), "# Acme Ops\n")?;
    fs::write(connector.join("plugin.json"), r#"{"namespace":"acme"}"#)?;

    let package = runx_command()?
        .args([
            "registry",
            "connector",
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
    assert_eq!(packaged["ref"], "acme/acme-ops");
    assert_eq!(packaged["manifest"]["id"], "acme-ops");
    assert_eq!(packaged["manifest"]["runtime_capability"], false);
    // The package carries the namespace it was published under, so it loads
    // under that publisher wherever it is installed.
    let files = packaged["files"].as_array().ok_or("files missing")?;
    assert_eq!(
        files
            .iter()
            .map(|file| file["path"].clone())
            .collect::<Vec<_>>(),
        [
            json!("README.md"),
            json!("connector.yaml"),
            json!("plugin.json")
        ]
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(
            files[2]["content"]
                .as_str()
                .ok_or("plugin.json has no content")?
        )?,
        json!({"namespace": "acme"})
    );
    assert!(
        packaged["operation_digests"]["item.read"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:"))
    );

    let refused = runx_command()?
        .args([
            "registry",
            "connector",
            "package",
            connector.to_str().ok_or("non-utf8 connector dir")?,
            "--owner",
            "acme",
        ])
        .output()?;
    assert_ne!(refused.status.code(), Some(0));
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn registry_install_versions_are_side_by_side() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-side-by-side");
    let registry_dir = root.join("registry");
    let install_dir = root.join("installed");
    publish_fixture_version(&root, &registry_dir, "1.0.0", "Echo")?;
    publish_fixture_version(&root, &registry_dir, "2.0.0", "Echo v2")?;
    sign_registry_version(&registry_dir, "1.0.0")?;
    sign_registry_version(&registry_dir, "2.0.0")?;

    for (subject, version_flag) in [("acme/echo@1.0.0", None), ("acme/echo", Some("2.0.0"))] {
        let install = runx_command()?
            .args([
                "registry",
                "install",
                subject,
                "--registry-dir",
                registry_dir.to_str().ok_or("non-utf8 registry dir")?,
                "--to",
                install_dir.to_str().ok_or("non-utf8 install dir")?,
            ])
            .args(
                version_flag
                    .into_iter()
                    .flat_map(|version| ["--version", version]),
            )
            .arg("--json")
            .output()?;
        assert_success_contains(&install, &["\"action\": \"install\""])?;
    }

    assert!(
        install_dir
            .join("acme")
            .join("echo")
            .join("1.0.0")
            .join("SKILL.md")
            .exists()
    );
    assert!(
        install_dir
            .join("acme")
            .join("echo")
            .join("2.0.0")
            .join("SKILL.md")
            .exists()
    );
    assert_ne!(
        fs::read_to_string(
            install_dir
                .join("acme")
                .join("echo")
                .join("1.0.0")
                .join("SKILL.md")
        )?,
        fs::read_to_string(
            install_dir
                .join("acme")
                .join("echo")
                .join("2.0.0")
                .join("SKILL.md")
        )?
    );

    Ok(())
}

#[test]
fn registry_install_reports_typed_trust_anchor_errors() -> Result<(), Box<dyn std::error::Error>> {
    type VersionMutator = fn(&mut serde_json::Value);
    let cases: [(&str, VersionMutator); 3] = [
        ("unknown_key", |version| {
            version["signed_manifest"]["signer"]["key_id"] =
                serde_json::Value::String("unknown-key".to_owned());
        }),
        ("invalid_signature", |version| {
            version["signed_manifest"]["signature"]["value"] =
                serde_json::Value::String("base64:invalid".to_owned());
        }),
        ("digest_mismatch", |version| {
            version["markdown"] = serde_json::Value::String(
                "---\nname: echo\ndescription: echo fixture.\n---\n# Tampered\n".to_owned(),
            );
        }),
    ];

    for (error_kind, mutate) in cases {
        let root = temp_root(&format!("registry-{error_kind}"));
        let registry_dir = publish_registry_fixture(&root)?;
        mutate_registry_version(&registry_dir, |version| {
            mutate(version);
            Ok(())
        })?;
        let install_dir = root.join("installed");

        let install = runx_command()?
            .args([
                "registry",
                "install",
                "acme/echo@1.0.0",
                "--registry-dir",
                registry_dir.to_str().ok_or("non-utf8 registry dir")?,
                "--to",
                install_dir.to_str().ok_or("non-utf8 install dir")?,
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

#[test]
fn registry_local_install_accepts_unsigned_manifest() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-local-unsigned");
    let registry_dir = publish_registry_fixture(&root)?;
    mutate_registry_version(&registry_dir, |version| {
        if let Some(object) = version.as_object_mut() {
            object.remove("signed_manifest");
        }
        Ok(())
    })?;
    let install_dir = root.join("installed");

    let install = runx_command()?
        .env("RUNX_REGISTRY_URL", "https://runx.ai")
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--to",
            install_dir.to_str().ok_or("non-utf8 install dir")?,
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
    assert!(
        install_dir
            .join("acme")
            .join("echo")
            .join("1.0.0")
            .join("SKILL.md")
            .exists()
    );
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
    let registry_dir = root.join("registry");
    let install_dir = root.join("installed");
    publish_fixture_version(&root, &registry_dir, "1.0.0", "Echo")?;
    publish_fixture_version(&root, &registry_dir, "2.0.0", "Echo v2")?;
    mutate_registry_version(&registry_dir, insert_signed_manifest)?;

    let read_v1 = runx_command()?
        .args([
            "registry",
            "read",
            "acme/echo",
            "--version",
            "1.0.0",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
        ])
        .output()?;
    assert_success_contains(
        &read_v1,
        &[
            "registry read    acme/echo",
            "source           local",
            "skill            acme/echo",
            "version          1.0.0",
            "digest           sha256:",
            "trust            community",
        ],
    )?;

    let resolve_v2 = runx_command()?
        .args([
            "registry",
            "resolve",
            "acme/echo@2.0.0",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
        ])
        .output()?;
    assert_success_contains(
        &resolve_v2,
        &[
            "registry resolve acme/echo@2.0.0",
            "version          2.0.0",
            "digest           sha256:",
            "trust            community",
        ],
    )?;

    let install_v1 = runx_command()?
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--to",
            install_dir.to_str().ok_or("non-utf8 install dir")?,
        ])
        .output()?;
    assert_success_contains(
        &install_v1,
        &[
            "registry install acme/echo@1.0.0",
            "status           installed",
            "version          1.0.0",
            "digest           sha256:",
            "signed           yes (runx-registry-test-key)",
            "destination      ",
        ],
    )?;

    let file_registry = format!("file://{}", registry_dir.display());
    let file_read = runx_command()?
        .args([
            "registry",
            "read",
            "acme/echo",
            "--registry",
            &file_registry,
        ])
        .output()?;
    assert_success_contains(
        &file_read,
        &["registry read    acme/echo", "source           file"],
    )?;

    Ok(())
}

#[test]
fn registry_human_output_names_search_results() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-search-human");
    let registry_dir = root.join("registry");
    publish_fixture_version(&root, &registry_dir, "1.0.0", "Echo")?;

    let search = runx_command()?
        .args([
            "registry",
            "search",
            "echo",
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
        ])
        .output()?;
    assert_success_contains(
        &search,
        &[
            "registry search  echo",
            "results          1",
            "- acme/echo@1.0.0",
            "digest   sha256:",
            "trust    community",
            "install  runx add acme/echo@1.0.0",
            "run      runx skill acme/echo@1.0.0",
        ],
    )?;

    Ok(())
}

fn publish_registry_fixture(root: &std::path::Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let skill_dir = root.join("skill");
    let registry_dir = root.join("registry");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        include_str!("../../../fixtures/registry/install/echo-SKILL.md"),
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        include_str!("../../../fixtures/registry/install/echo-run.yaml"),
    )?;

    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--owner",
            "acme",
            "--version",
            "1.0.0",
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;
    mutate_registry_version(&registry_dir, insert_signed_manifest)?;
    Ok(registry_dir)
}

fn publish_fixture_version(
    root: &std::path::Path,
    registry_dir: &std::path::Path,
    version: &str,
    title: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let skill_dir = root.join(format!("skill-{}", version.replace('.', "-")));
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        include_str!("../../../fixtures/registry/install/echo-SKILL.md")
            .replace("# Echo", &format!("# {title}")),
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        include_str!("../../../fixtures/registry/install/echo-run.yaml"),
    )?;
    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry-dir",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--owner",
            "acme",
            "--version",
            version,
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;
    Ok(())
}

fn sign_registry_version(
    registry_dir: &std::path::Path,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let version_path = registry_dir
        .join("acme")
        .join("echo")
        .join(format!("{version}.json"));
    let mut version_record =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&version_path)?)?;
    version_record["signed_manifest"] = signed_manifest(&version_record)?;
    fs::write(
        version_path,
        format!("{}\n", serde_json::to_string_pretty(&version_record)?),
    )?;
    Ok(())
}

fn mutate_registry_version(
    registry_dir: &std::path::Path,
    mutate: impl FnOnce(&mut serde_json::Value) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let version_path = registry_dir.join("acme").join("echo").join("1.0.0.json");
    let mut version =
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&version_path)?)?;
    mutate(&mut version)?;
    fs::write(
        version_path,
        format!("{}\n", serde_json::to_string_pretty(&version)?),
    )?;
    Ok(())
}

fn insert_signed_manifest(
    version: &mut serde_json::Value,
) -> Result<(), Box<dyn std::error::Error>> {
    version["signed_manifest"] = signed_manifest(version)?;
    Ok(())
}

fn signed_manifest(
    version_record: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let skill_id = version_record["skill_id"]
        .as_str()
        .ok_or("missing skill_id")?;
    let version = version_record["version"]
        .as_str()
        .ok_or("missing version")?;
    let digest = version_record["digest"].as_str().ok_or("missing digest")?;
    let profile_digest = version_record["profile_digest"].as_str();
    let package_digest = version_record["package_digest"].as_str();
    let payload =
        registry_manifest_payload(skill_id, version, digest, profile_digest, package_digest);
    let signature = test_manifest_key_pair()?.sign(payload.as_bytes());
    Ok(json!({
        "schema": runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA,
        "skill_id": skill_id,
        "version": version,
        "digest": digest,
        "profile_digest": profile_digest,
        "package_digest": package_digest,
        "signer": {
            "id": TEST_MANIFEST_SIGNER_ID,
            "key_id": TEST_MANIFEST_KEY_ID,
        },
        "signature": {
            "alg": "ed25519",
            "value": format!(
                "base64:{}",
                URL_SAFE_NO_PAD.encode(signature.as_ref())
            ),
        },
    }))
}

fn registry_manifest_payload(
    skill_id: &str,
    version: &str,
    digest: &str,
    profile_digest: Option<&str>,
    package_digest: Option<&str>,
) -> String {
    format!(
        "{}\nskill_id={skill_id}\nversion={version}\ndigest={digest}\nprofile_digest={}\npackage_digest={}\nsigner_id={TEST_MANIFEST_SIGNER_ID}\nkey_id={TEST_MANIFEST_KEY_ID}\n",
        runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA,
        profile_digest.unwrap_or(""),
        package_digest.unwrap_or("")
    )
}

fn test_manifest_key_pair() -> Result<ring::signature::Ed25519KeyPair, std::io::Error> {
    ring::signature::Ed25519KeyPair::from_seed_unchecked(&TEST_MANIFEST_SEED).map_err(|error| {
        std::io::Error::other(format!("static registry manifest seed rejected: {error:?}"))
    })
}

fn runx_command() -> Result<Command, Box<dyn std::error::Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_runx"));
    command.env("NO_COLOR", "1");
    crate::support::isolate_project(&mut command);
    command.env(
        runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ENV,
        STANDARD.encode(test_manifest_key_pair()?.public_key().as_ref()),
    );
    command.env(
        runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_KEY_ID_ENV,
        TEST_MANIFEST_KEY_ID,
    );
    command.env(
        runx_runtime::registry::RUNX_REGISTRY_MANIFEST_TRUST_OWNER_ENV,
        "acme",
    );
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
