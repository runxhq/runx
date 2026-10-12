//! Registry publication preparation from parser-owned package truth.

mod bundle;
mod connectors;
mod files;
mod harness;

pub(crate) use connectors::connector_package;
pub use connectors::{RegistryConnectorPackage, prepare_registry_connector_package};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use thiserror::Error;

use super::{
    RegistryPackageFile, RegistryPackageMetadata, RegistryPublishHarnessReport,
    package_metadata::project_registry_package_metadata,
};
use crate::{LoadedSkillPackage, LocalOrchestrator, RuntimeError};

#[derive(Debug, Error)]
pub enum RegistryPublishPackageError {
    #[error("{message}")]
    Invalid { message: String },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

impl RegistryPublishPackageError {
    pub(super) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid {
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RegistryPublishPackageRequest<'a> {
    pub subject: &'a str,
    pub profile: Option<&'a Path>,
    pub env: &'a BTreeMap<String, String>,
    pub cwd: &'a Path,
}

pub struct RegistryPublishPackage {
    markdown: String,
    profile_document: Option<String>,
    package_files: Vec<RegistryPackageFile>,
    metadata: RegistryPackageMetadata,
    harness: Option<harness::PublishHarnessPackage>,
}

impl RegistryPublishPackage {
    #[must_use]
    pub fn markdown(&self) -> &str {
        &self.markdown
    }

    #[must_use]
    pub fn profile_document(&self) -> Option<&str> {
        self.profile_document.as_deref()
    }

    #[must_use]
    pub fn package_files(&self) -> &[RegistryPackageFile] {
        &self.package_files
    }

    #[must_use]
    pub fn metadata(&self) -> &RegistryPackageMetadata {
        &self.metadata
    }

    pub fn run_harness(
        &self,
        orchestrator: &LocalOrchestrator,
        env: &BTreeMap<String, String>,
    ) -> Result<RegistryPublishHarnessReport, RegistryPublishPackageError> {
        let harness = self.harness.as_ref().ok_or_else(|| {
            RegistryPublishPackageError::invalid(
                "publish requires a skill execution profile and at least one harness case",
            )
        })?;
        harness::run_publish_harness(orchestrator, harness.path(), env)
    }

