//! Registry publication preparation from parser-owned package truth.

mod bundle;
mod files;
mod harness;

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
        crate::resolve_path_from_user_input(request.subject, request.env, request.cwd, true);
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
        .map(|path| crate::resolve_path_from_user_input(&path.to_string_lossy(), env, cwd, true))
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
    let package_files = bundle::collect_bundle_files(loaded, &local_closure, env, cwd)?;
    let metadata = project_registry_package_metadata(&loaded.package.skill, loaded.manifest());
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
        assert!(package.package_files().is_empty());
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
        assert!(package_paths.contains(&"dependencies/sibling/SKILL.md"));
        assert!(package_paths.contains(&"dependencies/sibling/run.yaml"));
        for schema in ["provider-operation", "verification"] {
            assert!(
                package_paths.contains(
                    &format!("dependencies/sibling/packets/{schema}.schema.json").as_str()
                )
            );
        }
        let harness_path = package
            .harness
            .as_ref()
            .ok_or("staged harness missing")?
            .path();
        assert!(harness_path.join("dependencies/sibling/SKILL.md").is_file());
        assert!(harness_path.join("dependencies/sibling/run.yaml").is_file());
        assert!(harness_path.join("packets/sibling.schema.json").is_file());
        let report = package.run_harness(&crate::LocalOrchestrator::default(), &BTreeMap::new())?;
        assert_eq!(report.status, "passed");
        assert_eq!(report.case_names, ["sibling"]);
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
}
