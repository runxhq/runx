use std::path::PathBuf;

use runx_contracts::{JsonNumber, JsonValue};
use runx_runtime::registry::{
    AcquiredRegistrySkill, FileRegistryStore, IngestSkillOptions, InstallCandidate,
    InstallLocalSkillResult, InstallStatus, PublishSkillMarkdownOptions, PublishStatus,
    RegistryPublisher, RegistryResolveOptions, RegistrySearchOptions, TrustTier,
    ingest_skill_markdown, publish_skill_markdown, read_registry_skill, resolve_registry_skill,
    resolve_runx_link, search_registry_with_options,
};
use runx_runtime::{RegistryInstallMetadataInput, registry_install_receipt_metadata};
use tempfile::tempdir;

#[test]
fn registry_install_metadata_records_installed_digest() -> Result<(), Box<dyn std::error::Error>> {
    let candidate = install_candidate()?;
    let install = install_result(
        "sha256:installed",
        Some("sha256:profile-installed"),
        InstallStatus::Installed,
    );
    let acquisition = acquisition("sha256:remote-advertised", 7)?;

    let metadata = registry_install_receipt_metadata(RegistryInstallMetadataInput {
        candidate: &candidate,
        install: &install,
        acquisition: Some(&acquisition),
    });

    assert_eq!(metadata.get("ref"), Some(&string("acme/echo@1.0.0")));
    assert_eq!(metadata.get("skill_id"), Some(&string("acme/echo")));
    assert_eq!(metadata.get("version"), Some(&string("1.0.0")));
    assert_eq!(metadata.get("digest"), Some(&string("sha256:installed")));
    assert_eq!(
        metadata.get("profile_digest"),
        Some(&string("sha256:profile-installed"))
    );
    assert_eq!(metadata.get("trust_tier"), Some(&string("verified")));
    assert_eq!(metadata.get("source_label"), Some(&string("runx registry")));
    assert_eq!(
        metadata.get("destination"),
        Some(&string("/tmp/runx/skills/acme/echo/SKILL.md"))
    );
    assert_eq!(metadata.get("status"), Some(&string("installed")));
    assert_eq!(
        metadata.get("install_count"),
        Some(&JsonValue::Number(JsonNumber::U64(7)))
    );
    assert_eq!(
        metadata.get("publisher"),
        Some(&JsonValue::Object(
            [
                ("display_name".to_owned(), string("Acme")),
                ("handle".to_owned(), string("acme")),
                ("id".to_owned(), string("pub_1")),
                ("kind".to_owned(), string("organization")),
            ]
            .into_iter()
            .collect()
        ))
    );
    Ok(())
}

#[test]
fn registry_install_metadata_omits_absent_remote_fields() -> Result<(), Box<dyn std::error::Error>>
{
    let candidate = install_candidate()?;
    let install = install_result("sha256:installed", None, InstallStatus::Unchanged);

    let metadata = registry_install_receipt_metadata(RegistryInstallMetadataInput {
        candidate: &candidate,
        install: &install,
        acquisition: None,
    });

    assert_eq!(metadata.get("digest"), Some(&string("sha256:installed")));
    assert_eq!(metadata.get("status"), Some(&string("unchanged")));
    assert!(!metadata.contains_key("profile_digest"));
    assert!(!metadata.contains_key("publisher"));
    assert!(!metadata.contains_key("install_count"));
    Ok(())
}

#[test]
fn file_registry_store_covers_profiled_skill_surface() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let markdown = include_str!("../../../skills/sourcey/SKILL.md");
    let profile_document = r#"skill: sourcey
version: "1.0.0"

catalog:
  kind: skill
  audience: public
  visibility: public
  role: context
  execution: read
  completion: runtime_receipt
  requires_adapter: false
  approval: none

harness:
  cases:
    - name: sourcey-registry-profile
      runner: sourcey
      operator_journeys:
        - mode: standalone
          confusors: [release, answer-from-docs]
          request: Inspect this profiled documentation operation.
          expected_outcome: Return the advertised bounded documentation profile result.
        - mode: composed
          request: Continue from a prior documentation profile result.
          expected_outcome: Reuse the supplied result without repeating profile discovery.
          prior_evidence:
            - The documentation profile result supplied by the prior step.
          must_not_repeat:
            - Do not repeat profile discovery.
      expect:
        status: sealed

runners:
  sourcey:
    default: true
    type: graph
    graph:
      name: sourcey-registry-profile
      result_from: [digest]
      steps:
        - id: digest
          tool: data.digest
          inputs:
            value: sourcey-registry-profile