    #[must_use]
    pub fn into_parts(self) -> RegistryPublishPackageParts {
        let Self {
            markdown,
            profile_document,
            package_files,
            metadata,
            harness,
        } = self;
        drop(harness);
        RegistryPublishPackageParts {
            markdown,
            profile_document,
            package_files,
            metadata,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RegistryPublishPackageParts {
    pub markdown: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_document: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package_files: Vec<RegistryPackageFile>,
    pub metadata: RegistryPackageMetadata,
}

pub fn prepare_registry_publish_package(
    request: RegistryPublishPackageRequest<'_>,
) -> Result<RegistryPublishPackage, RegistryPublishPackageError> {
    let subject_path =
        crate::RunxPaths::resolve(request.env, request.cwd).existing_input(request.subject);
    let profile_path =
        resolve_profile_path(&subject_path, request.profile, request.env, request.cwd);
    let load_path = profile_path.as_deref().unwrap_or(&subject_path);
    let loaded = crate::load_validated_skill_package(load_path)?;
    prepare_loaded_package(&loaded, request.env, request.cwd)
}

fn resolve_profile_path(
    subject_path: &Path,
    profile: Option<&Path>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Option<PathBuf> {
    profile
        .map(|path| crate::RunxPaths::resolve(env, cwd).existing_input(path))
        .or_else(|| {
            let package_dir = if subject_path.is_dir() {
                subject_path
            } else {
                subject_path.parent()?
            };
            let candidate = package_dir.join(runx_parser::PROFILE_FILE);
            candidate.exists().then_some(candidate)
        })
}

fn prepare_loaded_package(
    loaded: &LoadedSkillPackage,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<RegistryPublishPackage, RegistryPublishPackageError> {
    let local_closure = crate::skill_package::inspect_loaded_local_execution_closure(loaded, env)
        .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?;
    let profile_document = loaded
        .profile_path
        .as_deref()
        .and_then(|path| loaded.package.file_text(path))
        .map(str::to_owned);
    let files::CollectedPackage {
        files: package_files,
        connectors,
    } = bundle::collect_bundle_files(loaded, &local_closure, env, cwd)?;
    let metadata =
        project_registry_package_metadata(&loaded.package.skill, loaded.manifest(), connectors);
    let harness =
        harness::stage_publish_harness(loaded, profile_document.as_deref(), &package_files)?;
    Ok(RegistryPublishPackage {
        markdown: loaded.package.manual_markdown.clone(),
        profile_document,
        package_files,
        metadata,
        harness,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use super::{
        RegistryPublishPackageRequest, prepare_loaded_package, prepare_registry_publish_package,
    };

    #[test]
    fn publish_package_accepts_a_manual_only_registry_listing()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("SKILL.md"),
            "---\nname: manual-only\ndescription: Manual-only registry fixture.\n---\n# Manual only\n",
        )?;

        let package = prepare_registry_publish_package(RegistryPublishPackageRequest {
            subject: temp.path().to_str().ok_or("temporary path is not UTF-8")?,
            profile: None,
            env: &BTreeMap::new(),
            cwd: temp.path(),
        })?;

        assert!(package.profile_document().is_none());
        // Even a manual alone carries its descriptor, which marks the
        // installed package's root.
        let paths = package
            .package_files()
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["runx.package.json"]);
        assert_eq!(package.metadata().name, "manual-only");
        Ok(())
    }

    #[test]
    fn publish_package_projects_validated_source_without_a_second_walk()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: publish-shape\ndescription: Publish shape fixture.\n---\n# Publish shape\n",
        )?;
        fs::write(
            root.join("run.yaml"),
            r#"skill: publish-shape
harness:
  files:
    - fixtures/helper.mjs
  cases:
    - name: smoke
      runner: main
      inputs:
        helper: ./fixtures/helper.mjs
      expect:
        status: sealed
runners:
  main:
    default: true
    steps:
      - id: main
        tool: { command: node, args: [run.mjs] }
        outputs:
          plan: object
        artifacts:
          wrap_as: packet
          packet: runx.test.publish-shape.v1
"#,
        )?;
        fs::write(
            root.join("run.mjs"),
            "console.log(JSON.stringify({plan: {}}))\n",
        )?;
        fs::create_dir_all(root.join("tools/example/echo"))?;
        fs::write(
            root.join("tools/example/echo/manifest.json"),
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.echo",
  "source": { "command": "node", "args": ["./src/run.mjs"] }
}
"#,
        )?;
        fs::create_dir_all(root.join("tools/example/echo/src"))?;
        fs::write(
            root.join("tools/example/echo/src/run.mjs"),
            "import { echo } from './echo.mjs';\nprocess.stdout.write(echo());\n",
        )?;
        fs::write(
            root.join("tools/example/echo/src/echo.mjs"),
            "export const echo = () => '{}';\n",
        )?;
        fs::write(root.join("notes.txt"), "not published\n")?;
        fs::write(root.join(".env"), "SECRET=not-published\n")?;
        fs::create_dir_all(root.join("references"))?;
        fs::write(root.join("references/operator.md"), "# Operator\n")?;
        fs::create_dir_all(root.join("fixtures"))?;
        fs::write(root.join("fixtures/helper.mjs"), "console.log('helper')\n")?;
        fs::create_dir_all(root.join("context/review"))?;
        fs::write(
            root.join("context/review/SKILL.md"),
            "---\nname: review-context\ndescription: Review context.\n---\n# Review\n",
        )?;
        fs::write(
            root.join("context/review/run.yaml"),
            "skill: review-context\nrunners:\n  context:\n    default: true\n    steps:\n      - id: context\n        agent: { role: agent, task: context }\n",
        )?;
        fs::create_dir_all(root.join("packets"))?;
        fs::write(
            root.join("packets/publish-shape.schema.json"),
            "{\"x-runx-packet-id\":\"runx.test.publish-shape.v1\",\"type\":\"object\"}\n",
        )?;

        let package = prepare_registry_publish_package(RegistryPublishPackageRequest {
            subject: root.to_str().ok_or("temporary path is not UTF-8")?,
            profile: None,
            env: &BTreeMap::new(),
            cwd: root,
        })?;
        let paths = package
            .package_files()
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"run.mjs"));
        assert!(paths.contains(&"references/operator.md"));
        assert!(paths.contains(&"context/review/SKILL.md"));
        assert!(paths.contains(&"context/review/run.yaml"));
        assert!(paths.contains(&"packets/publish-shape.schema.json"));
        assert!(!paths.contains(&"notes.txt"));
        assert!(!paths.contains(&".env"));
        assert!(paths.contains(&"fixtures/helper.mjs"));
        assert!(paths.contains(&"tools/example/echo/manifest.json"));
        assert!(paths.contains(&"tools/example/echo/src/run.mjs"));
        assert!(paths.contains(&"tools/example/echo/src/echo.mjs"));

