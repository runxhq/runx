// Module rationale: graph loading keeps stage, registry, and local skill resolution together.
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::{CallAct, JsonObject, JsonValue};
use runx_core::state_machine::SequentialWorkflowStepDefinition;
use runx_parser::{
    Binding, Operand, SkillMethodDefinition, SkillMethodManifest, SkillTarget, Source, StepKind,
    Workflow, WorkflowStep,
};

use crate::RuntimeError;
use crate::registry::split_skill_id;

use super::workflow_index::PriorRunIndex;

#[derive(Clone)]
pub(crate) struct LoadedStepSkill {
    pub(crate) method: SkillMethodDefinition,
    /// The package's version, as its `run.yaml` states it.
    pub(crate) version: Option<String>,
    /// The offer the method is sold under, when its package sells it.
    pub(crate) offer: Option<runx_contracts::PaidSkillOfferTerms>,
    pub(crate) requirements: runx_contracts::ExecutionRequirements,
    pub(crate) directory: PathBuf,
    /// What the skill says it does, as its manual declares.
    pub(crate) description: Option<String>,
    pub(crate) manual_path: PathBuf,
    pub(crate) manual_markdown: Arc<str>,
    pub(crate) manual_digest: String,
    pub(crate) registry: Option<LoadedStepSkillRegistryProvenance>,
    /// The packages the skill's child run executes, when a turn's load bound
    /// them apart from the run's own.
    pub(crate) admitted: Option<crate::skill_package::AdmittedPackages>,
}

/// A registry child's provenance: what its verified signature says it is,
/// and the digests of the files it signs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LoadedStepSkillRegistryProvenance {
    pub(crate) reference: String,
    pub(crate) signed: crate::registry::SignedPackage,
    /// The manual's digest.
    pub(crate) digest: String,
    /// The digest of the package files beside the manual and the profile,
    /// when it has any.
    pub(crate) bundle_digest: Option<String>,
}

struct ResolvedStepSkillDirectory {
    directory: PathBuf,
    /// The registry ref the step names, when the package is the one a
    /// registry added to the project for it.
    registry_ref: Option<String>,
}

pub(crate) struct LoadedStepSkillPackage {
    pub(crate) package: Arc<crate::LoadedSkillPackage>,
    pub(crate) registry: Option<LoadedStepSkillRegistryProvenance>,
}

#[derive(Default)]
pub(crate) struct StepSkillCache {
    loaded: BTreeMap<String, LoadedStepSkill>,
}

impl StepSkillCache {
    pub(crate) fn load(
        &mut self,
        workflow_dir: &Path,
        step: &WorkflowStep,
        options: StepSkillLoadOptions<'_>,
    ) -> Result<LoadedStepSkill, RuntimeError> {
        if let Some(skill) = self.loaded.get(&step.id) {
            return Ok(skill.clone());
        }
        let skill = load_step_skill(workflow_dir, step, options)?;
        self.loaded.insert(step.id.clone(), skill.clone());
        Ok(skill)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct StepSkillLoadOptions<'a> {
    pub(crate) env: &'a BTreeMap<String, String>,
    /// The packages the run admitted, which a child skill step runs; none
    /// while admission walks the packages from disk.
    pub(crate) packages: Option<&'a crate::skill_package::AdmittedPackages>,
}

impl<'a> StepSkillLoadOptions<'a> {
    /// How a run loads its child skills: from the packages it admitted.
    pub(crate) fn of_run(options: &'a crate::execution::runner::RuntimeOptions) -> Self {
        Self {
            env: &options.env,
            packages: options.packages.as_ref(),
        }
    }

