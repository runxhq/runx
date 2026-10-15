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
fn registry_local_publish_search_install_json() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-local");
    let skill_dir = root.join("skill");
    let registry_dir = root.join("registry");
    let install_dir = root.join("skills");
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
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--publisher",
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
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(
        &search,
        &["\"action\": \"search\"", "\"skill_id\": \"acme/echo\""],
    )?;

    mutate_registry_version(&registry_dir, insert_signed_manifest)?;
    let install = runx_command()?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
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
    let install_dir = root.join("skills");
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
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--publisher",
            "acme",
            "--version",
            "1.0.0",
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;

    let install = runx_command()?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/bundled-root@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
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

/// Adding a skill adds the connectors its package carries to the project, so
/// the project can bind their instances by qualified reference.
#[test]
fn adding_a_skill_adds_the_connectors_it_carries() -> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-carried-connectors");
    let source = root.join("source");
    let skill_dir = source.join("carrier");
    let registry_dir = root.join("registry");
    let workspace = root.join("workspace");
    crate::support::plugin_with_source_connectors(&source, &["github"])?;
    fs::create_dir_all(&skill_dir)?;
    fs::create_dir_all(&workspace)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: carrier\ndescription: Carries the github connector.\n---\n# Carrier\n",
    )?;
    fs::write(
        skill_dir.join("run.yaml"),
        r#"skill: carrier
connectors:
  github:
    operations: [issue.read]
harness:
  cases:
    - name: digest
      runner: default
      expect:
        status: sealed
runners:
  default:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: carried
"#,
    )?;

    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--publisher",
            "acme",
            "--version",
            "1.0.0",
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;

    let add = runx_command()?
        .current_dir(&workspace)
        .args([
            "registry",
            "install",
            "acme/carrier@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(&add, &["\"status\": \"installed\""])?;

    let versions = fs::read_dir(workspace.join("connectors/github"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert!(versions[0].join("connector.yaml").is_file());

    // The workspace is in no plugin; the carried copy is reached by its ref.
    let inspect = runx_command()?
        .current_dir(&workspace)
        .args(["connector", "inspect", "github", "--json"])
        .output()?;
    assert_success_contains(&inspect, &["\"connector\": \"github\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A publish runs the harness on the package as the registry holds it: a
/// bundle that is its own plugin, its cases reaching the connectors it
/// carries.
#[test]
fn a_skill_whose_harness_grants_a_plugin_connector_publishes()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-publish-plugin-harness");
    crate::support::plugin_with_source_connectors(&root, &["google/calendar"])?;
    let skill_dir = root.join("skills/google-calendar");
    crate::support::copy_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/google-calendar"),
        &skill_dir,
    )?;
    let registry_dir = root.join("registry");
    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// A case runs its whole closure, so its provider grant reaches a connector
/// the package's composed skill carries, not only its own.
#[test]
fn a_case_grants_a_connector_its_composed_skill_carries() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-publish-composed-grant");
    crate::support::plugin_with_source_connectors(&root, &["google/calendar"])?;
    let skills = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    crate::support::copy_dir(
        &skills.join("google-calendar"),
        &root.join("skills/google-calendar"),
    )?;
    // A skill that binds no connector itself: its case reaches the calendar
    // grant only through the skill it composes.
    let skill_dir = root.join("skills/calendar-digest");
    fs::create_dir_all(&skill_dir)?;
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: calendar-digest\ndescription: Digest one day of calendar events.\n---\n# Calendar digest\n",
    )?;
    fs::write(skill_dir.join("run.yaml"), CALENDAR_DIGEST)?;
    let registry_dir = root.join("registry");
    let publish = runx_command()?
        .args([
            "registry",
            "publish",
            skill_dir.to_str().ok_or("non-utf8 skill dir")?,
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--json",
        ])
        .output()?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;
    fs::remove_dir_all(&root)?;
    Ok(())
}

const CALENDAR_DIGEST: &str = r#"skill: calendar-digest
harness:
  cases:
    - name: digest-reaches-the-composed-calendar-grant
      inputs:
        calendar_id: primary
        time_min: "2026-10-04T00:00:00Z"
        time_max: "2026-10-05T00:00:00Z"
      caller:
        provider_responses:
          principal_id: operator-fixture
          grant:
            grant_id: calendar-events-fixture
            provider: google/calendar
            scopes: [events.read]
          operations:
            - operation: events.list
              target: primary
              access: read
              result:
                calendar_id: primary
                time_min: "2026-10-04T00:00:00Z"
                time_max: "2026-10-05T00:00:00Z"
                fetched_at: "2026-10-04T01:00:00Z"
                complete: true
                events: []
      expect:
        status: sealed
        steps: [events, digest]
runners:
  digest:
    default: true
    inputs:
      calendar_id: { type: string, required: true }
      time_min: { type: string, required: true }
      time_max: { type: string, required: true }
    steps:
      - id: events
        skill: ../google-calendar
        runner: events
        inputs:
          calendar_id: $input.calendar_id
          time_min: $input.time_min
          time_max: $input.time_max
      - id: digest
        tool: data.digest
        inputs:
          value: $steps.events
"#;

/// A case's provider grant reaches a connector only a registry child carries:
/// the case's package names the child, the child's bundle carries the
/// connector, and the case's run binds it from there.
#[test]
fn a_case_grants_a_connector_its_registry_child_carries() -> Result<(), Box<dyn std::error::Error>>
{
    let root = temp_root("registry-child-grant");
    let registry_dir = publish_calendar_revisions(&root, &["1.0.0"])?;
    let consumer = calendar_consumer(&root, &["1.0.0"])?;
    let publish = publish_consumer(&consumer, &registry_dir)?;
    assert_success_contains(&publish, &["\"action\": \"publish\""])?;
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
    let registry_dir = publish_calendar_revisions(&root, &["1.0.0", "2.0.0"])?;
    let consumer = calendar_consumer(&root, &["1.0.0", "2.0.0"])?;
    let publish = publish_consumer(&consumer, &registry_dir)?;
    assert_ne!(publish.status.code(), Some(0));
    let output = format!(
        "{}{}",
        String::from_utf8_lossy(&publish.stdout),
        String::from_utf8_lossy(&publish.stderr)
    );
    assert!(
        output.contains("the closure reaches two revisions of connector google/calendar"),
        "{output}"
    );
    fs::remove_dir_all(&root)?;
    Ok(())
}

/// Publishes the google-calendar skill as acme's at each version, each
/// carrying its own revision of the google/calendar connector.
fn publish_calendar_revisions(
    root: &std::path::Path,
    versions: &[&str],
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let source = root.join("source");
    crate::support::plugin_with_source_connectors(&source, &["google/calendar"])?;
    let skill_dir = source.join("skills/google-calendar");
    crate::support::copy_dir(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/google-calendar"),
        &skill_dir,
    )?;
    let manifest = source.join("connectors/google/calendar/connector.yaml");
    let registry_dir = root.join("registry");
    let original = fs::read_to_string(&manifest)?;
    for version in versions {
        fs::write(
            &manifest,
            original.replacen(
                "description: Read Google Calendar events",
                &format!("description: Revision {version} reads Google Calendar events"),
                1,
            ),
        )?;
        let publish = runx_command()?
            .args([
                "registry",
                "publish",
                skill_dir.to_str().ok_or("non-utf8 skill dir")?,
                "--registry",
                registry_dir.to_str().ok_or("non-utf8 registry dir")?,
                "--publisher",
                "acme",
                "--version",
                version,
                "--json",
            ])
            .output()?;
        assert_success_contains(&publish, &["\"action\": \"publish\""])?;
    }
    Ok(registry_dir)
}

/// A package in no plugin whose one case reads events through each named
/// version of acme's google-calendar, granting google/calendar.
fn calendar_consumer(
    root: &std::path::Path,
    versions: &[&str],
) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let consumer = root.join("consumer/agenda");
    fs::create_dir_all(consumer.join("fixtures"))?;
    fs::write(
        consumer.join("SKILL.md"),
        "---\nname: agenda\ndescription: Reads the day's events through a published calendar skill.\n---\n# Agenda\n",
    )?;
    let steps = versions
        .iter()
        .enumerate()
        .map(|(index, version)| {
            format!(
                "      - id: events-{index}\n        skill: registry:acme/google-calendar@{version}\n        runner: events\n        inputs:\n          calendar_id: primary\n          time_min: \"2026-10-04T00:00:00Z\"\n          time_max: \"2026-10-05T00:00:00Z\"\n"
            )
        })
        .collect::<String>();
    fs::write(
        consumer.join("run.yaml"),
        format!(
            "skill: agenda\nrunners:\n  default:\n    default: true\n    result_from: [events-0]\n    steps:\n{steps}"
        ),
    )?;
    // Every child makes the one call, which the fixture answers once.
    let operations = r#"      - operation: events.list
        target: primary
        access: read
        result:
          calendar_id: primary
          time_min: "2026-10-04T00:00:00Z"
          time_max: "2026-10-05T00:00:00Z"
          fetched_at: "2026-10-04T01:00:00Z"
          complete: true
          events:
            - { event_id: instance-1, revision: v1, status: confirmed, visibility: default, transparency: opaque, summary: Review, all_day: false, start: "2026-10-04T09:00:00Z", end: "2026-10-04T10:00:00Z" }
"#;
    fs::write(
        consumer.join("fixtures/reads-through-registry-child.yaml"),
        format!(
            r#"name: agenda-reads-through-registry-child
kind: skill
target: ..
runner: default
caller:
  provider_responses:
    principal_id: operator-fixture
    grant:
      grant_id: calendar-events-fixture
      provider: google/calendar
      scopes: [events.read]
    operations:
{operations}expect:
  status: sealed
"#
        ),
    )?;
    Ok(consumer)
}

