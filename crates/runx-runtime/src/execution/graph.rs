// Module rationale: graph loading keeps stage, registry, and local skill resolution together.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};
use runx_core::state_machine::SequentialGraphStepDefinition;
use runx_parser::{
    ExecutionGraph, GraphStep, SkillRunnerDefinition, SkillRunnerManifest, parse_graph_yaml,
    validate_graph,
};

use crate::RuntimeError;
use crate::registry::{
    FileRegistryStore, InstallCandidate, InstallLocalSkillOptions, RegistryResolveOptions,
    install_local_skill, materialization_cache_path, materialization_digest_marker,
    resolve_registry_skill, split_skill_id, trusted_registry_manifest_keys_from_env,
};

use super::graph_index::PriorRunIndex;

#[derive(Clone)]
pub(crate) struct LoadedStepSkill {
    pub(crate) runner: SkillRunnerDefinition,
    pub(crate) requirements: runx_contracts::ExecutionRequirements,
    pub(crate) directory: PathBuf,
    pub(crate) manual_path: PathBuf,
    pub(crate) manual_markdown: Arc<str>,
    pub(crate) manual_digest: String,
    pub(crate) registry: Option<LoadedStepSkillRegistryProvenance>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadedStepSkillRegistryProvenance {
    pub(crate) reference: String,
    pub(crate) source: String,
    pub(crate) source_label: String,
    pub(crate) skill_id: String,
    pub(crate) version: String,
    pub(crate) digest: String,
    pub(crate) package_digest: Option<String>,
    pub(crate) trust_tier: String,
}

struct ResolvedStepSkillDirectory {
    directory: PathBuf,
    registry: Option<LoadedStepSkillRegistryProvenance>,
}

pub(crate) struct LoadedStepSkillPackage {
    pub(crate) package: crate::LoadedSkillPackage,
    pub(crate) registry: Option<LoadedStepSkillRegistryProvenance>,
}

#[derive(Default)]
pub(crate) struct StepSkillCache {
    loaded: BTreeMap<String, LoadedStepSkill>,
}

impl StepSkillCache {
    pub(crate) fn load(
        &mut self,
        graph_dir: &Path,
        step: &GraphStep,
        options: StepSkillLoadOptions<'_>,
    ) -> Result<LoadedStepSkill, RuntimeError> {
        if let Some(skill) = self.loaded.get(&step.id) {
            return Ok(skill.clone());
        }
        let skill = load_step_skill(graph_dir, step, options)?;
        self.loaded.insert(step.id.clone(), skill.clone());
        Ok(skill)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct StepSkillLoadOptions<'a> {
    pub(crate) env: &'a BTreeMap<String, String>,
}

pub(crate) fn load_graph(graph_path: &Path) -> Result<ExecutionGraph, RuntimeError> {
    let source = fs::read_to_string(graph_path)
        .map_err(|source| RuntimeError::io("reading graph file", source))?;
    let raw = parse_graph_yaml(&source)?;
    validate_graph(raw).map_err(RuntimeError::from)
}

/// The parent step's provenance, carried to the nested graph's steps that
/// receive the same values: an input passed ambiently under its runner-input
/// name, or bound from `$input.<name>` (renamed to the binding's key). Context
/// edges inside the nested graph keep their own provenance.
pub(crate) fn inherited_step_provenance(
    graph: &ExecutionGraph,
    graph_inputs: &JsonObject,
    parent: &[runx_contracts::ProvenanceEntry],
) -> BTreeMap<String, Vec<runx_contracts::ProvenanceEntry>> {
    if parent.is_empty() {
        return BTreeMap::new();
    }
    graph
        .steps
        .iter()
        .map(|step| {
            let mut entries = Vec::new();
            if step.operation().is_none() {
                for name in graph_inputs.keys() {
                    let overridden = step.inputs.contains_key(name)
                        || step.context_edges.iter().any(|edge| edge.input == *name);
                    if !overridden {
                        inherit_provenance(parent, name, name, &mut entries);
                    }
                }
            }
            for (key, value) in &step.inputs {
                if let Some(path) = value
                    .as_str()
                    .and_then(|value| value.strip_prefix("$input."))
                {
                    let runner_input = path.split('.').next().unwrap_or(path);
                    inherit_provenance(parent, runner_input, key, &mut entries);
                }
            }
            (step.id.clone(), entries)
        })
        .filter(|(_, entries)| !entries.is_empty())
        .collect()
}

fn inherit_provenance(
    parent: &[runx_contracts::ProvenanceEntry],
    runner_input: &str,
    input: &str,
    entries: &mut Vec<runx_contracts::ProvenanceEntry>,
) {
    entries.extend(
        parent
            .iter()
            .filter(|entry| entry.input.as_ref() == runner_input)
            .map(|entry| runx_contracts::ProvenanceEntry {
                input: input.into(),
                ..entry.clone()
            }),
    );
}

pub(crate) fn materialize_graph_parameter_inputs(
    mut graph: ExecutionGraph,
    graph_inputs: &JsonObject,
) -> ExecutionGraph {
    for step in &mut graph.steps {
        let declared_inputs = std::mem::take(&mut step.inputs);
        // An operation call is exactly what its step declares; graph
        // parameters reach it only through `$input` references.
        let mut inputs = if step.operation().is_some() {
            JsonObject::new()
        } else {
            graph_inputs.clone()
        };
        // Graph inputs are ambient graph parameters, but a context edge is the
        // explicit producer for its input name. Remove only that ambient value
        // here. The parser separately rejects a real collision between the
        // step's declared `inputs` and `context`.
        for edge in &step.context_edges {
            inputs.remove(&edge.input);
        }
        for (key, value) in &declared_inputs {
            if let Some(value) = materialize_graph_input_value(value, graph_inputs) {
                inputs.insert(key.clone(), value);
            } else {
                inputs.remove(key);
            }
        }
        step.inputs = inputs;
        step.idempotency_key = step.idempotency_key.as_deref().and_then(|value| {
            value.strip_prefix("$input.").map_or_else(
                || Some(value.to_owned()),
                |path| {
                    resolve_graph_input_path(graph_inputs, path)
                        .and_then(JsonValue::as_str)
                        .map(str::to_owned)
                        // Context edges are materialized only when their producer
                        // has run. Preserve a reference that is not an ambient
                        // graph input so the effect boundary can resolve it from
                        // the complete step invocation instead of silently
                        // discarding the idempotency contract.
                        .or_else(|| Some(value.to_owned()))
                },
            )
        });
    }
    graph
}

fn materialize_graph_input_value(
    value: &JsonValue,
    graph_inputs: &JsonObject,
) -> Option<JsonValue> {
    match value {
        JsonValue::String(value) => {
            if let Some(path) = value.strip_prefix("$input.") {
                return resolve_graph_input_path(graph_inputs, path).cloned();
            }
            Some(JsonValue::String(value.clone()))
        }
        JsonValue::Array(values) => Some(JsonValue::Array(
            values
                .iter()
                .filter_map(|value| materialize_graph_input_value(value, graph_inputs))
                .collect(),
        )),
        JsonValue::Object(object) => Some(JsonValue::Object(
            object
                .iter()
                .filter_map(|(key, value)| {
                    materialize_graph_input_value(value, graph_inputs)
                        .map(|value| (key.clone(), value))
                })
                .collect(),
        )),
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => Some(value.clone()),
    }
}

fn resolve_graph_input_path<'a>(value: &'a JsonObject, path: &str) -> Option<&'a JsonValue> {
    let mut current: Option<&JsonValue> = None;
    for segment in path.split('.') {
        current = match current {
            None => value.get(segment),
            Some(JsonValue::Object(object)) => object.get(segment),
            Some(_) => return None,
        };
    }
    current
}