    /// How admission loads child skills: from disk, as they are.
    pub(crate) fn from_disk(env: &'a BTreeMap<String, String>) -> Self {
        Self {
            env,
            packages: None,
        }
    }
}

/// The parent step's provenance, carried to the nested graph's steps that
/// read the same values from the run's input, under the name each binds it
/// to. Values a nested step reads from its own graph's steps keep their own
/// provenance.
pub(crate) fn inherited_step_provenance(
    graph: &Workflow,
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
                    (Some(method_input), Some(key)) => {
                        inherit_provenance(parent, method_input, key, &mut entries);
                    }
                    (Some(method_input), None) => {
                        inherit_provenance(parent, method_input, method_input, &mut entries);
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
    method_input: &str,
    input: &str,
    entries: &mut Vec<runx_contracts::ProvenanceEntry>,
) {
    entries.extend(
        parent
            .iter()
            .filter(|entry| entry.input.as_ref() == method_input)
            .map(|entry| runx_contracts::ProvenanceEntry {
                input: input.into(),
                ..entry.clone()
            }),
    );
}

/// Bind only caller-supplied input facts for inspection. Run identity, acts
/// and preceding step outputs are deliberately unknown before execution.
pub(crate) fn bind_known_inputs(
    mut graph: Workflow,
    inputs: &JsonObject,
) -> Result<Workflow, RuntimeError> {
    let input = JsonValue::Object(inputs.clone());
    for step in &mut graph.steps {
        step.bindings = bind_invoked(
            &step.id,
            &mut step.inputs,
            std::mem::take(&mut step.bindings),
            &|source| match source {
                Source::Input { path } => value_at(&input, path).cloned().map(Some),
                _ => None,
            },
        )?;
        // Raw graph references remain strings until execution binds them.
        // They are unknown facts here, never literal provider arguments.
        for binding in &step.bindings {
            if binding.at.is_empty() {
                step.inputs.clear();
            } else {
                place(&step.id, &mut step.inputs, binding, Some(JsonValue::Null))?;
            }
        }
    }
    Ok(graph)
}

/// Bind what the run was invoked as into every step: each reference to the
/// run's input, each `$run.act` inside an `act:`, and each `$run.id` in an
/// operation's key takes its value, and an absent one leaves itself out of
/// the object or list that holds it; an act that is wholly absent leaves the
/// step without one. References to earlier steps stay for the step to bind
/// when it runs.
pub(crate) fn bind_run_inputs(
    mut graph: Workflow,
    workflow_inputs: &JsonObject,
    act: Option<&CallAct>,
    run_id: &str,
) -> Result<Workflow, RuntimeError> {
    let run_input = JsonValue::Object(workflow_inputs.clone());
    let run_act = act
        .map(JsonValue::from_serialize)
        .transpose()
        .map_err(|error| RuntimeError::EngineInvariant {
            context: "binding the run's act",
            message: error.to_string(),
        })?;
    let invoked = |source: &Source| match source {
        Source::Input { path } => Some(value_at(&run_input, path).cloned()),
        Source::Act { path } => Some(
            run_act
                .as_ref()
                .and_then(|act| value_at(act, path))
                .cloned(),
        ),
        Source::RunId => Some(Some(JsonValue::String(run_id.to_owned()))),
        Source::Step { .. } => None,
    };
    for step in &mut graph.steps {
        step.bindings = bind_invoked(
            &step.id,
            &mut step.inputs,
            step.bindings.split_off(0),
            &invoked,
        )?;
        if let Some(act) = &mut step.act {
            let whole_absent = act.bindings.iter().any(|binding| {
                binding.at.is_empty()
                    && invoked(&binding.source).is_some_and(|value| value.is_none())
            });
            if whole_absent {
                step.act = None;
            } else {
                act.bindings = bind_invoked(
                    &step.id,
                    &mut act.values,
                    act.bindings.split_off(0),
                    &invoked,
                )?;
            }
        }
        if let Some(when) = &mut step.when {
            bind_operand(&mut when.value, &run_input);
        }
        for condition in &mut step.guard {
            bind_operand(&mut condition.value, &run_input);
        }
    }
    Ok(graph)
}

/// Place every binding `invoked` resolves, last first so a list item left out
/// never moves one still to place, and return the rest for the step to bind
/// when it runs, each moved up past any item left out before it.
fn bind_invoked(
    step_id: &str,
    values: &mut JsonObject,
    bindings: Vec<Binding>,
    invoked: &dyn Fn(&Source) -> Option<Option<JsonValue>>,
) -> Result<Vec<Binding>, RuntimeError> {
    let mut later: Vec<Binding> = Vec::new();
    for binding in bindings.into_iter().rev() {
        match invoked(&binding.source) {
            Some(value) => {
                let left_out = value
                    .is_none()
                    .then(|| list_item(values, &binding.at))
                    .flatten();
                place(step_id, values, &binding, value)?;
                if let Some((list, index)) = left_out {
                    for waiting in &mut later {
                        move_up_past(&mut waiting.at, list, index);
                    }
                }
            }
            None => later.push(binding),
        }
    }
    later.reverse();
    Ok(later)
}

/// The list and index `at` names, when it names an item of a list.
fn list_item<'a>(values: &JsonObject, at: &'a [String]) -> Option<(&'a [String], usize)> {
    let (last, list) = at.split_last()?;
    let index = last.parse::<usize>().ok()?;
    let (first, rest) = list.split_first()?;
    matches!(
        value_at(values.get(first)?, rest),
        Some(JsonValue::Array(_))
    )
    .then_some((list, index))
}

/// Move a path inside `list` up one place when the item at `removed`, before
/// it, was left out.
fn move_up_past(at: &mut [String], list: &[String], removed: usize) {
    if at.len() > list.len()
        && at.starts_with(list)
        && let Some(index) = at[list.len()]
            .parse::<usize>()
            .ok()
            .filter(|index| *index > removed)
    {
        at[list.len()] = (index - 1).to_string();
    }
}

/// Bind the input field a condition or a guard reads. An absent field is not
/// `null`: it stays unbound, resolves to nothing, and decides nothing.
fn bind_operand(operand: &mut Operand, run_input: &JsonValue) {
    if let Operand::Reference(Source::Input { path }) = operand
        && let Some(value) = value_at(run_input, path)
    {
        *operand = Operand::Literal(value.clone());
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
    let refused = |reason: String| RuntimeError::StepFailed {
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
    workflow_dir: &Path,
    step: &WorkflowStep,
    options: StepSkillLoadOptions<'_>,
) -> Result<LoadedStepSkill, RuntimeError> {
    let resolved = load_step_skill_package(workflow_dir, step, options)?;
    step_skill(step, resolved, options.env)
}

/// The skill `step` runs, from the package it resolved to.
pub(crate) fn step_skill(
    step: &WorkflowStep,
    resolved: LoadedStepSkillPackage,
    env: &BTreeMap<String, String>,
) -> Result<LoadedStepSkill, RuntimeError> {
    let package = resolved.package;
    let manifest = package.manifest().ok_or_else(|| RuntimeError::StepFailed {
        step_id: step.id.clone(),
        reason: format!(
            "sub-skill {} does not declare a run.yaml method",
            package.directory.display()
        ),
    })?;
    let method = select_step_method(
        manifest,
        step.skill().and_then(|target| target.method.as_deref()),
    )?
    .clone();
    let requirements = manifest.execution_requirements(&method);
    let directory = package.directory.clone();
    let manual_path = package.package_root.join("SKILL.md");
    let manual_markdown = package.package.manual_markdown.clone().into();
    let manual_digest = package.package.manual_digest.clone();
    let offer = method.offer.clone();
    let loaded = LoadedStepSkill {
        version: manifest.version.clone(),
        offer,
        method,
        requirements,
        directory,
        description: package.package.skill.description.clone(),
        manual_path: manual_path.clone(),
        manual_markdown,
        manual_digest,
        registry: resolved.registry,
        admitted: None,
    };
    for path in [
        loaded.directory.join(runx_parser::PROFILE_FILE),
        manual_path,
    ] {
        if path.exists() {
            super::prepared_skill::verify_prepared_artifact_at_use(env, &path)?;
        }
    }
    Ok(loaded)
}

/// The graph a purchase runs: one step that calls `method` of `skill`, the
/// registry selector the skill was reached by, with the run's inputs. The
/// step reaches a method the registry sells, so the kernel buys it (S10).
pub(crate) fn purchase_workflow(skill: &str, method: &str, inputs: &JsonObject) -> Workflow {
    Workflow {
        name: format!("purchase {skill}#{method}"),
        result_from: vec![PURCHASE_STEP.to_owned()],
        outputs: None,
        effects: None,
        reach: None,
        steps: vec![WorkflowStep {
            id: PURCHASE_STEP.to_owned(),
            kind: StepKind::Skill(SkillTarget {
                skill: skill.to_owned(),
                method: Some(method.to_owned()),
            }),
            artifacts: None,
            outputs: None,
            inputs: inputs.clone(),
            bindings: Vec::new(),
            act: None,
            context_skills: Vec::new(),
            when: None,
            guard: Vec::new(),
            retry_unknown_outcomes: false,
            effects: None,
            reach: None,
        }],
    }
}

/// The one step of a purchase's graph.
pub(crate) const PURCHASE_STEP: &str = "purchase";

/// The child skills `step` may run, each as the skill step that runs it: a
/// skill step its own target, and an agent turn one step per child skill its
/// allowance names, resolved exactly as the turn's call to it is.
pub(crate) fn child_skill_steps(step: &WorkflowStep) -> Vec<Cow<'_, WorkflowStep>> {
    match &step.kind {
        StepKind::Skill(_) => vec![Cow::Borrowed(step)],
        StepKind::Agent(agent) => agent
            .allowance
            .named_skills()
            .map(|skill| {
                Cow::Owned(WorkflowStep {
                    id: step.id.clone(),
                    kind: StepKind::Skill(turn_skill_target(skill.as_str())),
                    artifacts: None,
                    outputs: None,
                    inputs: JsonObject::new(),
                    bindings: Vec::new(),
                    act: None,
                    context_skills: Vec::new(),
                    when: None,
                    guard: Vec::new(),
                    retry_unknown_outcomes: false,
                    effects: step.effects,
                    reach: step.reach,
                })
            })
            .collect(),
        StepKind::Tool(_) | StepKind::Operation(_) | StepKind::Ask(_) | StepKind::Load(_) => {
            Vec::new()
        }
    }
}

/// What a turn's call to child skill `skill` runs: the method its reference
/// names after `#`, or the package's default.
pub(crate) fn turn_skill_target(skill: &str) -> SkillTarget {
    match skill.split_once('#') {
        Some((skill, method)) => SkillTarget {
            skill: skill.to_owned(),
            method: Some(method.to_owned()),
        },
        None => SkillTarget {
            skill: skill.to_owned(),
            method: None,
        },
    }
}

pub(crate) fn load_step_skill_package(
    workflow_dir: &Path,
    step: &WorkflowStep,
    options: StepSkillLoadOptions<'_>,
) -> Result<LoadedStepSkillPackage, RuntimeError> {
    let resolved = resolve_step_skill_directory(workflow_dir, step, options)?;
    let package = match options.packages {
        Some(admitted) => admitted.get(&resolved.directory)?,
        None => Arc::new(crate::load_validated_skill_package(&resolved.directory)?),
    };
    let registry = match resolved.registry_ref {
        Some(reference) => Some(registry_provenance(reference, &package, options).map_err(
            |reason| RuntimeError::StepFailed {
                step_id: step.id.clone(),
                reason,
            },
        )?),
        None => None,
    };
    Ok(LoadedStepSkillPackage { package, registry })
}

/// A registry child's provenance, from its signature: verified when its run
/// bound it, or now, against its exact bytes and the version its ref pins
/// (D30). An installed copy that does not verify never binds.
fn registry_provenance(
    reference: String,
    package: &crate::LoadedSkillPackage,
    options: StepSkillLoadOptions<'_>,
) -> Result<LoadedStepSkillRegistryProvenance, String> {
    let admitted = options
        .packages
        .and_then(|admitted| admitted.identity(&package.package_root));
    let signed = match admitted {
        Some(crate::identity::PackageIdentity::Signed(signed)) => signed.clone(),
        _ => crate::identity::registry_copy_identity(&reference, package, options.env)?,
    };
    Ok(LoadedStepSkillRegistryProvenance {
        reference,
        signed,
        digest: package.package.manual_digest.clone(),
        bundle_digest: package_files_digest(package),
    })
}

/// The registry's digest of the files a package holds beside its manual and
/// profile, when it holds any.
pub(crate) fn package_files_digest(package: &crate::LoadedSkillPackage) -> Option<String> {
    let files = package
        .package
        .source
        .files
        .iter()
        .filter(|(path, _)| {
            path.as_str() != "SKILL.md" && path.as_str() != runx_parser::PROFILE_FILE
        })
        .map(|(path, content)| crate::registry::RegistryPackageFile {
            path: path.clone(),
            content: String::from_utf8_lossy(content).into_owned(),
        })
        .collect::<Vec<_>>();
    crate::registry::registry_bundle_digest(&files)
}

pub(crate) fn select_step_method<'a>(
    manifest: &'a SkillMethodManifest,
    requested_method: Option<&str>,
) -> Result<&'a SkillMethodDefinition, RuntimeError> {
    if let Some(method) = requested_method {
        return manifest.methods.get(method).ok_or_else(|| {
            RuntimeError::UnsupportedMethodSelection {
                method: method.to_owned(),
            }
        });
    }
    manifest
        .default_method()
        .ok_or_else(|| RuntimeError::UnsupportedMethodSelection {
            method: "default".to_owned(),
        })
}