"#
    .to_owned();

    let version = ingest_skill_markdown(
        &store,
        markdown,
        IngestSkillOptions {
            owner: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some("2026-04-10T00:00:00.000Z".to_owned()),
            profile_document: Some(profile_document.clone()),
            ..IngestSkillOptions::default()
        },
    )?;

    assert_eq!(version.skill_id, "acme/sourcey");
    assert_eq!(version.category.as_deref(), Some("content"));
    assert_eq!(version.source_category.as_deref(), None);
    assert_eq!(version.source_type, "graph");
    assert_eq!(version.runner_names, vec!["sourcey"]);
    assert_eq!(
        version.profile_document.as_deref(),
        Some(profile_document.as_str())
    );
    assert_eq!(version.profile_digest.as_ref().map(String::len), Some(64));
    assert_eq!(version.markdown, markdown);

    let versions = store.list_versions("acme/sourcey")?;
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].created_at, "2026-04-10T00:00:00.000Z");

    let skills = store.list_skills()?;
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].category.as_deref(), Some("content"));
    assert_eq!(skills[0].source_category.as_deref(), None);
    assert_eq!(skills[0].latest_version, "1.0.0");
    assert_eq!(skills[0].versions[0].skill_id, "acme/sourcey");

    let search_results = search_registry_with_options(
        &store,
        "sourcey",
        RegistrySearchOptions {
            registry_url: Some("https://runx.example.test".to_owned()),
            ..RegistrySearchOptions::default()
        },
    )?;
    assert_eq!(search_results.len(), 1);
    assert_eq!(search_results[0].skill_id, "acme/sourcey");
    assert_eq!(search_results[0].category.as_deref(), Some("content"));
    assert_eq!(search_results[0].source_category.as_deref(), None);
    assert!(search_results[0].tags.iter().any(|tag| tag == "content"));
    assert_eq!(search_results[0].source.as_deref(), Some("runx-registry"));
    assert_eq!(
        search_results[0].source_label.as_deref(),
        Some("runx registry")
    );
    assert_eq!(
        search_results[0].profile_mode,
        runx_runtime::registry::ProfileMode::Profiled
    );
    assert_eq!(search_results[0].profile_digest, version.profile_digest);
    assert_eq!(
        search_results[0].install_command,
        "runx add acme/sourcey@1.0.0 --registry https://runx.example.test"
    );
    assert_eq!(
        search_registry_with_options(&store, "content", RegistrySearchOptions::default())?[0]
            .skill_id,
        "acme/sourcey"
    );
    assert!(
        search_results[0]
            .trust_signals
            .iter()
            .any(|signal| signal.id == "runner_metadata" && signal.status == "verified")
    );

    let link = resolve_runx_link(&store, "acme/sourcey", Some("1.0.0"), None)?
        .ok_or_else(|| std::io::Error::other("missing runx link"))?;
    assert_eq!(link.skill_id, "acme/sourcey");
    assert_eq!(link.version, "1.0.0");
    assert_eq!(link.digest, version.digest);

    let detail = read_registry_skill(&store, "acme/sourcey", Some("1.0.0"), None)?
        .ok_or_else(|| std::io::Error::other("missing registry detail"))?;
    assert_eq!(detail.markdown, markdown);
    assert_eq!(detail.category.as_deref(), Some("content"));
    assert_eq!(detail.source_category.as_deref(), None);
    assert_eq!(detail.profile_digest, version.profile_digest);

    let resolved = resolve_registry_skill(
        &store,
        "registry:sourcey",
        RegistryResolveOptions::default(),
    )?
    .ok_or_else(|| std::io::Error::other("missing registry resolution"))?;
    assert_eq!(resolved.skill_id, "acme/sourcey");
    assert_eq!(
        resolved.profile_document.as_deref(),
        Some(profile_document.as_str())
    );
    assert_eq!(resolved.runner_names, vec!["sourcey"]);

    Ok(())
}

