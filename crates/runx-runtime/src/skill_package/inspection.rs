mod execution_closure;
mod runner;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{
    EnvironmentReadiness, EnvironmentReadinessStatus, InspectionReadiness,
    InspectionReadinessStatus, InspectionResume, JsonObject, JsonValue, RunnerInspectionEntry,
    SkillCatalog, SkillInspection, SkillInspectionSchema, SkillInspectionStatus,
};
use runx_parser::{CatalogMetadata, StepKind, ToolTarget};

use super::LoadedSkillPackage;
use execution_closure::inspect_local_execution_closure;
use execution_closure::unheld_registry_refs;
pub(crate) use execution_closure::{CaseClosureTarget, LocalExecutionClosure};
use execution_closure::{
    UnheldRegistryRefs, inspect_execution_closures, inspect_runner_execution_closure,
};
use runner::{catalog_capabilities, fixture_examples, fixture_operator_journeys, inspect_runner};
use thiserror::Error;

use crate::RuntimeError;

#[derive(Debug, Error)]
pub enum SkillInspectionError {
    #[error("connector execution binding is invalid: {0}")]
    ConnectorBinding(String),
    #[error(transparent)]
    Runtime(Box<RuntimeError>),
    #[error("skill has no runner '{runner}'")]
    RunnerNotFound { runner: String },
    #[error("runner manifest is unavailable")]
    ManifestUnavailable,
    #[error("native execution closure omitted runner {runner}")]
    ClosureMissing { runner: String },
    #[error("native execution closure {problem} for runner {runner}")]
    ClosureInvalid {
        runner: String,
        problem: &'static str,
    },
    #[error("skill package digest mismatch: expected {expected}, received {received}")]
    PackageDigestMismatch { expected: String, received: String },
    #[error("native execution closure for runner {runner} is not fully bound")]
    ClosureNotFullyBound { runner: String },
    #[error("skill execution closure digest mismatch: expected {expected}, received {received}")]
    ClosureDigestMismatch { expected: String, received: String },
    #[error("sub-skill {path} has no executable manifest")]
    SubSkillManifestMissing { path: PathBuf },
    #[error("sub-skill {path} has no selected runner {runner}")]
    SubSkillNamedRunnerMissing { path: PathBuf, runner: String },
    #[error("graph source omitted its validated graph")]
    GraphMissing,
    #[error("canonicalizing {label} {path}: {source}")]
    Canonicalize {
        label: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("sub-skill reference {reference} escapes the inspected execution closure")]
    ProfileEscape { reference: String },
    #[error("{context}: {source}")]
    Json {
        context: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

impl From<RuntimeError> for SkillInspectionError {
    fn from(error: RuntimeError) -> Self {
        Self::Runtime(Box::new(error))
    }
}

/// Project one already-validated package into the stable operator inspection
/// envelope. No source document is reparsed here. Registry dependencies are
/// resolved only when the caller supplies an admitted runtime environment.
pub fn inspect_skill_package(
    skill_path: &Path,
    selected_runner: Option<&str>,
    env: Option<&BTreeMap<String, String>>,
) -> Result<SkillInspection, SkillInspectionError> {
    let loaded = super::load_validated_skill_package(skill_path)?;
    inspect_loaded_skill_package(loaded, selected_runner, env)
}

/// A package's inspection as a caller reads it before running it: with how
/// ready its selected runner is to run here, under `authority`, by each
/// connector operation it may call and the environment it reads.
pub fn inspect_skill_readiness(
    skill_path: &Path,
    selected_runner: Option<&str>,
    authority: Option<&crate::RunAuthority>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<SkillInspection, SkillInspectionError> {
    let mut inspection = inspect_skill_package(skill_path, selected_runner, Some(env))?;
    let Some(requirements) = inspection
        .runner
        .as_ref()
        .map(|runner| runner.requirements.clone())
    else {
        return Ok(inspection);
    };
    let readiness = inspect_connector_readiness(skill_path, selected_runner, authority, env, cwd)?;
    if !readiness.is_empty() {
        inspection.readiness = Some(InspectionReadiness {
            status: if readiness.iter().all(|operation| operation.ready) {
                InspectionReadinessStatus::Ready
            } else {
                InspectionReadinessStatus::NeedsConnectorSetup
            },
        });
        inspection.connector_readiness = readiness;
    }
    if let Some(environment) = environment_readiness(&requirements, env) {
        if environment.status != EnvironmentReadinessStatus::Ready {
            inspection.readiness = Some(InspectionReadiness {
                status: InspectionReadinessStatus::NeedsEnvironment,
            });
        }
        inspection.environment = Some(environment);
    }
    Ok(inspection)
}

/// The environment a runner reads, by name and availability, never by
/// value; none when it reads none.
fn environment_readiness(
    requirements: &runx_contracts::ExecutionRequirements,
    env: &BTreeMap<String, String>,
) -> Option<EnvironmentReadiness> {
    if requirements.environment.is_empty() {
        return None;
    }
    let variables = crate::environment_requirement_statuses(&requirements.environment, env);
    let missing = variables
        .iter()
        .filter(|status| status.required && !status.available)
        .map(|status| status.name.clone())
        .collect::<Vec<_>>();
    Some(EnvironmentReadiness {
        status: if missing.is_empty() {
            EnvironmentReadinessStatus::Ready
        } else {
            EnvironmentReadinessStatus::NeedsEnvironment
        },
        variables,
        missing,
    })
}

/// The readiness of each connector operation a run of the selected runner may
/// call, its child skills' included, projected by the kernel from the
/// instances and evidence present for `cwd`, under the authority a host would
/// start the run with.
pub fn inspect_connector_readiness(
    skill_path: &Path,
    selected_runner: Option<&str>,
    authority: Option<&crate::RunAuthority>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<runx_contracts::Readiness>, SkillInspectionError> {
    let loaded = Arc::new(super::load_validated_skill_package(skill_path)?);
    let Some(runner) =
        inspected_runner(loaded.manifest(), selected_runner)?.map(|runner| runner.name.clone())
    else {
        return Ok(Vec::new());
    };
    let mut readiness = Vec::new();
    for (package, runner) in
        execution_closure::inspect_runner_closure_runners(Arc::clone(&loaded), &runner, env)?
    {
        let Some(manifest) = package.manifest() else {
            continue;
        };
        let Some(runner) = manifest.runners.get(&runner) else {
            continue;
        };
        let projected = crate::kernel::admission::skill_readiness(
            &package.directory,
            &runner_requirements(&manifest.connectors, runner),
            authority,
            env,
            cwd,
        )
        .map_err(|message| {
            SkillInspectionError::Runtime(Box::new(RuntimeError::SkillFailed {
                skill_name: "connectors".to_owned(),
                message,
            }))
        })?;
        for operation in projected {
            if !readiness.contains(&operation) {
                readiness.push(operation);
            }
        }
    }
    Ok(readiness)
}

/// The requirements a runner's operation steps use, narrowed to the
/// operations they call. A step that binds its operation at run time may call
/// any operation its requirement lists.
fn runner_requirements(
    connectors: &BTreeMap<String, runx_parser::ConnectorRequirement>,
    runner: &runx_parser::SkillRunnerDefinition,
) -> BTreeMap<String, runx_parser::ConnectorRequirement> {
    let mut used = BTreeMap::<String, runx_parser::ConnectorRequirement>::new();
    let targets = runner
        .graph
        .steps
        .iter()
        .filter_map(runx_parser::GraphStep::operation);
    for target in targets {
        let Some(declared) = connectors.get(&target.requirement) else {
            continue;
        };
        let requirement = used.entry(target.requirement.clone()).or_insert_with(|| {
            runx_parser::ConnectorRequirement {
                operations: Vec::new(),
                ..declared.clone()
            }
        });
        let operations = target
            .operation
            .as_ref()
            .map_or(declared.operations.as_slice(), std::slice::from_ref);
        for operation in operations {
            if !requirement.operations.contains(operation) {
                requirement.operations.push(operation.clone());
            }
        }
    }
    used
}

/// The runner inspection describes: the named one, else the default.
fn inspected_runner<'a>(
    manifest: Option<&'a runx_parser::SkillRunnerManifest>,
    selected_runner: Option<&str>,
) -> Result<Option<&'a runx_parser::SkillRunnerDefinition>, SkillInspectionError> {
    Ok(match (manifest, selected_runner) {
        (Some(manifest), Some(name)) => Some(manifest.runners.get(name).ok_or_else(|| {
            SkillInspectionError::RunnerNotFound {
                runner: name.to_owned(),
            }
        })?),
        (Some(manifest), None) => manifest
            .runners
            .values()
            .find(|runner| runner.default)
            .or_else(|| {
                (manifest.runners.len() == 1)
                    .then(|| manifest.runners.values().next())
                    .flatten()
            }),
        (None, Some(name)) => {
            return Err(SkillInspectionError::RunnerNotFound {
                runner: name.to_owned(),
            });
        }
        (None, None) => None,
    })
}

pub(crate) fn inspect_loaded_skill_package(
    loaded: LoadedSkillPackage,
    selected_runner: Option<&str>,
    env: Option<&BTreeMap<String, String>>,
) -> Result<SkillInspection, SkillInspectionError> {
    let loaded = Arc::new(loaded);
    let mut output = base_inspection(&loaded)?;
    let manifest = loaded.manifest();
    let runner = inspected_runner(manifest, selected_runner)?;
    // Inspection reports the registry refs its registry directory lacks, so a
    // host can supply them and inspect again; binding below rejects them.
    let mut execution_closures =
        inspect_execution_closures(loaded.clone(), env, UnheldRegistryRefs::Report)?;
    let mut closure_of = |runner: &str| -> Result<JsonObject, SkillInspectionError> {
        match execution_closures.remove(runner) {
            Some(JsonValue::Object(closure)) => Ok(closure),
            Some(_) => Err(SkillInspectionError::ClosureInvalid {
                runner: runner.to_owned(),
                problem: "is not an object",
            }),
            None => Err(SkillInspectionError::ClosureMissing {
                runner: runner.to_owned(),
            }),
        }
    };
    if let Some(manifest) = manifest {
        for definition in manifest.runners.values() {
            let examples = super::effective_runner_examples(&loaded.package, manifest, definition);
            let execution_closure = closure_of(&definition.name)?;
            if runner.is_some_and(|runner| runner.name == definition.name) {
                output.execution_closure = Some(execution_closure.clone());
            }
            output.runner_inspections.push(RunnerInspectionEntry {
                runner: inspect_runner(manifest, definition, &examples)?,
                execution_closure,
            });
        }
    }
    if let Some(runner) = runner {
        let manifest = manifest.ok_or(SkillInspectionError::ManifestUnavailable)?;
        let examples = super::effective_runner_examples(&loaded.package, manifest, runner);
        output.runner = Some(inspect_runner(manifest, runner, &examples)?);
        output.readiness = Some(InspectionReadiness {
            status: InspectionReadinessStatus::Ready,
        });
        output.examples = Some(fixture_examples(
            &loaded.package,
            loaded.manifest(),
            &runner.name,
        ));
        output.resume = Some(InspectionResume {
            may_pause: runner.graph.steps.iter().any(|step| {
                !matches!(
                    step.kind,
                    StepKind::Tool(ToolTarget::Worker(_) | ToolTarget::Process(_))
                )
            }),
            command: "runx resume <run-id> answers.json".to_owned(),
        });
    }
    Ok(output)
}

pub(crate) struct InspectedExecutionClosureBinding {
    pub(crate) digest: String,
    pub(crate) fully_bound: bool,
    /// Every package the closure binds, as it was loaded, by canonical
    /// directory.
    pub(crate) packages: BTreeMap<PathBuf, Arc<LoadedSkillPackage>>,
}

pub(crate) fn inspect_loaded_execution_closure_binding(
    loaded: Arc<LoadedSkillPackage>,
    selected_runner: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<InspectedExecutionClosureBinding, SkillInspectionError> {
    let (closure, packages) = inspect_runner_execution_closure(loaded, selected_runner, env)?;
    let closure = closure
        .as_object()
        .ok_or_else(|| SkillInspectionError::ClosureInvalid {
            runner: selected_runner.to_owned(),
            problem: "is not an object",
        })?;
    let fully_bound = closure
        .get("fully_bound")
        .and_then(JsonValue::as_bool)
        .ok_or_else(|| SkillInspectionError::ClosureInvalid {
            runner: selected_runner.to_owned(),
            problem: "omitted binding state",
        })?;
    let digest = closure
        .get("closure_digest")
        .and_then(JsonValue::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| SkillInspectionError::ClosureInvalid {
            runner: selected_runner.to_owned(),
            problem: "omitted digest",
        })?;
    Ok(InspectedExecutionClosureBinding {
        digest,
        fully_bound,
        packages,
    })
}

/// The registry refs the package at `skill_path` names that neither the
/// project nor a named local registry holds: what `runx skill` fetches before
/// the run.
pub fn unheld_registry_refs_at(
    skill_path: &Path,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<String>, SkillInspectionError> {
    let loaded = super::load_validated_skill_package(skill_path)?;
    unheld_loaded_registry_refs(&loaded, env)
}

/// The registry refs a loaded package names that the project and `env`'s
/// registry directory do not hold.
pub(crate) fn unheld_loaded_registry_refs(
    loaded: &LoadedSkillPackage,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<String>, SkillInspectionError> {
    unheld_registry_refs(Arc::new(loaded.clone()), env).map(|refs| refs.into_iter().collect())
}

pub(crate) fn inspect_loaded_local_execution_closure(
    loaded: &LoadedSkillPackage,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<LocalExecutionClosure, SkillInspectionError> {
    inspect_local_execution_closure(Arc::new(loaded.clone()), env)
}

/// Every connector a harness case's run binds through its whole closure.
pub(crate) fn case_closure_connectors(
    loaded: &LoadedSkillPackage,
    target: CaseClosureTarget<'_>,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<Arc<crate::kernel::connectors::LoadedConnector>>, SkillInspectionError> {
    execution_closure::case_closure_connectors(Arc::new(loaded.clone()), target, env)
}

fn base_inspection(loaded: &LoadedSkillPackage) -> Result<SkillInspection, SkillInspectionError> {
    let package = &loaded.package;
    let manifest = loaded.manifest();
    let semantic_report = manifest
        .map(|manifest| {
            JsonValue::from_serialize(&runx_parser::analyze_package_catalog_semantics(
                &package.skill.name,
                manifest,
                &package.harness_fixtures,
            ))
            .map_err(|error| SkillInspectionError::Json {
                context: "projecting catalog semantic report",
                source: serde::ser::Error::custom(error),
            })
        })
        .transpose()?;
    Ok(SkillInspection {
        schema: SkillInspectionSchema::V1,
        status: SkillInspectionStatus::Ok,
        name: package.skill.name.clone(),
        skill_path: loaded.directory.to_string_lossy().into_owned(),
        manual_digest: package.manual_digest.clone(),
        package_digest: package.package_digest.clone(),
        description: package.skill.description.clone(),
        version: manifest.and_then(|manifest| manifest.version.clone()),
        capabilities: manifest
            .and_then(|manifest| manifest.catalog.as_ref())
            .and_then(catalog_capabilities),
        catalog: manifest
            .and_then(|manifest| manifest.catalog.as_ref())
            .map(inspect_catalog),
        semantic_report,
        runners: manifest
            .map(|manifest| manifest.runners.keys().cloned().collect())
            .unwrap_or_default(),
        operator_journeys: manifest
            .map(|manifest| fixture_operator_journeys(package, manifest))
            .unwrap_or_default(),
        runner_inspections: Vec::new(),
        runner: None,
        execution_closure: None,
        readiness: None,
        connector_readiness: Vec::new(),
        environment: None,
        examples: None,
        resume: None,
        entry_provenance: None,
    })
}

fn inspect_catalog(catalog: &CatalogMetadata) -> SkillCatalog {
    SkillCatalog {
        kind: catalog.kind.as_str().to_owned(),
        audience: catalog.audience.as_str().to_owned(),
        visibility: catalog.visibility.as_str().to_owned(),
        role: catalog.role.as_str().to_owned(),
        canonical_skill: catalog.canonical_skill.clone(),
        provider: catalog.provider.clone(),
        part_of: catalog.part_of.clone(),
        execution: catalog.execution.map(|value| value.as_str().to_owned()),
        completion: catalog.completion.map(|value| value.as_str().to_owned()),
        approval: catalog.approval.map(|value| value.as_str().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use runx_contracts::{JsonObject, JsonValue};

    /// The inspection as its JSON, as a host reads it.
    fn inspect_skill_package(
        root: &std::path::Path,
        runner: Option<&str>,
        env: Option<&std::collections::BTreeMap<String, String>>,
    ) -> Result<JsonValue, Box<dyn std::error::Error>> {
        Ok(serde_json::from_value(serde_json::to_value(
            super::inspect_skill_package(root, runner, env)?,
        )?)?)
    }

    #[test]
    fn environment_readiness_names_variables_without_their_values() {
        let requirements = runx_contracts::ExecutionRequirements {
            scopes: Vec::new(),
            environment: runx_contracts::EnvironmentRequirements {
                required: vec!["REGION".to_owned()],
                optional: vec!["TRACE_LABEL".to_owned()],
            },
        };
        let readiness = super::environment_readiness(
            &requirements,
            &std::collections::BTreeMap::from([(
                "TRACE_LABEL".to_owned(),
                "do-not-render".to_owned(),
            )]),
        )
        .expect("a runner that reads the environment has readiness");
        assert_eq!(
            readiness.status,
            runx_contracts::EnvironmentReadinessStatus::NeedsEnvironment
        );
        assert_eq!(readiness.missing, ["REGION"]);
        let encoded = serde_json::to_string(&readiness).expect("readiness encodes");
        assert!(!encoded.contains("do-not-render"), "{encoded}");
    }

    const ROOT_MANUAL: &str =
        "---\nname: root\ndescription: Root inspection fixture.\n---\n\n# Root\n";
    const CHILD_MANUAL: &str =
        "---\nname: child\ndescription: Child inspection fixture.\n---\n\n# Child\n";
    const ROOT_MANIFEST: &str = r#"
skill: root
version: "0.1.0"
runners:
  inspect:
    default: true
    result_from: [child]
    steps:
      - id: child
        skill: child
  alternate:
    result_from: [child]
    steps:
      - id: child
        skill: child
"#;
    const CHILD_MANIFEST: &str = r#"
skill: child
version: "0.1.0"
runners:
  read:
    default: true
    result_from: [digest]
    steps:
      - id: digest
        tool: data.digest
        inputs:
          value: inspected
"#;

    #[test]
    fn execution_closure_uses_validated_names_and_transitive_native_tools()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir().expect("temporary skill catalog");
        let root = temp.path().join("root");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("child skill directory");
        fs::write(root.join("SKILL.md"), ROOT_MANUAL).expect("root manual");
        fs::write(root.join("run.yaml"), ROOT_MANIFEST).expect("root manifest");
        fs::write(child.join("SKILL.md"), CHILD_MANUAL).expect("child manual");
        fs::write(child.join("run.yaml"), CHILD_MANIFEST).expect("child manifest");

        let inspected = inspect_skill_package(&root, None, None).expect("valid inspection");
        let JsonValue::Object(inspected) = inspected else {
            return Err("inspection should be an object".into());
        };
        let closure = inspected
            .get("execution_closure")
            .and_then(JsonValue::as_object)
            .expect("execution closure");
        assert_eq!(
            closure.get("summary").and_then(JsonValue::as_str),
            Some("tool:data.digest")
        );
        assert_eq!(
            closure.get("skill_edges"),
            Some(&JsonValue::Array(vec![JsonValue::String(
                "child#read".to_owned()
            )]))
        );
        assert_eq!(
            closure.get("direct_external_skill_edges"),
            Some(&JsonValue::Array(Vec::new()))
        );
        assert_eq!(
            closure.get("fully_bound").and_then(JsonValue::as_bool),
            Some(true)
        );
        assert_eq!(
            closure.get("runtime_release").and_then(JsonValue::as_str),
            Some(crate::EXECUTION_RUNTIME_RELEASE)
        );
        assert!(
            closure
                .get("closure_digest")
                .and_then(JsonValue::as_str)
                .is_some_and(|value| value.starts_with("sha256:"))
        );
        assert_eq!(
            closure
                .get("package_bindings")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            closure.get("profiles"),
            Some(&JsonValue::Array(vec![
                JsonValue::String("child/run.yaml#read".to_owned()),
                JsonValue::String("run.yaml#inspect".to_owned()),
            ]))
        );
        let runner_names = inspected
            .get("runner_inspections")
            .and_then(JsonValue::as_array)
            .expect("runner inspections")
            .iter()
            .filter_map(JsonValue::as_object)
            .filter_map(|entry| entry.get("runner"))
            .filter_map(JsonValue::as_object)
            .filter_map(|runner| runner.get("name"))
            .filter_map(JsonValue::as_str)
            .collect::<Vec<_>>();
        assert_eq!(runner_names, vec!["alternate", "inspect"]);
        Ok(())
    }

    /// A hosted connector a vendor bundles, named `id`.
    fn hosted_connector(id: &str) -> String {
        format!(
            r#"schema: runx.connector.v1
id: {id}
name: Acme
description: Write one note to Acme.
runtime_capability: false
auth:
  credentials:
    service_token:
      kind: api_key
      fields:
        - {{name: token, label: Acme service token}}
      delivery: {{env: ACME_SERVICE_TOKEN}}
      audience: [notes.acme.test]
placements:
  hosted: {{auth: credential, credential: service_token}}
operations:
  notes.write:
    description: Write one note.
    verb: create
    scopes: [notes.write]
    credentials: [service_token]
    target:
      template: "acme:notes"
    guarantees:
      provider_idempotency: inherent
      readback: unavailable
    input_schema:
      type: object
      required: [value]
      additionalProperties: false
      properties:
        value: {{type: string}}
    result_fields: [note]
    hosted: {{}}
    implementation:
      http:
        allowed_hosts: [notes.acme.test]
        auth: {{service_token: {{type: bearer, field: token}}}}
        requests:
          - method: POST
            url: https://notes.acme.test/v1/notes
            headers: {{accept: application/json}}
            body:
              value: {{"$argument": "/value"}}
        result_paths: {{note: acknowledgement.note}}
"#
        )
    }

    /// Each operation readiness projects, as `connector operation`.
    fn projected(
        skill: &std::path::Path,
        workspace: &std::path::Path,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        let env = std::collections::BTreeMap::from([
            (
                "RUNX_HOME".to_owned(),
                workspace.join("home").display().to_string(),
            ),
            (
                "RUNX_PROJECT_DIR".to_owned(),
                workspace.join("project").display().to_string(),
            ),
        ]);
        Ok(
            super::inspect_connector_readiness(skill, None, None, &env, workspace)?
                .into_iter()
                .map(|operation| {
                    format!(
                        "{} {}",
                        operation
                            .connector
                            .map(|name| name.to_string())
                            .unwrap_or_default(),
                        operation.operation
                    )
                })
                .collect(),
        )
    }

    #[test]
    fn readiness_covers_the_operations_a_child_skill_calls()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir().expect("temporary skill catalog");
        let root = temp.path().join("root");
        let child = root.join("child");
        fs::create_dir_all(child.join("connectors/acme/notes")).expect("child connector directory");
        fs::write(root.join("SKILL.md"), ROOT_MANUAL).expect("root manual");
        fs::write(
            root.join("run.yaml"),
            "skill: root\nversion: \"0.1.0\"\nrunners:\n  compose:\n    default: true\n    result_from: [child]\n    steps:\n      - id: child\n        skill: child\n",
        )
        .expect("root manifest");
        fs::write(child.join("SKILL.md"), CHILD_MANUAL).expect("child manual");
        fs::write(
            child.join("connectors/acme/notes/connector.yaml"),
            hosted_connector("acme/notes"),
        )
        .expect("child connector");
        fs::write(
            child.join("run.yaml"),
            "skill: child\nversion: \"0.1.0\"\nconnectors:\n  acme/notes:\n    operations: [notes.write]\nrunners:\n  write:\n    default: true\n    steps:\n      - id: write\n        operation: acme/notes/notes.write\n        result_fields: [note]\n        idempotency_key: $run.id\n        input:\n          value: written\n",
        )
        .expect("child manifest");

        // The root calls no operation itself; its child's is what a run admits.
        assert_eq!(projected(&root, temp.path())?, ["acme/notes notes.write"]);
        Ok(())
    }

    #[test]
    fn readiness_covers_every_connector_a_run_may_choose() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir().expect("temporary skill catalog");
        let root = temp.path().join("chooser");
        for id in ["acme/notes", "acme/drafts"] {
            fs::create_dir_all(root.join("connectors").join(id)).expect("connector directory");
            fs::write(
                root.join("connectors").join(id).join("connector.yaml"),
                hosted_connector(id),
            )
            .expect("connector");
        }
        fs::write(
            root.join("SKILL.md"),
            ROOT_MANUAL.replace("root", "chooser"),
        )
        .expect("manual");
        fs::write(
            root.join("run.yaml"),
            "skill: chooser\nversion: \"0.1.0\"\nconnectors:\n  target:\n    operations: [notes.write]\n    connector_from: $input.target\n    allowed_connectors: [acme/notes, acme/drafts]\nrunners:\n  write:\n    default: true\n    inputs:\n      target:\n        type: string\n        required: true\n    steps:\n      - id: write\n        operation: target/notes.write\n        result_fields: [note]\n        idempotency_key: $run.id\n        input:\n          value: written\n",
        )
        .expect("manifest");

        // Either may be chosen, so neither is assumed ready.
        let mut operations = projected(&root, temp.path())?;
        operations.sort();
        assert_eq!(
            operations,
            ["acme/drafts notes.write", "acme/notes notes.write"]
        );
        Ok(())
    }

    #[test]
    fn execution_closure_pins_what_an_agent_turn_may_call() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir().expect("temporary skill catalog");
        let root = temp.path().join("root");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("child skill directory");
        fs::write(root.join("SKILL.md"), ROOT_MANUAL).expect("root manual");
        fs::write(
            root.join("run.yaml"),
            "skill: root\ntools: [data.digest]\nskills: [child]\nrunners:\n  answer:\n    default: true\n    outputs:\n      answer: string\n",
        )
        .expect("root manifest");
        fs::write(child.join("SKILL.md"), CHILD_MANUAL).expect("child manual");
        fs::write(child.join("run.yaml"), CHILD_MANIFEST).expect("child manifest");
        let closure = |root: &std::path::Path| -> Result<JsonObject, Box<dyn std::error::Error>> {
            let JsonValue::Object(mut inspected) = inspect_skill_package(root, None, None)? else {
                return Err("inspection should be an object".into());
            };
            match inspected.remove("execution_closure") {
                Some(JsonValue::Object(closure)) => Ok(closure),
                _ => Err("execution closure".into()),
            }
        };

        let first = closure(&root)?;
        assert_eq!(
            first.get("skill_edges"),
            Some(&JsonValue::Array(vec![JsonValue::String(
                "child#read".to_owned()
            )]))
        );
        assert_eq!(
            first.get("summary").and_then(JsonValue::as_str),
            Some("tool:data.digest; 1 agent act")
        );
        assert_eq!(
            first
                .get("package_bindings")
                .and_then(JsonValue::as_array)
                .map(Vec::len),
            Some(2),
            "the child the turn may call is bound by its package digest"
        );
        fs::write(
            child.join("SKILL.md"),
            format!("{CHILD_MANUAL}\nChanged.\n"),
        )
        .expect("changed child manual");
        assert_ne!(
            closure(&root)?.get("closure_digest"),
            first.get("closure_digest"),
            "changing the child the turn may call changes the closure"
        );
        Ok(())
    }

    #[test]
    fn execution_closure_projects_transitive_environment_requirements()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir().expect("temporary skill catalog");
        let root = temp.path().join("root");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("child skill directory");
        fs::write(root.join("SKILL.md"), ROOT_MANUAL).expect("root manual");
        fs::write(
            root.join("run.yaml"),
            r#"
skill: root
runners:
  inspect:
    default: true
    result_from: [inline]
    steps:
      - id: inline
        tool: { module: inline.mjs, environment: { required: [ROOT_REQUIRED], optional: [ROOT_OPTIONAL, SHARED] } }
        outputs: { value: string }
      - id: child
        skill: child
"#,
        )
        .expect("root manifest");
        fs::write(
            root.join("inline.mjs"),
            "export default () => ({ value: 'root' });\n",
        )
        .expect("root module");
        fs::write(child.join("SKILL.md"), CHILD_MANUAL).expect("child manual");
        fs::write(
            child.join("run.yaml"),
            r#"
skill: child
runners:
  read:
    default: true
    steps:
      - id: read
        tool: { module: child.mjs, environment: { required: [CHILD_REQUIRED, SHARED], optional: [CHILD_OPTIONAL] } }
        outputs: { value: string }
"#,
        )
        .expect("child manifest");
        fs::write(
            child.join("child.mjs"),
            "export default () => ({ value: 'child' });\n",
        )
        .expect("child module");

        let inspected = inspect_skill_package(&root, None, None).expect("valid inspection");
        let environment = inspected
            .as_object()
            .and_then(|value| value.get("execution_closure"))
            .and_then(JsonValue::as_object)
            .and_then(|value| value.get("environment"))
            .and_then(JsonValue::as_object)
            .expect("closure environment requirements");
        assert_eq!(
            environment.get("required"),
            Some(&JsonValue::Array(vec![
                JsonValue::String("CHILD_REQUIRED".to_owned()),
                JsonValue::String("ROOT_REQUIRED".to_owned()),
                JsonValue::String("SHARED".to_owned()),
            ]))
        );
        assert_eq!(
            environment.get("optional"),
            Some(&JsonValue::Array(vec![
                JsonValue::String("CHILD_OPTIONAL".to_owned()),
                JsonValue::String("ROOT_OPTIONAL".to_owned()),
            ]))
        );
        Ok(())
    }

    #[test]
    fn execution_closure_distinguishes_direct_external_skills_from_private_stages()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir().expect("temporary skill catalog");
        let root = temp.path().join("root");
        let internal = root.join("internal");
        let external = temp.path().join("research");
        fs::create_dir_all(&internal).expect("internal skill directory");
        fs::create_dir_all(&external).expect("external skill directory");
        fs::write(root.join("SKILL.md"), ROOT_MANUAL).expect("root manual");
        fs::write(
            root.join("run.yaml"),
            r#"
skill: root
runners:
  inspect:
    default: true
    result_from: [internal, research]
    steps:
      - id: internal
        skill: internal
      - id: research
        skill: ../research
        runner: brief
"#,
        )
        .expect("root manifest");
        for (directory, name, runner) in [
            (&internal, "internal", "read"),
            (&external, "research", "brief"),
        ] {
            fs::write(
                directory.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Inspection fixture.\n---\n\n# {name}\n"),
            )
            .expect("child manual");
            fs::write(
                directory.join("run.yaml"),
                format!(
                    "skill: {name}\nrunners:\n  {runner}:\n    default: true\n    result_from: [digest]\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: inspected\n"
                ),
            )
            .expect("child manifest");
        }

        let inspected = inspect_skill_package(&root, None, None).expect("valid inspection");
        let closure = inspected
            .as_object()
            .and_then(|value| value.get("execution_closure"))
            .and_then(JsonValue::as_object)
            .expect("execution closure");
        assert_eq!(
            closure.get("direct_external_skill_edges"),
            Some(&JsonValue::Array(vec![JsonValue::Object(
                JsonObject::from([
                    ("runner".to_owned(), JsonValue::String("brief".to_owned())),
                    ("skill".to_owned(), JsonValue::String("research".to_owned())),
                ])
            )]))
        );
        Ok(())
    }
}
