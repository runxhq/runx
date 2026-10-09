// Module rationale: graph loading keeps stage, registry, and local skill resolution together.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{JsonObject, JsonValue, sha256_prefixed};
use runx_core::state_machine::SequentialGraphStepDefinition;
use runx_parser::{
    Binding, ExecutionGraph, GraphStep, Operand, SkillRunnerDefinition, SkillRunnerManifest,
    Source, parse_graph_yaml, validate_graph,
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
/// read the same values from the run's input, under the name each binds it
/// to. Values a nested step reads from its own graph's steps keep their own
/// provenance.
pub(crate) fn inherited_step_provenance(
    graph: &ExecutionGraph,
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
            for binding in &step.bindings {
                let Source::Input { path } = &binding.source else {
                    continue;
                };
                match (path.first(), binding.at.first()) {
                    (Some(runner_input), Some(key)) => {
                        inherit_provenance(parent, runner_input, key, &mut entries);
                    }
                    (Some(runner_input), None) => {
                        inherit_provenance(parent, runner_input, runner_input, &mut entries);
                    }
                    (None, key) => {
                        entries.extend(parent.iter().map(|entry| runx_contracts::ProvenanceEntry {
                            input:
                                key.map_or_else(|| entry.input.clone(), |key| key.as_str().into()),
                            ..entry.clone()
                        }))
                    }
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

/// Bind the run's input into every step: each reference to it takes its
/// value, and an absent optional input leaves itself out of the object or
/// list that holds it. References to earlier steps stay for the step to bind
/// when it runs.
pub(crate) fn bind_run_inputs(
    mut graph: ExecutionGraph,
    graph_inputs: &JsonObject,
) -> Result<ExecutionGraph, RuntimeError> {
    let run_input = JsonValue::Object(graph_inputs.clone());
    for step in &mut graph.steps {
        let bindings = std::mem::take(&mut step.bindings);
        let mut later = Vec::new();
        // Last first, so a list item left out never moves one still to bind.
        for binding in bindings.into_iter().rev() {
            let Source::Input { path } = &binding.source else {
                later.push(binding);
                continue;
            };
            let value = value_at(&run_input, path).cloned();
            place(&step.id, &mut step.inputs, &binding, value)?;
        }
        later.reverse();
        step.bindings = later;
        if let Some(when) = &mut step.when {
            bind_operand(&mut when.value, &run_input);
        }
    }
    if let Some(policy) = &mut graph.policy {
        for guard in &mut policy.guards {
            bind_operand(&mut guard.value, &run_input);
        }
    }
    Ok(graph)
}

fn bind_operand(operand: &mut Operand, run_input: &JsonValue) {
    if let Operand::Reference(Source::Input { path }) = operand {
        *operand = Operand::Literal(
            value_at(run_input, path)
                .cloned()
                .unwrap_or(JsonValue::Null),
        );
    }
}

/// The value at `path` inside `value`: object keys, and list indexes.
pub(crate) fn value_at<'a>(value: &'a JsonValue, path: &[String]) -> Option<&'a JsonValue> {
    path.iter().try_fold(value, |value, segment| match value {
        JsonValue::Object(object) => object.get(segment),
        JsonValue::Array(items) => items.get(segment.parse::<usize>().ok()?),
        _ => None,
    })
}

/// Put a bound `value` where `binding` sits in a step's values. Nothing to
/// put leaves the member or item out; the whole input must resolve.
fn place(
    step_id: &str,
    inputs: &mut JsonObject,
    binding: &Binding,
    value: Option<JsonValue>,
) -> Result<(), RuntimeError> {
    let refused = |reason: String| RuntimeError::InvalidRunStep {
        step_id: step_id.to_owned(),
        reason,
    };
    if binding.at.is_empty() {
        *inputs = match value {
            Some(JsonValue::Object(object)) => object,
            Some(_) => {
                return Err(refused(format!(
                    "{} is the step's whole input, so it must be an object",
                    binding.source.reference()
                )));
            }
            None => {
                return Err(refused(format!(
                    "{} is the step's whole input and resolves to nothing",
                    binding.source.reference()
                )));
            }
        };
        return Ok(());
    }
    let mut root = JsonValue::Object(std::mem::take(inputs));
    let placed = put(&mut root, &binding.at, value);
    if let JsonValue::Object(object) = root {
        *inputs = object;
    }
    if placed {
        Ok(())
    } else {
        Err(refused(format!(
            "{} resolves to nothing, and '{}' cannot be left out",
            binding.source.reference(),
            binding.at.join(".")
        )))
    }
}

fn put(value: &mut JsonValue, path: &[String], new: Option<JsonValue>) -> bool {
    let Some((head, rest)) = path.split_first() else {
        return false;
    };
    if rest.is_empty() {
        return match (value, new) {
            (JsonValue::Object(object), Some(new)) => {
                object.insert(head.clone(), new);
                true
            }
            (JsonValue::Object(object), None) => {
                object.remove(head);
                true
            }
            (JsonValue::Array(items), new) => match head.parse::<usize>().ok() {
                Some(index) if index < items.len() => {
                    match new {
                        Some(new) => items[index] = new,
                        None => {
                            items.remove(index);
                        }
                    }
                    true
                }
                _ => false,
            },
            _ => false,
        };
    }
    let child = match value {
        JsonValue::Object(object) => object.get_mut(head),
        JsonValue::Array(items) => head
            .parse::<usize>()
            .ok()
            .and_then(|index| items.get_mut(index)),
        _ => None,
    };
    child.is_some_and(|child| put(child, rest, new))
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

/// A step's values when it runs: each reference to an earlier step takes
/// that step's result.
pub(crate) fn materialize_step_invocation_inputs_with_index(
    step: &GraphStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<JsonObject, RuntimeError> {
    let mut inputs = step.inputs.clone();
    for binding in &step.bindings {
        let Source::Step { step: from, path } = &binding.source else {
            continue;
        };
        let value = prior_run_index.output(&step.id, &binding_label(binding), from, path)?;
        place(&step.id, &mut inputs, binding, Some(value))?;
    }
    Ok(inputs)
}

pub(crate) fn materialize_step_invocation_provenance_with_index(
    step: &GraphStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<Vec<runx_contracts::ProvenanceEntry>, RuntimeError> {
    step.bindings
        .iter()
        .filter_map(|binding| match &binding.source {
            Source::Step { step: from, path } => {
                Some(prior_run_index.provenance(&step.id, &binding_label(binding), from, path))
            }
            Source::Input { .. } => None,
        })
        .collect()
}

/// Where a binding sits, as a name for diagnostics and provenance.
fn binding_label(binding: &Binding) -> String {
    if binding.at.is_empty() {
        "inputs".to_owned()
    } else {
        binding.at.join(".")
    }
}

fn context_from(step: &GraphStep) -> Option<Vec<String>> {
    let reads = step.reads();
    (!reads.is_empty()).then(|| reads.into_iter().map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue};
    use runx_parser::{ExecutionGraph, Operand, parse_graph_yaml, validate_graph};

    use super::bind_run_inputs;

    fn graph(source: &str) -> Result<ExecutionGraph, String> {
        validate_graph(parse_graph_yaml(source).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())
    }

    fn object(json: serde_json::Value) -> JsonObject {
        serde_json::from_value(json).unwrap_or_default()
    }

    const BINDINGS: &str = r#"
name: bindings
steps:
  - id: produce
    tool: { module: produce.mjs }
    outputs: { value: string }
  - id: consume
    tool: { module: consume.mjs }
    when: {value: $input.mode, equals: apply}
    inputs:
      value: $steps.produce.value
      key: $input.retry.key
      label: $input.label
      fixed: kept
"#;

    #[test]
    fn a_step_receives_only_what_it_binds() -> Result<(), String> {
        let bound = bind_run_inputs(
            graph(BINDINGS)?,
            &object(serde_json::json!({
                "mode": "apply",
                "retry": {"key": "campaign-42"},
                "unrelated": "never reaches a step",
            })),
        )
        .map_err(|error| error.to_string())?;
        let consume = &bound.steps[1];
        assert_eq!(
            consume.inputs,
            object(serde_json::json!({
                "value": "$steps.produce.value",
                "key": "campaign-42",
                "fixed": "kept",
            })),
            "the run's input binds in place, an absent optional input is left out, and nothing else arrives"
        );
        assert_eq!(
            consume.reads(),
            ["produce"],
            "the step reference binds when the step runs"
        );
        assert_eq!(
            consume.when.as_ref().map(|when| &when.value),
            Some(&Operand::Literal(JsonValue::String("apply".to_owned())))
        );
        assert!(bound.steps[0].inputs.is_empty());
        Ok(())
    }

    #[test]
    fn a_whole_input_must_be_an_object_and_an_absent_item_leaves_itself_out() -> Result<(), String>
    {
        let whole = graph(
            "name: whole\nsteps:\n  - id: take\n    tool: { module: take.mjs }\n    inputs: $input.payload\n",
        )?;
        let error = bind_run_inputs(whole, &object(serde_json::json!({"payload": "text"})))
            .err()
            .ok_or("a whole input that is not an object binds nothing")?;
        assert!(error.to_string().contains("must be an object"), "{error}");

        let list = graph(
            "name: list\nsteps:\n  - id: take\n    tool: { module: take.mjs }\n    inputs: {ids: [$input.first, $input.second, $input.third]}\n",
        )?;
        let bound = bind_run_inputs(list, &object(serde_json::json!({"first": 1, "third": 3})))
            .map_err(|error| error.to_string())?;
        assert_eq!(
            bound.steps[0].inputs.get("ids"),
            serde_json::from_value::<JsonValue>(serde_json::json!([1, 3]))
                .ok()
                .as_ref(),
            "an absent item leaves itself out and the rest keep their order"
        );
        Ok(())
    }

    #[test]
    fn a_bound_value_is_never_read_again_as_a_reference() -> Result<(), String> {
        let bound = bind_run_inputs(
            graph(BINDINGS)?,
            &object(serde_json::json!({"mode": "apply", "label": "$steps.produce.value"})),
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            bound.steps[1].inputs.get("label"),
            Some(&JsonValue::String("$steps.produce.value".to_owned()))
        );
        assert_eq!(
            bound.steps[1].bindings.len(),
            1,
            "only the step reference remains"
        );
        Ok(())
    }
}