#[test]
fn pinned_registry_lookup_ignores_invalid_siblings() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let current = ingest_skill_markdown(
        &store,
        include_str!("../../../fixtures/skills/echo/SKILL.md"),
        IngestSkillOptions {
            owner: Some("acme".to_owned()),
            version: Some("1.0.0".to_owned()),
            ..IngestSkillOptions::default()
        },
    )?;
    let invalid_sibling = temp.path().join("acme/echo/0.9.0.json");
    let mut historic = serde_json::to_value(&current)?;
    historic["version"] = serde_json::Value::String("0.9.0".to_owned());
    historic["profile_document"] = serde_json::Value::String(
        r#"skill: echo
version: "0.9.0"

catalog:
  kind: skill
  audience: public
  visibility: public
  role: context

runners:
  echo:
    default: true
    type: graph
    graph:
      name: echo
      result_from: [digest]
      steps:
        - id: digest
          tool: data.digest
          inputs:
            value: "{{message}}"
"#
        .to_owned(),
    );
    historic["profile_digest"] = serde_json::Value::Null;
    std::fs::write(&invalid_sibling, serde_json::to_vec_pretty(&historic)?)?;

    let resolved = store
        .get_version("acme/echo", Some("1.0.0"))?
        .ok_or_else(|| std::io::Error::other("missing pinned registry version"))?;
    assert_eq!(resolved.digest, current.digest);
    assert!(
        store
            .get_version("acme/echo", Some("0.9.0"))
            .is_err_and(|error| error.to_string().contains("retired graph input binding"))
    );
    assert!(
        store
            .list_versions("acme/echo")
            .is_err_and(|error| error.to_string().contains("retired graph input binding"))
    );

    Ok(())
}

#[test]
fn local_registry_owner_runx_defaults_to_community_trust() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let version = ingest_skill_markdown(
        &store,
        include_str!("../../../skills/sourcey/SKILL.md"),
        IngestSkillOptions {
            owner: Some("runx".to_owned()),
            version: Some("1.0.0".to_owned()),
            ..IngestSkillOptions::default()
        },
    )?;

    assert_eq!(version.skill_id, "runx/sourcey");
    assert_eq!(version.trust_tier, TrustTier::Community);
    let results =
        search_registry_with_options(&store, "sourcey", RegistrySearchOptions::default())?;
    assert_eq!(results[0].trust_tier, TrustTier::Community);
    Ok(())
}

#[test]
fn local_registry_search_excludes_internal_catalog_versions_but_direct_resolve_works()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let markdown = r#"---
name: internal-pay-step
description: Internal payment engine step.
---
Runs as an engine step, not as a public catalog face.
"#;
    let profile_document = r#"skill: internal-pay-step
catalog:
  kind: skill
  audience: operator
  visibility: internal
  role: graph-stage
  part_of:
    - runx/spend
runners:
  default:
    default: true
    type: cli-tool
    command: node
    args:
      - -e
      - "process.stdout.write('{}')"
harness:
  cases:
    - name: internal-pay-step-smoke
      inputs: {}
      expect:
        status: sealed
"#;

    let version = ingest_skill_markdown(
        &store,
        markdown,
        IngestSkillOptions {
            owner: Some("runx".to_owned()),
            version: Some("1.0.0".to_owned()),
            created_at: Some("2026-06-01T00:00:00.000Z".to_owned()),
            profile_document: Some(profile_document.to_owned()),
            ..IngestSkillOptions::default()
        },
    )?;

    assert_eq!(version.catalog_visibility.as_deref(), Some("internal"));
    let search_results = search_registry_with_options(
        &store,
        "internal-pay-step",
        RegistrySearchOptions::default(),
    )?;
    assert!(search_results.is_empty());

    let resolved = resolve_registry_skill(
        &store,
        "registry:internal-pay-step",
        RegistryResolveOptions::default(),
    )?
    .ok_or_else(|| std::io::Error::other("missing internal registry resolution"))?;
    assert_eq!(resolved.skill_id, "runx/internal-pay-step");
    assert_eq!(resolved.profile_document.as_deref(), Some(profile_document));
    Ok(())
}

#[test]
fn local_registry_publish_rejects_changed_duplicate() -> Result<(), Box<dyn std::error::Error>> {
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path());
    let markdown = include_str!("../../../fixtures/skills/echo/SKILL.md");

    let first = publish_skill_markdown(
        &store,
        markdown,
        PublishSkillMarkdownOptions {
            ingest: IngestSkillOptions {
                owner: Some("acme".to_owned()),
                version: Some("1.0.0".to_owned()),
                created_at: Some("2026-04-10T00:00:00.000Z".to_owned()),
                ..IngestSkillOptions::default()
            },
            registry_url: Some("https://runx.example.test".to_owned()),
            ..PublishSkillMarkdownOptions::default()
        },
    )?;
    let second = publish_skill_markdown(
        &store,
        markdown,
        PublishSkillMarkdownOptions {
            ingest: IngestSkillOptions {
                owner: Some("acme".to_owned()),
                version: Some("1.0.0".to_owned()),
                ..IngestSkillOptions::default()
            },
            registry_url: Some("https://runx.example.test".to_owned()),
            ..PublishSkillMarkdownOptions::default()
        },
    )?;

    assert_eq!(first.status, PublishStatus::Published);
    assert_eq!(first.skill_id, "acme/echo");
    assert_eq!(first.source_type, "manual");
    assert_eq!(first.digest.len(), 64);
    assert_eq!(
        first.link.install_command,
        "runx add acme/echo@1.0.0 --registry https://runx.example.test"
    );
    assert_eq!(
        first.link.run_command,
        "runx skill acme/echo@1.0.0 --registry https://runx.example.test"
    );
    assert_eq!(second.status, PublishStatus::Unchanged);
    assert_eq!(second.digest, first.digest);
    assert!(second.runner_names.is_empty());

    let changed = markdown.replace("Echo the provided message.", "Echo the changed message.");
    let conflict = publish_skill_markdown(
        &store,
        &changed,
        PublishSkillMarkdownOptions {
            ingest: IngestSkillOptions {
                owner: Some("acme".to_owned()),
                version: Some("1.0.0".to_owned()),
                ..IngestSkillOptions::default()
            },
            registry_url: None,
            ..PublishSkillMarkdownOptions::default()
        },
    );
    assert!(conflict.is_err_and(|error| error.to_string().contains("different digest")));

    Ok(())
}