fn publish_consumer(
    consumer: &std::path::Path,
    registry_dir: &std::path::Path,
) -> Result<std::process::Output, Box<dyn std::error::Error>> {
    Ok(runx_command()?
        .current_dir(consumer.parent().ok_or("consumer has no parent")?)
        .env("RUNX_REGISTRY_DIR", registry_dir)
        .args([
            "registry",
            "publish",
            consumer.to_str().ok_or("non-utf8 consumer dir")?,
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--publisher",
            "consumer",
            "--version",
            "1.0.0",
            "--json",
        ])
        .output()?)
}

/// A graph case runs its composed skill's operations under what it binds: a
/// credential from the variable the case sets, or the provider grant its
/// caller answers for. Both are bound from the graph file's directory.
#[test]
fn a_graph_case_binds_the_connectors_its_composed_skill_calls()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("harness-graph-binding");
    crate::support::plugin_with_source_connectors(&root, &["exa", "google/calendar"])?;
    let skills = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    for name in ["web-search", "google-calendar"] {
        crate::support::copy_dir(&skills.join(name), &root.join("skills").join(name))?;
    }
    let search = root.join("skills/web-search/fixtures");
    fs::write(
        search.join("support/live.graph"),
        "name: web-search-live\nsteps:\n  - id: web-search\n    skill: ../..\n    runner: search\n    inputs:\n      query: Example API documentation\n      limit: 3\n",
    )?;
    fs::write(
        search.join("graph-binds-credential.yaml"),
        r#"name: web-search-graph-binds-credential
kind: graph
target: support/live.graph
expect:
  status: sealed
caller:
  http_exchanges:
  - request:
      method: POST
      url: https://api.exa.ai/search
      body:
        json:
          query: Example API documentation
          numResults: 3
          type: auto
    response:
      status: 200
      headers:
        content-type: application/json
      body: '{"requestId":"fixture-1","results":[{"url":"https://example.com/docs","title":"Example docs"}],"costDollars":{"total":0.005}}'
setup:
  credentials: [exa]
env:
  EXA_API_KEY: harness-exa-key
"#,
    )?;
    let calendar = root.join("skills/google-calendar/fixtures");
    fs::write(
        calendar.join("support/live.graph"),
        "name: google-calendar-live\nsteps:\n  - id: events\n    skill: ../..\n    runner: events\n    inputs:\n      calendar_id: primary\n      time_min: \"2026-10-04T00:00:00Z\"\n      time_max: \"2026-10-05T00:00:00Z\"\n",
    )?;
    fs::write(
        calendar.join("graph-binds-grant.yaml"),
        r#"name: google-calendar-graph-binds-grant
kind: graph
target: support/live.graph
expect:
  status: sealed
caller:
  provider_responses:
    principal_id: operator-fixture
    grant:
      grant_id: calendar-events-fixture
      provider: google/calendar
      scopes: [events.read]
    operations:
      - operation: events.list
        target: primary
        access: read
        result:
          calendar_id: primary
          time_min: "2026-10-04T00:00:00Z"
          time_max: "2026-10-05T00:00:00Z"
          fetched_at: "2026-10-04T01:00:00Z"
          complete: true
          events:
            - { event_id: instance-1, revision: v1, status: confirmed, visibility: default, transparency: opaque, summary: Review, all_day: false, start: "2026-10-04T09:00:00Z", end: "2026-10-04T10:00:00Z" }
"#,
    )?;
    for case in [
        search.join("graph-binds-credential.yaml"),
        calendar.join("graph-binds-grant.yaml"),
    ] {
        let run = runx_command()?
            .current_dir(&root)
            .args(["harness", case.to_str().ok_or("non-utf8 case")?, "--json"])
            .output()?;
        assert_eq!(
            run.status.code(),
            Some(0),
            "{}\nstdout={}\nstderr={}",
            case.display(),
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
    }
    fs::remove_dir_all(&root)?;
    Ok(())
}

