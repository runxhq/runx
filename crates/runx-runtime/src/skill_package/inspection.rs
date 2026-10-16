mod execution_closure;
mod runner;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{JsonObject, JsonValue};
use runx_parser::{CatalogMetadata, SkillRunnerDefinition, StepKind, ToolTarget};

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
) -> Result<JsonValue, SkillInspectionError> {
    let loaded = super::load_validated_skill_package(skill_path)?;
    inspect_loaded_skill_package(loaded, selected_runner, env)
}

/// The readiness of each connector operation the selected runner calls,
/// projected by the kernel from the instances and evidence present for `cwd`,
/// under the authority a host would start the run with.
pub fn inspect_connector_readiness(
    skill_path: &Path,
    selected_runner: Option<&str>,
    authority: Option<&crate::RunAuthority>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<runx_contracts::Readiness>, SkillInspectionError> {
    let loaded = super::load_validated_skill_package(skill_path)?;
    let manifest = loaded.manifest();
    let Some((manifest, runner)) = manifest.zip(inspected_runner(manifest, selected_runner)?)
    else {
        return Ok(Vec::new());
    };
    crate::kernel::admission::skill_readiness(
        &loaded.directory,
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
    })
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
) -> Result<JsonValue, SkillInspectionError> {
    let loaded = Arc::new(loaded);
    let mut output = base_inspection(&loaded)?;
    let manifest = loaded.manifest();
    let runner = inspected_runner(manifest, selected_runner)?;
    // Inspection reports the registry refs its registry directory lacks, so a
    // host can supply them and inspect again; binding below rejects them.
    let mut execution_closures =
        inspect_execution_closures(loaded.clone(), env, UnheldRegistryRefs::Report)?;
    if let Some(manifest) = manifest {
        output.insert(
            "runner_inspections".to_owned(),
            JsonValue::Array(
                manifest
                    .runners
                    .values()
                    .map(|runner| {
                        let examples =
                            super::effective_runner_examples(&loaded.package, manifest, runner);
                        let closure =
                            execution_closures
                                .get(&runner.name)
                                .cloned()
                                .ok_or_else(|| SkillInspectionError::ClosureMissing {
                                    runner: runner.name.clone(),
                                })?;
                        Ok(JsonValue::Object(JsonObject::from([
                            (
                                "runner".to_owned(),
                                inspect_runner(manifest, runner, &examples)?,
                            ),
                            ("execution_closure".to_owned(), closure),
                        ])))
                    })
                    .collect::<Result<Vec<_>, SkillInspectionError>>()?,
            ),
        );
    }
    if let Some(runner) = runner {
        let closure = execution_closures.remove(&runner.name).ok_or_else(|| {
            SkillInspectionError::ClosureMissing {
                runner: runner.name.clone(),
            }
        })?;
        append_runner_inspection(
            &mut output,
            &loaded,
            manifest.ok_or(SkillInspectionError::ManifestUnavailable)?,
            runner,
            closure,
        )?;
    }
    Ok(JsonValue::Object(output))
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

fn base_inspection(loaded: &LoadedSkillPackage) -> Result<JsonObject, SkillInspectionError> {
    let package = &loaded.package;
    let mut output = JsonObject::from([
        (
            "schema".to_owned(),
            JsonValue::String("runx.skill.inspect.v1".to_owned()),
        ),
        ("status".to_owned(), JsonValue::String("ok".to_owned())),
        (
            "name".to_owned(),
            JsonValue::String(package.skill.name.clone()),
        ),
        (
            "skill_path".to_owned(),
            JsonValue::String(loaded.directory.to_string_lossy().into_owned()),
        ),
        (
            "manual_digest".to_owned(),
            JsonValue::String(package.manual_digest.clone()),
        ),
        // The execution package digest, as a run records it; a listing's
        // package_digest is the registry's digest of the published files.
        (
            "package_digest".to_owned(),
            JsonValue::String(package.package_digest.clone()),
        ),
    ]);
    if let Some(description) = &package.skill.description {
        output.insert(
            "description".to_owned(),
            JsonValue::String(description.clone()),
        );
    }
    if let Some(manifest) = loaded.manifest() {
        if let Some(version) = &manifest.version {
            output.insert("version".to_owned(), JsonValue::String(version.clone()));
        }
        if let Some(capabilities) = manifest.catalog.as_ref().and_then(catalog_capabilities) {
            output.insert("capabilities".to_owned(), capabilities);
        }
        if let Some(catalog) = manifest.catalog.as_ref() {
            output.insert("catalog".to_owned(), inspect_catalog(catalog));
        }
        let semantic_report = runx_parser::analyze_package_catalog_semantics(
            &package.skill.name,
            manifest,
            &package.harness_fixtures,
        );
        let encoded =
            serde_json::to_vec(&semantic_report).map_err(|source| SkillInspectionError::Json {
                context: "serializing catalog semantic report",
                source,
            })?;
        output.insert(
            "semantic_report".to_owned(),
            serde_json::from_slice(&encoded).map_err(|source| SkillInspectionError::Json {
                context: "projecting catalog semantic report",
                source,
            })?,
        );
        output.insert(
            "runners".to_owned(),
            JsonValue::Array(
                manifest
                    .runners
                    .keys()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        );
        output.insert(
            "operator_journeys".to_owned(),
            JsonValue::Array(fixture_operator_journeys(package, manifest)),
        );
    } else {
        output.insert("runners".to_owned(), JsonValue::Array(Vec::new()));
        output.insert("operator_journeys".to_owned(), JsonValue::Array(Vec::new()));
    }
    Ok(output)
}

fn append_runner_inspection(
    output: &mut JsonObject,
    loaded: &LoadedSkillPackage,
    manifest: &runx_parser::SkillRunnerManifest,
    runner: &SkillRunnerDefinition,
    execution_closure: JsonValue,
) -> Result<(), SkillInspectionError> {
    let examples = super::effective_runner_examples(&loaded.package, manifest, runner);
    output.insert(
        "runner".to_owned(),
        inspect_runner(manifest, runner, &examples)?,
    );
    output.insert("execution_closure".to_owned(), execution_closure);
    output.insert(
        "readiness".to_owned(),
        JsonValue::Object(JsonObject::from([(
            "status".to_owned(),
            JsonValue::String("ready".to_owned()),
        )])),
    );
    output.insert(
        "examples".to_owned(),
        JsonValue::Array(fixture_examples(
            &loaded.package,
            loaded.manifest(),
            &runner.name,
        )),
    );
    output.insert(
        "resume".to_owned(),
        JsonValue::Object(JsonObject::from([
            (
                "may_pause".to_owned(),
                JsonValue::Bool(runner.graph.steps.iter().any(|step| {
                    !matches!(
                        step.kind,
                        StepKind::Tool(ToolTarget::Worker(_) | ToolTarget::Process(_))
                    )
                })),
            ),
            (
                "command".to_owned(),
                JsonValue::String("runx resume <run-id> answers.json".to_owned()),
            ),
        ])),
    );
    Ok(())
}

fn inspect_catalog(catalog: &CatalogMetadata) -> JsonValue {
    let mut output = JsonObject::from([
        (
            "kind".to_owned(),
            JsonValue::String(catalog.kind.as_str().to_owned()),
        ),
        (
            "audience".to_owned(),
            JsonValue::String(catalog.audience.as_str().to_owned()),
        ),
        (
            "visibility".to_owned(),
            JsonValue::String(catalog.visibility.as_str().to_owned()),
        ),
        (
            "role".to_owned(),
            JsonValue::String(catalog.role.as_str().to_owned()),
        ),
    ]);
    if let Some(canonical_skill) = &catalog.canonical_skill {
        output.insert(
            "canonical_skill".to_owned(),
            JsonValue::String(canonical_skill.clone()),
        );
    }
    if let Some(provider) = &catalog.provider {
        output.insert("provider".to_owned(), JsonValue::String(provider.clone()));
    }
    if !catalog.part_of.is_empty() {
        output.insert(
            "part_of".to_owned(),
            JsonValue::Array(
                catalog
                    .part_of
                    .iter()
                    .cloned()
                    .map(JsonValue::String)
                    .collect(),
            ),
        );
    }
    if let Some(execution) = catalog.execution {
        output.insert(
            "execution".to_owned(),
            JsonValue::String(execution.as_str().to_owned()),
        );
    }
    if let Some(completion) = catalog.completion {
        output.insert(
            "completion".to_owned(),
            JsonValue::String(completion.as_str().to_owned()),
        );
    }
    if let Some(approval) = catalog.approval {
        output.insert(
            "approval".to_owned(),
            JsonValue::String(approval.as_str().to_owned()),
        );
    }
    JsonValue::Object(output)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::fs;

    use runx_contracts::{JsonObject, JsonValue};

    use super::inspect_skill_package;

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