pub(crate) fn load_step_skill(
    graph_dir: &Path,
    step: &GraphStep,
    options: StepSkillLoadOptions<'_>,
) -> Result<LoadedStepSkill, RuntimeError> {
    let resolved = load_step_skill_package(graph_dir, step, options)?;
    let package = resolved.package;
    let manifest = package
        .manifest()
        .ok_or_else(|| RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason: format!(
                "sub-skill {} does not declare a run.yaml runner",
                package.directory.display()
            ),
        })?;
    let runner = select_step_runner(
        manifest,
        step.skill().and_then(|target| target.runner.as_deref()),
    )?
    .clone();
    let requirements = manifest.execution_requirements(&runner);
    let directory = package.directory.clone();
    let manual_path = package.package_root.join("SKILL.md");
    let manual_markdown = package.package.manual_markdown.clone().into();
    let manual_digest = package.package.manual_digest.clone();
    let loaded = LoadedStepSkill {
        runner,
        requirements,
        directory,
        manual_path: manual_path.clone(),
        manual_markdown,
        manual_digest,
        registry: resolved.registry,
    };
    for path in [
        loaded.directory.join(runx_parser::PROFILE_FILE),
        manual_path,
    ] {
        if path.exists() {
            super::prepared_skill::verify_prepared_artifact_at_use(options.env, &path)?;
        }
    }
    Ok(loaded)
}