        let harness_path = package
            .harness
            .as_ref()
            .ok_or("staged harness missing")?
            .path()
            .to_path_buf();
        assert!(harness_path.join("fixtures/helper.mjs").is_file());
        drop(package);
        assert!(!harness_path.exists());
        Ok(())
    }

    #[test]
    fn publish_harness_preserves_local_sibling_execution_closure()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("pnpm-workspace.yaml"), "packages: []\n")?;
        let catalog = temp.path().join("skills");
        let root = catalog.join("root");
        let sibling = catalog.join("sibling");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&sibling)?;
        fs::write(
            root.join("SKILL.md"),
            "---\nname: root\ndescription: Root fixture.\n---\n# Root\n",
        )?;
        fs::write(
            root.join("run.yaml"),
            r#"skill: root
harness:
  cases:
    - name: sibling
      runner: main
      inputs: {}
      expect:
        status: sealed
runners:
  main:
    default: true
    result_from: [sibling]
    steps:
      - id: sibling
        skill: ../sibling
        artifacts:
          wrap_as: sibling_packet
          packet: runx.test.sibling.v1
"#,
        )?;
        fs::write(
            sibling.join("SKILL.md"),
            "---\nname: sibling\ndescription: Sibling fixture.\n---\n# Sibling\n",
        )?;
        fs::write(
            sibling.join("run.yaml"),
            r#"skill: sibling
runners:
  default:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: sibling
  verify:
    inputs:
      provider_operation:
        type: json
        required: true
        packet: runx.test.provider-operation.v1
    steps:
      - id: verify
        agent: { role: agent, task: verify }
        inputs:
          provider_operation: $input.provider_operation
        outputs:
          verification: object
        artifacts:
          named_emits:
            verification: verification
          packets:
            verification: runx.test.verification.v1
"#,
        )?;
        fs::create_dir_all(temp.path().join("dist/packets"))?;
        for (name, packet) in [
            ("provider-operation", "runx.test.provider-operation.v1"),
            ("verification", "runx.test.verification.v1"),
        ] {
            fs::write(
                temp.path().join(format!("dist/packets/{name}.schema.json")),
                format!(r#"{{"x-runx-packet-id":"{packet}","type":"object"}}"#),
            )?;
        }
        fs::create_dir_all(root.join("packets"))?;
        fs::write(
            root.join("packets/sibling.schema.json"),
            "{\"x-runx-packet-id\":\"runx.test.sibling.v1\",\"type\":\"object\"}\n",
        )?;

        let package = prepare_registry_publish_package(RegistryPublishPackageRequest {
            subject: root.to_str().ok_or("temporary path is not UTF-8")?,
            profile: None,
            env: &BTreeMap::new(),
            cwd: &root,
        })?;
        assert!(
            package
                .package_files()
                .iter()
                .any(|file| file.path == "packets/sibling.schema.json")
        );
        let package_paths = package
            .package_files()
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert!(package_paths.contains(&"runx.package.json"));
        assert!(package_paths.contains(&"dependencies/1/SKILL.md"));
        assert!(package_paths.contains(&"dependencies/1/run.yaml"));
        for schema in ["provider-operation", "verification"] {
            assert!(
                package_paths
                    .contains(&format!("dependencies/1/packets/{schema}.schema.json").as_str())
            );
        }
        let harness_path = package
            .harness
            .as_ref()
            .ok_or("staged harness missing")?
            .path();
        assert!(harness_path.join("dependencies/1/SKILL.md").is_file());
        assert!(harness_path.join("dependencies/1/run.yaml").is_file());
        assert!(harness_path.join("packets/sibling.schema.json").is_file());
        let report = package.run_harness(&crate::LocalOrchestrator::default(), &BTreeMap::new())?;
        assert_eq!(report.status, "passed");
        assert_eq!(report.case_names, ["sibling"]);
        Ok(())
    }

    #[test]
    fn publication_is_independent_of_workspace_and_dependency_locations()
    -> Result<(), Box<dyn std::error::Error>> {
        let workspace = tempfile::tempdir()?;
        // On macOS these roots have no common ancestor below the filesystem
        // root. Local dependencies must not require a shared checkout.
        let dependencies = tempfile::tempdir_in(std::env::current_dir()?)?;
        let prepare = |root: &std::path::Path, child: &std::path::Path| {
            fs::create_dir_all(root)?;
            fs::create_dir_all(child)?;
            fs::write(
                root.join("SKILL.md"),
                "---\nname: root\ndescription: Root fixture.\n---\n# Root\n",
            )?;
            fs::write(
                root.join("run.yaml"),
                "skill: root\nrunners:\n  main:\n    default: true\n    steps:\n      - id: child\n        skill: child\n",
            )?;
            fs::write(
                child.join("SKILL.md"),
                "---\nname: child\ndescription: Child fixture.\n---\n# Child\n",
            )?;
            fs::write(
                child.join("run.yaml"),
                "skill: child\nrunners:\n  main:\n    default: true\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: {value: child}\n",
            )?;
            let roots = std::env::join_paths([child.parent().ok_or("child parent missing")?])?;
            let env = BTreeMap::from([(
                "RUNX_SKILL_ROOTS".to_owned(),
                roots
                    .into_string()
                    .map_err(|_| "skill roots are not UTF-8")?,
            )]);
            Ok::<_, Box<dyn std::error::Error>>(
                prepare_registry_publish_package(RegistryPublishPackageRequest {
                    subject: root.to_str().ok_or("root path is not UTF-8")?,
                    profile: None,
                    env: &env,
                    cwd: root,
                })?
                .into_parts(),
            )
        };
        let first = prepare(
            &workspace.path().join("private-layout/root"),
            &dependencies.path().join("a/child"),
        )?;
        let relocated = prepare(
            &dependencies.path().join("other-layout/root"),
            &workspace.path().join("z/child"),
        )?;
        assert_eq!(first, relocated);
        assert!(
            first
                .package_files
                .iter()
                .any(|file| file.path == "dependencies/1/SKILL.md")
        );
        assert!(
            first
                .package_files
                .iter()
                .all(|file| !file.path.contains("private-layout"))
        );
        Ok(())
    }

    #[test]
    fn materialized_registry_bundle_repackages_without_drift()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let catalog = temp.path().join("skills");
        let root = catalog.join("root");
        let sibling = catalog.join("sibling");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&sibling)?;
        fs::write(
            root.join("SKILL.md"),
            "---\nname: root\ndescription: Root fixture.\n---\n# Root\n",
        )?;
        fs::write(
            root.join("run.yaml"),
            r#"skill: root
harness:
  cases:
    - name: sibling
      runner: main
      inputs: {}
      expect: { status: sealed }
runners:
  main:
    default: true
    result_from: [sibling]
    steps:
      - id: sibling
        skill: ../sibling
"#,
        )?;
        fs::write(
            sibling.join("SKILL.md"),
            "---\nname: sibling\ndescription: Sibling fixture.\n---\n# Sibling\n",
        )?;
        fs::write(
            sibling.join("run.yaml"),
            r#"skill: sibling
runners:
  default:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs: { value: sibling }
"#,
        )?;

        let first = prepare_registry_publish_package(RegistryPublishPackageRequest {
            subject: root.to_str().ok_or("temporary path is not UTF-8")?,
            profile: None,
            env: &BTreeMap::new(),
            cwd: &root,
        })?
        .into_parts();
        let staged = temp.path().join("staged");
        fs::create_dir_all(&staged)?;
        fs::write(staged.join("SKILL.md"), &first.markdown)?;
        fs::write(
            staged.join("run.yaml"),
            first
                .profile_document
                .as_deref()
                .ok_or("profile document missing")?,
        )?;
        for file in &first.package_files {
            let path = staged.join(&file.path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, &file.content)?;
        }

        let second = prepare_registry_publish_package(RegistryPublishPackageRequest {
            subject: staged.to_str().ok_or("temporary path is not UTF-8")?,
            profile: None,
            env: &BTreeMap::new(),
            cwd: &staged,
        })?
        .into_parts();

        assert_eq!(second, first);
        Ok(())
    }

    #[test]
    fn packet_input_schema_is_materialized_into_registry_bundle()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("pnpm-workspace.yaml"), "packages: []\n")?;
        let root = temp.path().join("skills/packet-consumer");
        fs::create_dir_all(&root)?;
        fs::write(
            root.join("SKILL.md"),
            "---\nname: packet-consumer\ndescription: Packet consumer fixture.\n---\n# Consumer\n",
        )?;
        fs::write(
            root.join("run.yaml"),
            r#"skill: packet-consumer
runners:
  inspect:
    default: true
    inputs:
      plan:
        type: json
        required: true
        packet: runx.test.input-plan.v1
    examples:
      - plan: { operation: inspect }
    steps:
      - id: inspect
        agent: { role: agent, task: inspect }
        inputs:
          plan: $input.plan
"#,
        )?;
        fs::create_dir_all(temp.path().join("dist/packets"))?;
        fs::write(
            temp.path().join("dist/packets/input-plan.schema.json"),
            r#"{"x-runx-packet-id":"runx.test.input-plan.v1","type":"object","required":["operation"],"properties":{"operation":{"const":"inspect"}},"additionalProperties":false}"#,
        )?;

        let loaded = crate::load_validated_skill_package(&root)?;
        fs::write(
            temp.path().join("dist/packets/input-plan.schema.json"),
            r#"{"x-runx-packet-id":"runx.test.input-plan.v1","type":"object","required":["operation"],"properties":{"operation":{"const":"mutate"}},"additionalProperties":false}"#,
        )?;
        let package = prepare_loaded_package(&loaded, &BTreeMap::new(), temp.path())?;

        assert!(package.package_files().iter().any(|file| {
            file.path == "packets/input-plan.schema.json"
                && file.content.contains(r#""const":"inspect""#)
        }));
        Ok(())
    }

    #[test]
    fn declared_packet_without_schema_fails_before_publish()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::write(
            root.join("SKILL.md"),
            "---\nname: missing-packet\ndescription: Missing packet fixture.\n---\n# Missing\n",
        )?;
        fs::write(
            root.join("run.yaml"),
            r#"skill: missing-packet
runners:
  main:
    default: true
    steps:
      - id: main
        tool: { command: node, args: [run.mjs] }
        outputs:
          plan: object
        artifacts:
          wrap_as: packet
          packet: runx.test.missing.v1
"#,
        )?;
        fs::write(
            root.join("run.mjs"),
            "console.log(JSON.stringify({plan: {}}))\n",
        )?;

        let error = prepare_registry_publish_package(RegistryPublishPackageRequest {
            subject: root.to_str().ok_or("temporary path is not UTF-8")?,
            profile: None,
            env: &BTreeMap::new(),
            cwd: root,
        })
        .err()
        .ok_or("missing packet schema must fail")?;
        assert!(error.to_string().contains("was not found"));
        Ok(())
    }

    const ACME_OPS: &str = r#"schema: runx.connector.v1