#[test]
fn registry_package_metadata_names_the_declared_connectors()
-> Result<(), Box<dyn std::error::Error>> {
    let root = temp_root("registry-package-connectors");
    crate::support::plugin_with_source_connectors(&root, &["github", "n8n", "zapier"])?;
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
  webhook:
    operations: [handoff.deliver]
    connector_from: $input.provider
    allowed_connectors: [zapier, n8n]
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
    // The package states what it declares, so a registry checks a
    // presentation's references against it: its inputs and child skills.
    assert_eq!(
        output["registry"]["package"]["metadata"]["input_names"],
        json!(["provider"])
    );
    assert_eq!(
        output["registry"]["package"]["metadata"]["skill_refs"],
        json!([])
    );
    // A run-time choice names each connector it may choose; refs are sorted
    // and operations keep the order the package lists them. Each connector is
    // named by its name and the digests a closure binds it by.
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
            json!({"ref": "github", "runtime_capability": false, "operations": ["issues.read", "issue.read"]}),
            json!({"ref": "n8n", "runtime_capability": false, "operations": ["handoff.deliver"]}),
            json!({"ref": "zapier", "runtime_capability": false, "operations": ["handoff.deliver"]}),
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
        "schema: runx.connector.v1\nid: acme/ops\nname: Acme Ops\ndescription: Reads Acme items.\nruntime_capability: false\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations:\n  item.read:\n    description: Read an item.\n    verb: read\n    scopes: [item.read]\n    target: {template: \"acme://items\"}\n",
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
    assert_eq!(packaged["ref"], "acme/ops");
    assert_eq!(packaged["manifest"]["id"], "acme/ops");
    assert_eq!(packaged["manifest"]["runtime_capability"], false);
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
        "schema: runx.connector.v1\nid: acme/ops\nname: Acme Ops\ndescription: Reads Acme items.\nruntime_capability: false\nauth: {none: {}}\nplacements: {local: {auth: [none]}}\noperations:\n  item.read:\n    description: Read an item.\n    verb: read\n    scopes: [item.read]\n    target: {template: \"acme://items\"}\n",
    )?;
    fs::write(connector.join("README.md"), "# Acme Ops\n")?;
    let package = runx_runtime::registry::prepare_registry_connector_package(&connector)?;
    let content_digest =
        runx_contracts::registry_content_digest(package.files.iter().map(|file| {
            runx_contracts::RegistryContentFile {
                path: &file.path,
                content: &file.content,
            }
        }))?;
    let install_command = runx_contracts::CopyableCommand::posix(vec![
        "runx".to_owned(),
        "registry".to_owned(),
        "install".to_owned(),
        "acme/ops@1.0.0".to_owned(),
    ])?;
    let detail = json!({"status": "success", "connector": {
        "ref": "acme/ops",
        "version": "1.0.0",
        "name": "Acme Ops",
        "description": "Reads Acme items.",
        "content_digest": content_digest,
        "manifest_digest": package.manifest_digest,
        "connector_digest": package.connector_digest,
        "runtime_capability": false,
        "trust_tier": "community",
        "operations": [{"name": "item.read", "description": "Read an item."}],
        "install_command": install_command,
    }});
    // No skill holds the name, so `registry install` installs the connector.
    let registry = crate::support::MockRegistry::serve(&[
        ("/v1/connectors/acme/ops", detail.clone()),
        ("/v1/connectors/acme/ops/versions/1.0.0", detail),
        (
            "/v1/connectors/acme/ops/versions/1.0.0/package",
            json!({"status": "success", "package": {
                "ref": "acme/ops",
                "version": "1.0.0",
                "trust_tier": "community",
                "files": package.files,
                "signed_manifest": sign_envelope(
                    runx_contracts::RegistrySubjectKind::Connector,
                    "acme/ops",
                    "1.0.0",
                    content_digest.clone(),
                )?,
            }}),
        ),
    ])?;
    let home = root.join("home");
    // An installed connector is added to the project's connectors directory.
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    let connector_command =
        |args: &[&str]| -> Result<std::process::Output, Box<dyn std::error::Error>> {
            Ok(runx_command()?
                .env("RUNX_HOME", &home)
                .env("RUNX_CWD", &project)
                .env("RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK", "1")
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
    let destination = project.join("connectors/acme/ops/1.0.0");
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
    let markdown = include_str!("../../../fixtures/registry/install/echo-SKILL.md");
    let profile = include_str!("../../../fixtures/registry/install/echo-run.yaml");
    let content_digest =
        runx_runtime::registry::skill_content_digest(markdown, Some(profile), &[])?;
    let command = |argv: &[&str]| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        Ok(serde_json::to_value(
            runx_contracts::CopyableCommand::posix(
                argv.iter().map(|word| (*word).to_owned()).collect(),
            )?,
        )?)
    };
    let publisher = json!({"kind": "organization", "id": "pub_acme", "handle": "acme"});
    let registry = crate::support::MockRegistry::serve(&[
        // The registry resolves the root name `echo` to acme's skill.
        (
            "/v1/skills/echo",
            json!({"status": "success", "skill": {
                "skill_id": "acme/echo",
                "owner": "acme",
                "name": "echo",
                "version": "1.0.0",
                "digest": "sha256:fixture",
                "markdown": markdown,
                "runner_names": ["default"],
                "trust_tier": "verified",
                "required_scopes": [],
                "tags": [],
                "publisher": publisher,
                "attestations": [],
                "install_command": command(&["runx", "registry", "install", "acme/echo@1.0.0"])?,
                "run_command": command(&["runx", "skill", "acme/echo@1.0.0"])?,
            }}),
        ),
        (
            "POST /v1/skills/acme/echo/acquire",
            json!({"status": "success", "install_count": 1, "acquisition": {
                "skill_id": "acme/echo",
                "owner": "acme",
                "name": "echo",
                "version": "1.0.0",
                "digest": "sha256:fixture",
                "markdown": markdown,
                "profile_document": profile,
                "runner_names": ["default"],
                "trust_tier": "verified",
                "publisher": publisher,
                "attestations": [],
                "signed_manifest": sign_envelope(
                    runx_contracts::RegistrySubjectKind::Skill,
                    "acme/echo",
                    "1.0.0",
                    content_digest,
                )?,
            }}),
        ),
    ])?;
    let project = root.join("project");
    fs::create_dir_all(&project)?;
    let install = |subject: &str| -> Result<std::process::Output, Box<dyn std::error::Error>> {
        Ok(runx_command()?
            .env("RUNX_HOME", root.join("home"))
            .env("RUNX_CWD", &project)
            .env("RUNX_PUBLIC_API_ALLOW_PRIVATE_NETWORK", "1")
            .args(["registry", "install", subject, "--registry", &registry.url])
            .arg("--json")
            .output()?)
    };

    let installed = install("echo")?;
    assert_success_contains(&installed, &["\"action\": \"install\""])?;
    assert!(project.join("skills/acme/echo/1.0.0/SKILL.md").is_file());

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
    let registry_dir = root.join("registry");
    let install_dir = root.join("skills");
    publish_fixture_version(&root, &registry_dir, "1.0.0", "Echo")?;
    publish_fixture_version(&root, &registry_dir, "2.0.0", "Echo v2")?;
    sign_registry_version(&registry_dir, "1.0.0")?;
    sign_registry_version(&registry_dir, "2.0.0")?;

    for subject in ["acme/echo@1.0.0", "acme/echo@2.0.0"] {
        let install = runx_command()?
            .env("RUNX_CWD", &root)
            .args([
                "registry",
                "install",
                subject,
                "--registry",
                registry_dir.to_str().ok_or("non-utf8 registry dir")?,
                "--json",
            ])
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
        let install_dir = root.join("skills");

        let install = runx_command()?
            .env("RUNX_CWD", &root)
            .args([
                "registry",
                "install",
                "acme/echo@1.0.0",
                "--registry",
                registry_dir.to_str().ok_or("non-utf8 registry dir")?,
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
    let install_dir = root.join("skills");

    let install = runx_command()?
        .env("RUNX_CWD", &root)
        .env("RUNX_REGISTRY_URL", "https://runx.ai")
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
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
    publish_fixture_version(&root, &registry_dir, "1.0.0", "Echo")?;
    publish_fixture_version(&root, &registry_dir, "2.0.0", "Echo v2")?;
    mutate_registry_version(&registry_dir, insert_signed_manifest)?;

    let install_v1 = runx_command()?
        .env("RUNX_CWD", &root)
        .args([
            "registry",
            "install",
            "acme/echo@1.0.0",
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
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
    let file_search = runx_command()?
        .args(["registry", "search", "echo", "--registry", &file_registry])
        .output()?;
    assert_success_contains(
        &file_search,
        &["registry search  echo", "source           file"],
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
            "--registry",
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
            "install  runx registry install acme/echo@1.0.0",
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
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--publisher",
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
            "--registry",
            registry_dir.to_str().ok_or("non-utf8 registry dir")?,
            "--publisher",
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

/// The envelope the registry serves for a stored record's exact files.
fn signed_manifest(
    version_record: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let skill_id = version_record["skill_id"]
        .as_str()
        .ok_or("missing skill_id")?;
    let version = version_record["version"]
        .as_str()
        .ok_or("missing version")?;
    let markdown = version_record["markdown"]
        .as_str()
        .ok_or("missing markdown")?;
    let package_files: Vec<runx_runtime::registry::RegistryPackageFile> =
        serde_json::from_value(version_record["package_files"].clone()).unwrap_or_default();
    let content_digest = runx_runtime::registry::skill_content_digest(
        markdown,
        version_record["profile_document"].as_str(),
        &package_files,
    )?;
    sign_envelope(
        runx_contracts::RegistrySubjectKind::Skill,
        skill_id,
        version,
        content_digest,
    )
}

/// The envelope the registry serves for one subject's version and content.
fn sign_envelope(
    kind: runx_contracts::RegistrySubjectKind,
    subject_ref: &str,
    version: &str,
    content_digest: String,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut manifest = runx_contracts::RegistrySignedManifest {
        schema: runx_runtime::registry::REGISTRY_SIGNED_MANIFEST_SCHEMA.to_owned(),
        subject: runx_contracts::RegistrySubject {
            kind,
            subject_ref: subject_ref.to_owned(),
        },
        version: version.to_owned(),
        content_digest,
        signer: runx_contracts::RegistryManifestSigner {
            id: TEST_MANIFEST_SIGNER_ID.to_owned(),
            key_id: TEST_MANIFEST_KEY_ID.to_owned(),
        },
        signature: runx_contracts::RegistryManifestSignature {
            alg: "ed25519".to_owned(),
            value: "base64:".to_owned(),
        },
    };
    let signature = test_manifest_key_pair()?.sign(&manifest.signing_payload()?);
    manifest.signature.value = format!("base64:{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
    Ok(serde_json::to_value(manifest)?)
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