pub(crate) fn load_step_skill_package(
    graph_dir: &Path,
    step: &GraphStep,
    options: StepSkillLoadOptions<'_>,
) -> Result<LoadedStepSkillPackage, RuntimeError> {
    let resolved = resolve_step_skill_directory(graph_dir, step, options)?;
    Ok(LoadedStepSkillPackage {
        package: crate::load_validated_skill_package(&resolved.directory)?,
        registry: resolved.registry,
    })
}

pub(crate) fn select_step_runner<'a>(
    manifest: &'a SkillRunnerManifest,
    requested_runner: Option<&str>,
) -> Result<&'a SkillRunnerDefinition, RuntimeError> {
    if let Some(runner) = requested_runner {
        return manifest.runners.get(runner).ok_or_else(|| {
            RuntimeError::UnsupportedRunnerSelection {
                runner: runner.to_owned(),
            }
        });
    }
    let defaults = manifest
        .runners
        .values()
        .filter(|runner| runner.default)
        .collect::<Vec<_>>();
    match defaults.as_slice() {
        [runner] => Ok(*runner),
        [] if manifest.runners.len() == 1 => manifest.runners.values().next().ok_or_else(|| {
            RuntimeError::UnsupportedRunnerSelection {
                runner: "default".to_owned(),
            }
        }),
        [] => Err(RuntimeError::UnsupportedRunnerSelection {
            runner: "default".to_owned(),
        }),
        _ => Err(RuntimeError::UnsupportedRunnerSelection {
            runner: "default".to_owned(),
        }),
    }
}

pub(crate) fn step_definitions(graph: &ExecutionGraph) -> Vec<SequentialGraphStepDefinition> {
    graph
        .steps
        .iter()
        .map(|step| SequentialGraphStepDefinition {
            id: step.id.clone(),
            context_from: context_from(step),
        })
        .collect()
}

fn resolve_step_skill_directory(
    graph_dir: &Path,
    step: &GraphStep,
    options: StepSkillLoadOptions<'_>,
) -> Result<ResolvedStepSkillDirectory, RuntimeError> {
    if let Some(skill) = step.skill().map(|target| target.reference.as_str()) {
        if is_registry_step_ref(skill) {
            return materialize_registry_step_skill(graph_dir, step, skill, options);
        }
        if let Some(directory) = crate::registry::package_bundle::resolve_bundled_skill(
            graph_dir, skill,
        )
        .map_err(|reason| RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason,
        })? {
            return Ok(ResolvedStepSkillDirectory {
                directory,
                registry: None,
            });
        }
        return Ok(ResolvedStepSkillDirectory {
            directory: graph_dir.join(skill),
            registry: None,
        });
    }
    Err(RuntimeError::StepMissingSkill {
        step_id: step.id.clone(),
    })
}