#[test]
fn file_registry_store_rejects_path_traversal_skill_ids() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = tempdir()?;
    let store = FileRegistryStore::new(temp.path().join("registry"));
    let markdown = include_str!("../../../fixtures/skills/echo/SKILL.md");

    for skill_id in ["../echo", "acme/..", "./echo", "acme/."] {
        let versions = store.list_versions(skill_id);
        assert!(
            versions.is_err_and(|error| error.to_string().contains("path component")),
            "{skill_id} should be rejected before registry path resolution"
        );
    }

    let result = publish_skill_markdown(
        &store,
        markdown,
        PublishSkillMarkdownOptions {
            ingest: IngestSkillOptions {
                owner: Some("..".to_owned()),
                version: Some("1.0.0".to_owned()),
                ..IngestSkillOptions::default()
            },
            registry_url: None,
            ..PublishSkillMarkdownOptions::default()
        },
    );
    assert!(result.is_err_and(|error| error.to_string().contains("path component")));
    assert!(!temp.path().join("echo").exists());

    Ok(())
}

fn install_candidate() -> Result<InstallCandidate, Box<dyn std::error::Error>> {
    Ok(InstallCandidate {
        markdown: "---\nname: echo\n---\n# Echo\n".to_owned(),
        profile_document: None,
        package_files: Vec::new(),
        package_digest: None,
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        r#ref: "acme/echo@1.0.0".to_owned(),
        skill_id: Some("acme/echo".to_owned()),
        version: Some("1.0.0".to_owned()),
        signed_manifest: None,
        profile_digest: None,
        runner_names: Vec::new(),
        trust_tier: Some(TrustTier::Verified),
        manifest_source_authority: None,
    })
}

fn install_result(
    digest: &str,
    profile_digest: Option<&str>,
    status: InstallStatus,
) -> InstallLocalSkillResult {
    InstallLocalSkillResult {
        status,
        destination: PathBuf::from("/tmp/runx/skills/acme/echo/SKILL.md"),
        skill_name: "echo".to_owned(),
        source: "runx-registry".to_owned(),
        source_label: "runx registry".to_owned(),
        skill_id: Some("acme/echo".to_owned()),
        version: Some("1.0.0".to_owned()),
        digest: digest.to_owned(),
        profile_digest: profile_digest.map(str::to_owned),
        profile_state_path: None,
        runner_names: Vec::new(),
        trust_tier: Some(TrustTier::Verified),
    }
}

fn acquisition(
    digest: &str,
    install_count: u64,
) -> Result<AcquiredRegistrySkill, Box<dyn std::error::Error>> {
    Ok(AcquiredRegistrySkill {
        skill_id: "acme/echo".to_owned(),
        owner: "acme".to_owned(),
        name: "echo".to_owned(),
        version: "1.0.0".to_owned(),
        digest: digest.to_owned(),
        signed_manifest: None,
        markdown: "---\nname: echo\n---\n# Echo\n".to_owned(),
        profile_document: None,
        profile_digest: None,
        package_files: Vec::new(),
        package_digest: None,
        paid_listing: None,
        runner_names: Vec::new(),
        trust_tier: TrustTier::Verified,
        publisher: RegistryPublisher {
            kind: "organization".to_owned(),
            id: "pub_1".to_owned(),
            handle: Some("acme".to_owned()),
            display_name: Some("Acme".to_owned()),
        },
        source_metadata: None,
        attestations: Vec::new(),
        install_count,
    })
}

fn string(value: &str) -> JsonValue {
    JsonValue::String(value.to_owned())
}