pub(crate) fn step_definitions(graph: &Workflow) -> Vec<SequentialWorkflowStepDefinition> {
    graph
        .steps
        .iter()
        .map(|step| SequentialWorkflowStepDefinition {
            id: step.id.clone(),
            context_from: context_from(step),
        })
        .collect()
}

fn resolve_step_skill_directory(
    workflow_dir: &Path,
    step: &WorkflowStep,
    options: StepSkillLoadOptions<'_>,
) -> Result<ResolvedStepSkillDirectory, RuntimeError> {
    if let Some(skill) = step.skill().map(|target| target.skill.as_str()) {
        // `runx/<name>` names the skill this runx bundles and nothing else, at
        // every depth: no workspace copy shadows it, and an installed package
        // reaches it too (D30).
        if crate::registry::is_bundled_reference(skill) {
            let paths = crate::RunxPaths::resolve(options.env, workflow_dir);
            let installed = crate::registry::materialize_bundled_skill(skill, &paths, None)
                .map_err(|reason| RuntimeError::StepFailed {
                    step_id: step.id.clone(),
                    reason,
                })?
                .ok_or_else(|| RuntimeError::StepFailed {
                    step_id: step.id.clone(),
                    reason: format!(
                        "{skill} names a skill runx {} bundles, and it bundles none by that name; a published skill is named registry:<publisher>/<name>@<version>",
                        crate::RUNX_VERSION
                    ),
                })?;
            let directory =
                installed
                    .destination
                    .parent()
                    .ok_or_else(|| RuntimeError::StepFailed {
                        step_id: step.id.clone(),
                        reason: "bundled skill has no package directory".to_owned(),
                    })?;
            return Ok(ResolvedStepSkillDirectory {
                directory: directory.to_path_buf(),
                registry_ref: None,
            });
        }
        if runx_parser::registry_ref(skill).is_some() {
            let paths = crate::RunxPaths::resolve(options.env, workflow_dir);
            // The user's own copy takes precedence over what runx added,
            // whatever version the ref names (core-shape §6.5); a bundle's
            // descriptor owns its children.
            let in_bundle = crate::registry::package_bundle::package_bundle_root(workflow_dir)
                .map_err(|reason| RuntimeError::StepFailed {
                    step_id: step.id.clone(),
                    reason,
                })?
                .is_some();
            if !in_bundle && let Some(directory) = own_registry_skill(skill, &paths, options.env)? {
                return Ok(ResolvedStepSkillDirectory {
                    directory,
                    registry_ref: None,
                });
            }
            if let Some(directory) = installed_registry_skill(skill, &paths)? {
                return Ok(ResolvedStepSkillDirectory {
                    directory,
                    registry_ref: Some(skill.to_owned()),
                });
            }
            return Err(RuntimeError::StepFailed {
                step_id: step.id.clone(),
                reason: format!(
                    "nested skill '{skill}' is not installed in this project; `runx skill` fetches it before the run"
                ),
            });
        }
        if let Some(directory) =
            crate::registry::package_bundle::resolve_bundled_skill(workflow_dir, skill).map_err(
                |reason| RuntimeError::StepFailed {
                    step_id: step.id.clone(),
                    reason,
                },
            )?
        {
            return Ok(ResolvedStepSkillDirectory {
                directory,
                registry_ref: None,
            });
        }
        let directory = workflow_dir.join(skill);
        // An installed package's descriptor owns its dependency identities.
        // Only source packages may discover additional local or bundled skills.
        if !directory.exists()
            && crate::registry::package_bundle::package_bundle_root(workflow_dir)
                .map_err(|reason| RuntimeError::StepFailed {
                    step_id: step.id.clone(),
                    reason,
                })?
                .is_none()
        {
            let paths = crate::RunxPaths::resolve(options.env, workflow_dir);
            if let Some(directory) = crate::resolve_local_skill_reference(
                &paths,
                Path::new(skill),
                &paths.skill_roots(options.env),
            )? {
                return Ok(ResolvedStepSkillDirectory {
                    directory,
                    registry_ref: None,
                });
            }
        }
        return Ok(ResolvedStepSkillDirectory {
            directory,
            registry_ref: None,
        });
    }
    Err(RuntimeError::StepMissingSkill {
        step_id: step.id.clone(),
    })
}