// Function rationale: registry step materialization owns cache, digest, and manifest restoration.
fn materialize_registry_step_skill(
    graph_dir: &Path,
    step: &GraphStep,
    reference: &str,
    options: StepSkillLoadOptions<'_>,
) -> Result<ResolvedStepSkillDirectory, RuntimeError> {
    let Some(registry_dir) = options.env.get("RUNX_REGISTRY_DIR") else {
        return Err(RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason: format!(
                "nested skill '{reference}' is a registry ref, but RUNX_REGISTRY_DIR is not configured"
            ),
        });
    };
    let registry_url = options.env.get("RUNX_REGISTRY_URL").cloned();
    let store = FileRegistryStore::new(registry_dir);
    let resolution = resolve_registry_skill(
        &store,
        reference,
        RegistryResolveOptions {
            version: None,
            registry_url,
        },
    )
    .map_err(|source| RuntimeError::InvalidRunStep {
        step_id: step.id.clone(),
        reason: format!("nested skill registry ref '{reference}' could not be resolved: {source}"),
    })?
    .ok_or_else(|| RuntimeError::InvalidRunStep {
        step_id: step.id.clone(),
        reason: format!("nested skill registry ref '{reference}' was not found"),
    })?;

    let (owner, name) = split_skill_id(&resolution.skill_id).map_err(|source| {
        RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason: format!(
                "nested skill registry ref '{reference}' resolved to invalid skill id '{}': {source}",
                resolution.skill_id
            ),
        }
    })?;
    let profile_digest = resolution
        .profile_document
        .as_ref()
        .map(|document| sha256_prefixed(document.as_bytes()));
    let identity_digest = sha256_prefixed(
        materialization_digest_marker(
            &prefixed_digest(&resolution.digest),
            profile_digest.as_deref(),
            resolution.package_digest.as_deref(),
        )
        .as_bytes(),
    );
    let provenance = LoadedStepSkillRegistryProvenance {
        reference: reference.to_owned(),
        source: resolution.source.clone(),
        source_label: resolution.source_label.clone(),
        skill_id: resolution.skill_id.clone(),
        version: resolution.version.clone(),
        digest: prefixed_digest(&resolution.digest),
        package_digest: resolution.package_digest.clone(),
        trust_tier: registry_trust_tier_label(&resolution.trust_tier).to_owned(),
    };
    let cache_root = crate::RunxPaths::resolve(options.env, graph_dir)
        .registry_step_skills()
        .join(registry_source_fingerprint(registry_dir));
    let destination_root = materialization_cache_path(
        &cache_root,
        owner,
        name,
        &resolution.version,
        &identity_digest,
    );
    let candidate = InstallCandidate {
        markdown: resolution.markdown,
        profile_document: resolution.profile_document,
        package_files: resolution.package_files,
        package_digest: resolution.package_digest,
        source: resolution.source,
        source_label: resolution.source_label,
        r#ref: format!("{}@{}", resolution.skill_id, resolution.version),
        skill_id: Some(resolution.skill_id),
        version: Some(resolution.version),
        signed_manifest: resolution.signed_manifest,
        profile_digest: resolution.profile_digest,
        runner_names: resolution.runner_names,
        trust_tier: Some(resolution.trust_tier),
        manifest_source_authority: crate::registry::registry_manifest_source_authority_from_env(
            options.env,
        ),
    };
    let trusted_manifest_keys = trusted_registry_manifest_keys_from_env(options.env).map_err(
        |source| RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason: format!(
                "nested skill registry ref '{reference}' trust configuration is invalid: {source}"
            ),
        },
    )?;
    let install = install_local_skill(
        &candidate,
        &InstallLocalSkillOptions {
            destination_root,
            expected_digest: None,
            trusted_manifest_keys,
        },
    )
    .map_err(|source| RuntimeError::InvalidRunStep {
        step_id: step.id.clone(),
        reason: format!("nested skill registry ref '{reference}' failed admission: {source}"),
    })?;
    let directory = install
        .destination
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| RuntimeError::InvalidRunStep {
            step_id: step.id.clone(),
            reason: format!(
                "nested skill registry ref '{reference}' installed to invalid path {}",
                install.destination.display()
            ),
        })?;
    Ok(ResolvedStepSkillDirectory {
        directory,
        registry: Some(provenance),
    })
}

fn registry_source_fingerprint(registry_dir: &str) -> String {
    sha256_prefixed(registry_dir.as_bytes())
        .trim_start_matches("sha256:")
        .chars()
        .take(16)
        .collect()
}

fn prefixed_digest(digest: &str) -> String {
    if digest.starts_with("sha256:") {
        digest.to_owned()
    } else {
        format!("sha256:{digest}")
    }
}

fn registry_trust_tier_label(value: &crate::registry::TrustTier) -> &'static str {
    match value {
        crate::registry::TrustTier::FirstParty => "first_party",
        crate::registry::TrustTier::Verified => "verified",
        crate::registry::TrustTier::Community => "community",
    }
}

fn is_registry_step_ref(reference: &str) -> bool {
    reference.starts_with("registry:")
        || reference.starts_with("runx-registry:")
        || reference.starts_with("runx://skill/")
}