id: acme-ops
name: Acme Ops
description: Syncs Acme items.
category: dev-tools
runtime_capability: false
auth: {none: {}}
placements: {local: {auth: [none]}}
operations:
  item.sync:
    description: Sync an item.
    verb: read
    scopes: [item.read]
    target: {template: "acme://items"}
    implementation:
      process:
        tool: acme.sync
"#;
    const ACME_SYNC_MANIFEST: &str = r#"{"schema": "runx.tool.manifest.v1", "name": "acme.sync", "version": "0.1.0", "description": "Syncs.", "source": {"command": "node", "args": ["./sync.mjs"]}}"#;
    const MARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M0 0h24v24H0z" fill="#123456"/></svg>"##;

    /// A plugin repository: `skills/sync` declares the plugin's `acme-ops`
    /// and the official `web`; the plugin also holds `acme-extra`, which the
    /// skill does not declare.
    fn plugin_repository(
        root: &std::path::Path,
    ) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        fs::write(root.join("plugin.json"), r#"{"namespace":"acme"}"#)?;
        let skill = root.join("skills/sync");
        fs::create_dir_all(&skill)?;
        fs::write(
            skill.join("SKILL.md"),
            "---\nname: sync\ndescription: Syncs Acme items.\n---\n# Sync\n",
        )?;
        fs::write(
            skill.join("run.yaml"),
            "skill: sync\nconnectors:\n  web: {operations: [page.read]}\n  acme-ops: {operations: [item.sync]}\nrunners:\n  main:\n    default: true\n    steps:\n      - id: plan\n        agent: { role: agent, task: plan }\n",
        )?;
        let connector = root.join("connectors/acme-ops");
        fs::create_dir_all(connector.join("tools/acme/sync"))?;
        fs::write(connector.join("connector.yaml"), ACME_OPS)?;
        fs::write(
            connector.join("tools/acme/sync/manifest.json"),
            ACME_SYNC_MANIFEST,
        )?;
        fs::write(
            connector.join("tools/acme/sync/sync.mjs"),
            "process.stdout.write('{}');\n",
        )?;
        fs::write(connector.join("README.md"), "# Acme Ops\n")?;
        fs::write(connector.join("mark.svg"), MARK)?;
        fs::write(connector.join("notes.txt"), "not shipped\n")?;
        let extra = root.join("connectors/acme-extra");
        fs::create_dir_all(extra.join("tools/acme/sync"))?;
        fs::write(
            extra.join("tools/acme/sync/manifest.json"),
            ACME_SYNC_MANIFEST,
        )?;
        fs::write(
            extra.join("tools/acme/sync/sync.mjs"),
            "process.stdout.write('[]');\n",
        )?;
        fs::write(
            extra.join("connector.yaml"),
            ACME_OPS
                .replace("id: acme-ops", "id: acme-extra")
                .replace("name: Acme Ops", "name: Acme Extra"),
        )?;
        Ok(skill)
    }

    fn package_at(
        dir: &std::path::Path,
    ) -> Result<super::RegistryPublishPackageParts, Box<dyn std::error::Error>> {
        Ok(
            prepare_registry_publish_package(RegistryPublishPackageRequest {
                subject: dir.to_str().ok_or("temporary path is not UTF-8")?,
                profile: None,
                env: &BTreeMap::new(),
                cwd: dir,
            })?
            .into_parts(),
        )
    }

    #[test]
    fn a_package_ships_every_declared_connector_snapshot() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let skill = plugin_repository(temp.path())?;
        let package = package_at(&skill)?;

        let files = package
            .package_files
            .iter()
            .map(|file| (file.path.as_str(), file.content.as_str()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            files.get("connectors/acme/acme-ops/connector.yaml"),
            Some(&ACME_OPS)
        );
        assert_eq!(
            files.get("connectors/acme/acme-ops/tools/acme/sync/manifest.json"),
            Some(&ACME_SYNC_MANIFEST)
        );
        assert!(files.contains_key("connectors/acme/acme-ops/tools/acme/sync/sync.mjs"));
        assert_eq!(
            files.get("connectors/acme/acme-ops/README.md"),
            Some(&"# Acme Ops\n")
        );
        assert_eq!(files.get("connectors/acme/acme-ops/mark.svg"), Some(&MARK));
        // Only what the connector is: no stray file, no undeclared connector,
        // while the official connector is pinned as a snapshot too.
        assert!(!files.contains_key("connectors/acme/acme-ops/notes.txt"));
        assert!(
            !files
                .keys()
                .any(|path| path.starts_with("connectors/acme/acme-extra/"))
        );
        assert!(files.contains_key("connectors/runx/web/connector.yaml"));

        let scope = crate::ConnectorScope::at(&skill)?;
        let acme = scope.connector("acme-ops")?;
        let web = scope.connector("web")?;
        let expected = |connector: &crate::kernel::connectors::LoadedConnector| {
            runx_contracts::connector::connector_digest(
                &connector.manifest_digest,
                connector
                    .tools
                    .iter()
                    .map(|(name, tool)| (name.as_str(), &tool.digest)),
            )
        };
        assert_eq!(
            package.metadata.connectors,
            vec![
                super::super::RegistryConnectorMetadata {
                    connector_ref: acme.connector_ref.clone(),
                    version: super::prepare_registry_connector_package(
                        &temp.path().join("connectors/acme-ops")
                    )?
                    .version,
                    manifest_digest: acme.manifest_digest.clone(),
                    connector_digest: expected(&acme)?,
                    runtime_capability: false,
                    operations: vec!["item.sync".to_owned()],
                },
                super::super::RegistryConnectorMetadata {
                    connector_ref: web.connector_ref.clone(),
                    version: package
                        .metadata
                        .connectors
                        .iter()
                        .find(|binding| binding.connector_ref == "runx/web")
                        .ok_or("web binding missing")?
                        .version
                        .clone(),
                    manifest_digest: web.manifest_digest.clone(),
                    connector_digest: expected(&web)?,
                    runtime_capability: true,
                    operations: vec!["page.read".to_owned()],
                },
            ]
        );
        // A tool is part of the connector's identity, beyond its manifest.
        let extra = scope.connector("acme-extra")?;
        assert_ne!(expected(&acme)?, expected(&extra)?);
        Ok(())
    }

    #[test]
    fn a_packaged_connector_repackages_without_drift_and_without_its_old_plugin()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let skill = plugin_repository(temp.path())?;
        let first = package_at(&skill)?;

        // Staged inside the source plugin, the installed package still reads
        // only its own connectors: its descriptor marks where it ends.
        let staged = temp.path().join("skills/staged");
        fs::create_dir_all(&staged)?;
        fs::write(staged.join("SKILL.md"), &first.markdown)?;
        fs::write(
            staged.join("run.yaml"),
            first.profile_document.as_deref().ok_or("profile missing")?,
        )?;
        for file in &first.package_files {
            let path = staged.join(&file.path);
            fs::create_dir_all(path.parent().ok_or("file has no parent")?)?;
            fs::write(path, &file.content)?;
        }
        fs::remove_dir_all(temp.path().join("connectors/acme-ops"))?;

        let second = package_at(&staged)?;
        assert_eq!(second, first);
        assert!(
            crate::ConnectorScope::at(&staged)?
                .connector("acme-extra")
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn a_plugin_connector_with_a_bad_mark_fails_the_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let skill = plugin_repository(temp.path())?;
        fs::write(
            temp.path().join("connectors/acme-ops/mark.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><script>alert(1)</script></svg>"#,
        )?;
        let error = package_at(&skill)
            .err()
            .ok_or("a scripted mark must fail the package")?;
        assert!(
            error.to_string().contains("connector acme-ops: mark.svg"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn a_declared_operation_the_connector_lacks_fails_the_package()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let skill = plugin_repository(temp.path())?;
        fs::write(
            skill.join("run.yaml"),
            "skill: sync\nconnectors:\n  acme-ops: {operations: [item.delete]}\nrunners:\n  main:\n    default: true\n    steps:\n      - id: plan\n        agent: { role: agent, task: plan }\n",
        )?;
        let error = package_at(&skill)
            .err()
            .ok_or("an undeclared operation must fail the package")?;
        assert!(error.to_string().contains("item.delete"), "{error}");
        Ok(())
    }

    #[test]
    fn a_connector_packages_on_its_own_with_the_digests_a_skill_binds()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let skill = plugin_repository(temp.path())?;
        let connector =
            super::prepare_registry_connector_package(&temp.path().join("connectors/acme-ops"))?;

        let paths = connector
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [
                "README.md",
                "connector.yaml",
                "mark.svg",
                "plugin.json",
                "tools/acme/sync/manifest.json",
                "tools/acme/sync/sync.mjs"
            ]
        );
        assert_eq!(connector.manifest.id.as_str(), "acme-ops");
        let binding = package_at(&skill)?
            .metadata
            .connectors
            .into_iter()
            .find(|binding| binding.connector_ref.as_str() == "acme/acme-ops")
            .ok_or("binding missing")?;
        assert_eq!(connector.manifest_digest, binding.manifest_digest);
        assert_eq!(connector.connector_digest, binding.connector_digest);
        Ok(())
    }

    #[test]
    fn an_official_connector_source_tree_packages_as_the_official_connector()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../connectors/github");
        let connector = super::prepare_registry_connector_package(&directory)?;
        let official = crate::ConnectorScope::official_only()?.connector("github")?;
        assert_eq!(connector.manifest_digest, official.manifest_digest);
        // The tools shipped are the bytes in the source tree, which are the
        // bytes the runtime embeds.
        for file in connector
            .files
            .iter()
            .filter(|file| file.path.starts_with("tools/"))
        {
            assert_eq!(
                fs::read_to_string(directory.join(&file.path))?,
                file.content,
                "{}",
                file.path
            );
        }
        assert!(
            connector
                .files
                .iter()
                .any(|file| file.path == "tools/git/push/manifest.json")
        );
        Ok(())
    }
}