/// Whether the project holds the skill a registry ref names, resolved
/// exactly as execution resolves it: the user's own copy, or the one a
/// registry added.
pub(crate) fn registry_holds_step_skill(
    reference: &str,
    env: &BTreeMap<String, String>,
) -> Result<bool, RuntimeError> {
    let paths = crate::paths::RunxPaths::resolve(env, Path::new("."));
    Ok(own_registry_skill(reference, &paths, env)?.is_some()
        || installed_registry_skill(reference, &paths)?.is_some())
}

/// The user's own copy of the skill a registry ref names, at
/// `skills/<publisher>/<name>/` in the skill roots: it takes precedence over what
/// runx added, whatever version the ref names (core-shape §6.5).
fn own_registry_skill(
    reference: &str,
    paths: &crate::RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<Option<PathBuf>, RuntimeError> {
    crate::resolve_local_skill_reference(
        paths,
        Path::new(&crate::registry::parse_registry_ref(reference).skill_id),
        &paths.skill_roots(env),
    )
}

/// The installed package a `registry:<publisher>/<name>[@version]` ref names: the
/// project's added skills hold what `runx skill` fetched for it, once per
/// version, and a ref with no version names the one version installed.
pub(crate) fn installed_registry_skill(
    reference: &str,
    paths: &crate::RunxPaths,
) -> Result<Option<PathBuf>, RuntimeError> {
    let Some(registry_ref) = runx_parser::registry_ref(reference) else {
        return Ok(None);
    };
    let parsed = crate::registry::parse_registry_ref(registry_ref);
    if split_skill_id(&parsed.skill_id).is_err() {
        return Ok(None);
    }
    // The directory the install wrote, named as it names it: each part
    // reduced to a safe path segment, so no ref reaches outside the skills
    // directory and every version spelling finds what was installed.
    let parts = crate::registry::safe_skill_package_parts(registry_ref, &parsed.skill_id, None);
    let root = paths.added_skills();
    let directory = parts
        .iter()
        .fold(root.clone(), |path, part| path.join(part));
    if parsed.version.is_some() {
        return Ok(directory.join("SKILL.md").is_file().then_some(directory));
    }
    crate::resolve_local_skill_reference(paths, &directory, &[root])
}

/// A step's values when it runs: each reference to an earlier step takes
/// that step's result. An operation step's act is one of its call's fields.
pub(crate) fn materialize_step_invocation_inputs_with_index(
    step: &WorkflowStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<JsonObject, RuntimeError> {
    let mut inputs = bind_earlier_steps(step, &step.inputs, &step.bindings, "", prior_run_index)?;
    if step.operation().is_some()
        && let Some(act) = materialize_step_act_with_index(step, prior_run_index)?
    {
        inputs.insert("act".to_owned(), JsonValue::Object(act));
    }
    Ok(inputs)
}

/// A step's act when it runs: an operation step's call's, or the `$run.act` a
/// skill step's child run takes.
pub(crate) fn materialize_step_act_with_index(
    step: &WorkflowStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<Option<JsonObject>, RuntimeError> {
    step.act
        .as_ref()
        .map(|act| bind_earlier_steps(step, &act.values, &act.bindings, "act", prior_run_index))
        .transpose()
}

fn bind_earlier_steps(
    step: &WorkflowStep,
    values: &JsonObject,
    bindings: &[Binding],
    root: &str,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<JsonObject, RuntimeError> {
    let mut values = values.clone();
    for binding in bindings {
        let Source::Step { step: from, path } = &binding.source else {
            continue;
        };
        let label = match (root, binding_label(binding)) {
            ("", label) => label,
            (root, _) if binding.at.is_empty() => root.to_owned(),
            (root, label) => format!("{root}.{label}"),
        };
        let value = prior_run_index.output(&step.id, &label, from, path)?;
        place(&step.id, &mut values, binding, Some(value))?;
    }
    Ok(values)
}

pub(crate) fn materialize_step_invocation_provenance_with_index(
    step: &WorkflowStep,
    prior_run_index: &PriorRunIndex<'_>,
) -> Result<Vec<runx_contracts::ProvenanceEntry>, RuntimeError> {
    step.bindings
        .iter()
        .filter_map(|binding| match &binding.source {
            Source::Step { step: from, path } => {
                Some(prior_run_index.provenance(&step.id, &binding_label(binding), from, path))
            }
            Source::Input { .. } | Source::Act { .. } | Source::RunId => None,
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

fn context_from(step: &WorkflowStep) -> Option<Vec<String>> {
    let reads = step.reads();
    (!reads.is_empty()).then(|| reads.into_iter().map(str::to_owned).collect())
}

/// A test's workflow, written the one way a workflow is written: as a
/// method's steps in a `run.yaml`.
#[cfg(test)]
pub(crate) mod testing {
    use runx_parser::{Workflow, parse_method_manifest_yaml, validate_method_manifest};

    /// The workflow of the one method `run_yaml` declares, read as the
    /// `run.yaml` of the package `skill`.
    pub(crate) fn method_workflow(skill: &str, run_yaml: &str) -> Result<Workflow, String> {
        let raw = parse_method_manifest_yaml(run_yaml).map_err(|error| error.to_string())?;
        let manifest = validate_method_manifest(skill, raw).map_err(|error| error.to_string())?;
        manifest
            .default_method()
            .map(|method| method.graph.clone())
            .ok_or_else(|| format!("{skill}'s run.yaml declares one method"))
    }
}

#[cfg(test)]
mod tests {
    use runx_contracts::{JsonObject, JsonValue};
    use runx_parser::{Operand, Workflow};

    use super::bind_run_inputs;

    fn graph(source: &str) -> Result<Workflow, String> {
        super::testing::method_workflow("bindings", source)
    }

    fn object(json: serde_json::Value) -> JsonObject {
        serde_json::from_value(json).unwrap_or_default()
    }

    /// The user's own copy of a registry skill, `skills/<publisher>/<name>/`
    /// beside the source, answers its ref whatever version the ref names
    /// (core-shape §6.5).
    #[test]
    fn the_users_own_copy_answers_a_registry_ref() -> Result<(), String> {
        let text = |error: std::io::Error| error.to_string();
        let workspace = tempfile::tempdir().map_err(text)?;
        let env = std::collections::BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().display().to_string(),
        )]);
        let paths = crate::RunxPaths::resolve(&env, workspace.path());
        let own = paths.skills().join("acme/echo");
        std::fs::create_dir_all(&own).map_err(text)?;
        std::fs::write(own.join("SKILL.md"), "---\nname: echo\n---\n").map_err(text)?;
        let added = paths.added_skills().join("acme/echo/1.0.0");
        std::fs::create_dir_all(&added).map_err(text)?;
        std::fs::write(added.join("SKILL.md"), "---\nname: echo\n---\n").map_err(text)?;
        let own_copy = |reference: &str| {
            super::own_registry_skill(reference, &paths, &env).map_err(|error| error.to_string())
        };
        for reference in ["registry:acme/echo@1.0.0", "registry:acme/echo"] {
            assert!(
                own_copy(reference)?
                    .is_some_and(|directory| directory.ends_with("skills/acme/echo")),
                "{reference}"
            );
        }
        assert_eq!(own_copy("registry:acme/other@1.0.0")?, None);
        Ok(())
    }

    /// A nested ref resolves only to what the install wrote among the
    /// project's added skills, named the way the install names it.
    #[test]
    fn an_installed_registry_skill_is_found_only_where_the_install_wrote_it() -> Result<(), String>
    {
        let text = |error: std::io::Error| error.to_string();
        let workspace = tempfile::tempdir().map_err(text)?;
        let outside = tempfile::tempdir().map_err(text)?;
        std::fs::write(outside.path().join("SKILL.md"), "---\nname: outside\n---\n")
            .map_err(text)?;
        let env = std::collections::BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().display().to_string(),
        )]);
        let paths = crate::RunxPaths::resolve(&env, workspace.path());
        let installed = paths.added_skills().join("acme/echo/1.0.0-meta");
        std::fs::create_dir_all(&installed).map_err(text)?;
        std::fs::write(installed.join("SKILL.md"), "---\nname: echo\n---\n").map_err(text)?;
        let find = |reference: &str| {
            super::installed_registry_skill(reference, &paths).map_err(|error| error.to_string())
        };
        assert_eq!(
            find("registry:acme/echo@1.0.0+meta")?,
            Some(installed.clone())
        );
        assert_eq!(find("registry:acme/echo")?, Some(installed));
        assert_eq!(
            find(&format!("registry:acme/echo@{}", outside.path().display()))?,
            None
        );
        assert_eq!(find("registry:acme/echo@../../..")?, None);
        Ok(())
    }

    const BINDINGS: &str = r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: produce
        tool: {module: produce.mjs}
        outputs: {value: {type: string}}
      - id: consume
        tool: {module: consume.mjs}
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
            None,
            "run",
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
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: take\n        tool: {module: take.mjs}\n        inputs: $input.payload\n",
        )?;
        let error = bind_run_inputs(
            whole,
            &object(serde_json::json!({"payload": "text"})),
            None,
            "run",
        )
        .err()
        .ok_or("a whole input that is not an object binds nothing")?;
        assert!(error.to_string().contains("must be an object"), "{error}");

        let list = graph(
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: take\n        tool: {module: take.mjs}\n        inputs: {ids: [$input.first, $input.second, $input.third]}\n",
        )?;
        let bound = bind_run_inputs(
            list,
            &object(serde_json::json!({"first": 1, "third": 3})),
            None,
            "run",
        )
        .map_err(|error| error.to_string())?;
        assert_eq!(
            bound.steps[0].inputs.get("ids"),
            serde_json::from_value::<JsonValue>(serde_json::json!([1, 3]))
                .ok()
                .as_ref(),
            "an absent item leaves itself out and the rest keep their order"
        );

        // A step reference after an absent item still names its own item.
        let mixed = graph(
            "minimum_runx: 0.12.0\nmethods:\n  run:\n    steps:\n      - id: discover\n        tool: {module: discover.mjs}\n        outputs: {id: {type: string}}\n      - id: take\n        tool: {module: take.mjs}\n        inputs: {ids: [$input.optional, $steps.discover.id, {of: $steps.discover.id}]}\n",
        )?;
        let bound = bind_run_inputs(mixed, &JsonObject::new(), None, "run")
            .map_err(|error| error.to_string())?;
        let take = &bound.steps[1];
        assert_eq!(
            take.inputs.get("ids"),
            serde_json::from_value::<JsonValue>(serde_json::json!([
                "$steps.discover.id",
                {"of": "$steps.discover.id"}
            ]))
            .ok()
            .as_ref()
        );
        assert_eq!(
            take.bindings
                .iter()
                .map(|binding| binding.at.join("."))
                .collect::<Vec<_>>(),
            ["ids.0", "ids.1.of"]
        );
        Ok(())
    }

    #[test]
    fn an_operation_key_on_the_run_id_is_the_run_that_binds_it() -> Result<(), String> {
        let source = r#"
minimum_runx: 0.12.0
methods:
  run:
    steps:
      - id: provision
        operation: acme/inbox.provision
        inputs: {seconds: $input.seconds}
        idempotency_key: $run.id
"#;
        let inputs = object(serde_json::json!({"seconds": 3600}));
        let key = |run_id: &str| -> Result<Option<JsonValue>, String> {
            let bound = bind_run_inputs(graph(source)?, &inputs, None, run_id)
                .map_err(|error| error.to_string())?;
            assert!(
                bound.steps[0].bindings.is_empty(),
                "the run's id binds before the step runs"
            );
            Ok(bound.steps[0].inputs.get("idempotency_key").cloned())
        };
        assert_eq!(
            key("hosted_purchase-1")?,
            Some(JsonValue::String("hosted_purchase-1".to_owned()))
        );
        assert_eq!(
            key("hosted_purchase-2")?,
            Some(JsonValue::String("hosted_purchase-2".to_owned())),
            "another run keys another write"
        );
        Ok(())
    }

    #[test]
    fn a_bound_value_is_never_read_again_as_a_reference() -> Result<(), String> {
        let bound = bind_run_inputs(
            graph(BINDINGS)?,
            &object(serde_json::json!({"mode": "apply", "label": "$steps.produce.value"})),
            None,
            "run",
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