pub(crate) fn materialize_step_invocation_inputs_with_index(
    step: &GraphStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<JsonObject, RuntimeError> {
    let mut inputs = step.inputs.clone();
    for edge in &step.context_edges {
        let value = prior_run_index.output(&step.id, &edge.input, &edge.from_step, &edge.output)?;
        if inputs.insert(edge.input.clone(), value).is_some() {
            return Err(RuntimeError::InvalidRunStep {
                step_id: step.id.clone(),
                reason: format!(
                    "input '{}' is declared by both static inputs and context",
                    edge.input
                ),
            });
        }
    }
    Ok(inputs)
}

pub(crate) fn materialize_step_invocation_provenance_with_index(
    step: &GraphStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<Vec<runx_contracts::ProvenanceEntry>, RuntimeError> {
    step.context_edges
        .iter()
        .map(|edge| {
            prior_run_index.provenance(&step.id, &edge.input, &edge.from_step, &edge.output)
        })
        .collect()
}

fn context_from(step: &GraphStep) -> Option<Vec<String>> {
    let refs = step
        .context_edges
        .iter()
        .map(|edge| edge.from_step.clone())
        .collect::<Vec<_>>();
    (!refs.is_empty()).then_some(refs)
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue};
    use runx_parser::{ExecutionGraph, RawGraphIr, parse_graph_yaml, validate_graph};

    use super::materialize_graph_parameter_inputs;

    fn graph_with_idempotency_key(value: &str) -> Result<ExecutionGraph, String> {
        let step = JsonObject::from([
            ("id".to_owned(), JsonValue::String("mutate".to_owned())),
            (
                "tool".to_owned(),
                JsonValue::String("provider.mutate".to_owned()),
            ),
            (
                "idempotency_key".to_owned(),
                JsonValue::String(value.to_owned()),
            ),
        ]);
        let raw = RawGraphIr {
            document: JsonObject::from([
                ("name".to_owned(), JsonValue::String("example".to_owned())),
                (
                    "steps".to_owned(),
                    JsonValue::Array(vec![JsonValue::Object(step)]),
                ),
            ]),
        };
        validate_graph(raw).map_err(|error| error.to_string())
    }

    #[test]
    fn graph_input_materialization_resolves_step_idempotency_key() -> Result<(), String> {
        let graph = graph_with_idempotency_key("$input.retry.key")?;
        let inputs = JsonObject::from([(
            "retry".to_owned(),
            JsonValue::Object(JsonObject::from([(
                "key".to_owned(),
                JsonValue::String("campaign-42".to_owned()),
            )])),
        )]);

        let materialized = materialize_graph_parameter_inputs(graph, &inputs);
        let step = materialized
            .steps
            .first()
            .ok_or_else(|| "graph should have one step".to_owned())?;

        assert_eq!(step.idempotency_key.as_deref(), Some("campaign-42"));
        Ok(())
    }

    #[test]
    fn graph_input_materialization_preserves_static_step_idempotency_key() -> Result<(), String> {
        let graph = graph_with_idempotency_key("fixed-key")?;

        let materialized = materialize_graph_parameter_inputs(graph, &JsonObject::new());
        let step = materialized
            .steps
            .first()
            .ok_or_else(|| "graph should have one step".to_owned())?;

        assert_eq!(step.idempotency_key.as_deref(), Some("fixed-key"));
        Ok(())
    }

    #[test]
    fn graph_input_materialization_preserves_context_bound_idempotency_reference()
    -> Result<(), String> {
        let graph = graph_with_idempotency_key("$input.idempotency_key")?;

        let materialized = materialize_graph_parameter_inputs(graph, &JsonObject::new());
        let step = materialized
            .steps
            .first()
            .ok_or_else(|| "graph should have one step".to_owned())?;

        assert_eq!(
            step.idempotency_key.as_deref(),
            Some("$input.idempotency_key")
        );
        Ok(())
    }

    #[test]
    fn graph_context_replaces_only_ambient_runner_input() -> Result<(), String> {
        let raw = parse_graph_yaml(
            r#"
name: context-precedence
steps:
  - id: produce
    tool: { module: produce.mjs }
    outputs: { value: string }
  - id: consume
    tool: { module: consume.mjs }
    context:
      value: produce.value
"#,
        )
        .map_err(|error| error.to_string())?;
        let graph = validate_graph(raw).map_err(|error| error.to_string())?;
        let inputs =
            JsonObject::from([("value".to_owned(), JsonValue::String("ambient".to_owned()))]);

        let materialized = materialize_graph_parameter_inputs(graph, &inputs);
        let consume = materialized
            .steps
            .iter()
            .find(|step| step.id == "consume")
            .ok_or_else(|| "graph should contain consume".to_owned())?;

        assert!(!consume.inputs.contains_key("value"));
        assert_eq!(consume.context_edges.len(), 1);
        Ok(())
    }
}
