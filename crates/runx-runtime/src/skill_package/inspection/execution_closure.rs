// Module rationale: execution-closure inspection keeps package traversal,
// registry-edge classification, cycle detection, and summary projection in one
// canonical walk.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use runx_contracts::connector::{ConnectorRef, Digest};
use runx_contracts::{EnvironmentRequirements, JsonValue, sha256_prefixed};
use runx_parser::{GraphStep, StepKind, ToolTarget};
use serde::Serialize;

use super::super::LoadedSkillPackage;
use super::SkillInspectionError;

#[derive(Default)]
struct ClosureAccumulator {
    components: BTreeSet<String>,
    skill_edges: BTreeSet<String>,
    direct_external_skill_edges: BTreeSet<DirectExternalSkillEdge>,
    unresolved_skill_edges: BTreeSet<String>,
    package_bindings: BTreeSet<ExecutionPackageBinding>,
    connector_bindings: BTreeSet<ExecutionConnectorBinding>,
    local_packages: BTreeMap<PathBuf, BTreeSet<String>>,
    local_skill_edges: BTreeSet<LocalSkillEdge>,
    profiles: BTreeSet<String>,
    environment_required: BTreeSet<String>,
    environment_optional: BTreeSet<String>,
    agent_acts: usize,
    declared_artifact: bool,
}

impl ClosureAccumulator {
    fn extend_environment(&mut self, environment: &runx_contracts::EnvironmentRequirements) {
        self.environment_required
            .extend(environment.required.iter().cloned());
        self.environment_optional
            .extend(environment.optional.iter().cloned());
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct LocalSkillEdge {
    pub source_package_root: PathBuf,
    pub graph_directory: PathBuf,
    pub reference: String,
    pub target_package_root: PathBuf,
}

#[derive(Default)]
pub(crate) struct LocalExecutionClosure {
    pub packages: BTreeMap<PathBuf, BTreeSet<String>>,
    pub skill_edges: BTreeSet<LocalSkillEdge>,
}

#[derive(Serialize)]
struct ExecutionClosure {
    closure_digest: String,
    runtime_release: String,
    fully_bound: bool,
    summary: String,
    components: Vec<String>,
    skill_edges: Vec<String>,
    direct_external_skill_edges: Vec<DirectExternalSkillEdge>,
    unresolved_skill_edges: Vec<String>,
    package_bindings: Vec<ExecutionPackageBinding>,
    connector_bindings: Vec<ExecutionConnectorBinding>,
    agent_acts: u64,
    declared_artifact: bool,
    profiles: Vec<String>,
    #[serde(skip_serializing_if = "EnvironmentRequirements::is_empty")]
    environment: EnvironmentRequirements,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct ExecutionPackageBinding {
    skill: String,
    runner: String,
    package_digest: String,
    source_kind: ExecutionPackageSourceKind,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    input_packet_schemas: Vec<InputPacketSchemaBinding>,
    source_path: String,
    source_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct ExecutionConnectorBinding {
    connector: ConnectorRef,
    manifest_digest: Digest,
    connector_digest: Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
enum ExecutionPackageSourceKind {
    SourceRoot,
    Registry,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct InputPacketSchemaBinding {
    packet: String,
    schema_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct DirectExternalSkillEdge {
    skill: String,
    runner: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EdgeDepth {
    Direct,
    Nested,
}

impl EdgeDepth {
    const fn records_direct_edges(self) -> bool {
        matches!(self, Self::Direct)
    }
}

/// What inspection does with a registry ref its `RUNX_REGISTRY_DIR` does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnheldRegistryRefs {
    /// Record it as an unresolved edge, so a host can supply it and inspect again.
    Report,
    /// Fail: binding and packaging need every reference present.
    Reject,
}

pub(super) fn inspect_execution_closures(
    loaded: Arc<LoadedSkillPackage>,
    env: Option<&BTreeMap<String, String>>,
    unheld_registry_refs: UnheldRegistryRefs,
) -> Result<BTreeMap<String, JsonValue>, SkillInspectionError> {
    let runner_names = loaded
        .manifest()
        .map(|manifest| manifest.runners.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut inspector = ExecutionClosureInspector::new(loaded, env, unheld_registry_refs)?;
    runner_names
        .into_iter()
        .map(|runner_name| {
            let closure = inspector.inspect_root_runner(&runner_name)?;
            Ok((runner_name, closure))
        })
        .collect()
}

pub(super) fn inspect_local_execution_closure(
    loaded: Arc<LoadedSkillPackage>,
    env: &BTreeMap<String, String>,
) -> Result<LocalExecutionClosure, SkillInspectionError> {
    let runner_names = loaded
        .manifest()
        .map(|manifest| manifest.runners.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut inspector =
        ExecutionClosureInspector::new(loaded, Some(env), UnheldRegistryRefs::Reject)?;
    let mut closure = LocalExecutionClosure::default();
    for runner_name in runner_names {
        merge_local_execution_closure(
            &mut closure,
            inspector
                .inspect_root_runner_accumulator(&runner_name)?
                .into_local_execution_closure(),
        );
    }
    let harness_graphs = inspector
        .root
        .package
        .harness_fixtures
        .iter()
        .filter(|(_, fixture)| {
            fixture.kind == runx_parser::harness_fixture::HarnessFixtureKind::Graph
        })
        .map(|(relative, fixture)| (relative.clone(), fixture.target.clone()))
        .collect::<Vec<_>>();
    for (fixture_path, target) in harness_graphs {
        merge_local_execution_closure(
            &mut closure,
            inspector
                .inspect_harness_graph_accumulator(&fixture_path, &target)?
                .into_local_execution_closure(),
        );
    }
    Ok(closure)
}

impl ClosureAccumulator {
    fn into_local_execution_closure(self) -> LocalExecutionClosure {
        LocalExecutionClosure {
            packages: self.local_packages,
            skill_edges: self.local_skill_edges,
        }
    }
}

fn merge_local_execution_closure(
    target: &mut LocalExecutionClosure,
    source: LocalExecutionClosure,
) {
    for (package_root, packet_ids) in source.packages {
        target
            .packages
            .entry(package_root)
            .or_default()
            .extend(packet_ids);
    }
    target.skill_edges.extend(source.skill_edges);
}

struct ExecutionClosureInspector<'a> {
    root: Arc<LoadedSkillPackage>,
    root_directory: PathBuf,
    package_root: PathBuf,
    env: Option<&'a BTreeMap<String, String>>,
    unheld_registry_refs: UnheldRegistryRefs,
}

impl<'a> ExecutionClosureInspector<'a> {
    fn new(
        root: Arc<LoadedSkillPackage>,
        env: Option<&'a BTreeMap<String, String>>,
        unheld_registry_refs: UnheldRegistryRefs,
    ) -> Result<Self, SkillInspectionError> {
        let root_directory = canonical_directory(&root.directory, "inspected skill")?;
        let package_root = canonical_directory(&root.package_root, "inspected package")?;
        Ok(Self {
            root,
            root_directory,
            package_root,
            env,
            unheld_registry_refs,
        })
    }

    fn inspect_root_runner(
        &mut self,
        runner_name: &str,
    ) -> Result<JsonValue, SkillInspectionError> {
        serialize_closure(self.inspect_root_runner_accumulator(runner_name)?)
    }

    fn inspect_root_runner_accumulator(
        &mut self,
        runner_name: &str,
    ) -> Result<ClosureAccumulator, SkillInspectionError> {
        let mut closure = ClosureAccumulator::default();
        let mut visited = BTreeSet::new();
        let mut walk = ExecutionWalkState {
            closure: &mut closure,
            visited: &mut visited,
        };
        let profile_path = self
            .root
            .profile_path
            .as_deref()
            .unwrap_or(runx_parser::PROFILE_FILE)
            .to_owned();
        self.walk_runner(
            RunnerWalkTarget {
                loaded: self.root.clone(),
                skill_directory: self.root_directory.clone(),
                profile_path,
                runner_name: runner_name.to_owned(),
                edge_depth: EdgeDepth::Direct,
                materialize_local: true,
            },
            &mut walk,
        )?;
        Ok(closure)
    }

    fn inspect_harness_graph_accumulator(
        &mut self,
        fixture_path: &str,
        target: &str,
    ) -> Result<ClosureAccumulator, SkillInspectionError> {
        let fixture_path = self.package_root.join(fixture_path);
        let fixture_directory =
            fixture_path
                .parent()
                .ok_or_else(|| SkillInspectionError::ProfileEscape {
                    reference: fixture_path.to_string_lossy().into_owned(),
                })?;
        let target_path =
            canonical_directory(&fixture_directory.join(target), "harness graph target")?;
        let graph_directory =
            target_path
                .parent()
                .ok_or_else(|| SkillInspectionError::ProfileEscape {
                    reference: target.to_owned(),
                })?;
        let profile_path = target_path
            .strip_prefix(&self.package_root)
            .map_err(|_| SkillInspectionError::ProfileEscape {
                reference: target.to_owned(),
            })?
            .to_string_lossy()
            .into_owned();
        let graph = crate::execution::graph::load_graph(&target_path)?;
        let mut closure = ClosureAccumulator::default();
        let mut visited = BTreeSet::new();
        let mut walk = ExecutionWalkState {
            closure: &mut closure,
            visited: &mut visited,
        };
        self.walk_graph(
            self.root.clone(),
            GraphWalkContext {
                directory: graph_directory,
                profile_path: &profile_path,
                edge_depth: EdgeDepth::Direct,
                materialize_local: true,
            },
            &graph,
            &mut walk,
        )?;
        Ok(closure)
    }

    fn walk_runner(
        &mut self,
        target: RunnerWalkTarget,
        walk: &mut ExecutionWalkState<'_>,
    ) -> Result<(), SkillInspectionError> {
        let RunnerWalkTarget {
            loaded,
            skill_directory,
            profile_path,
            runner_name,
            edge_depth,
            materialize_local,
        } = target;
        if !walk.visited.insert((
            skill_directory.clone(),
            runner_name.clone(),
            materialize_local,
        )) {
            return Ok(());
        }
        let package_root = canonical_directory(&loaded.package_root, "bound skill package")?;
        let mut scope = crate::kernel::connectors::ConnectorScope::at(&package_root)
            .map_err(|error| SkillInspectionError::ConnectorBinding(error.to_string()))?;
        if let Some(env) = self.env {
            scope = scope.with_installed(
                crate::paths::RunxPaths::resolve(env, &package_root).installed_connectors(),
            );
        }
        for (key, requirement) in loaded
            .manifest()
            .into_iter()
            .flat_map(|manifest| &manifest.connectors)
        {
            let references = if requirement.connector_from.is_some() {
                requirement
                    .allowed_connectors
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            } else {
                vec![key.as_str()]
            };
            for reference in references {
                let connector = scope
                    .requirement(reference, requirement)
                    .map_err(|error| SkillInspectionError::ConnectorBinding(error.to_string()))?;
                walk.closure
                    .connector_bindings
                    .insert(ExecutionConnectorBinding {
                        connector: connector.connector_ref.clone(),
                        manifest_digest: connector.manifest_digest.clone(),
                        connector_digest: connector.connector_digest.clone(),
                    });
            }
        }
        let runner = loaded
            .manifest()
            .and_then(|manifest| manifest.runners.get(&runner_name))
            .ok_or_else(|| SkillInspectionError::SubSkillNamedRunnerMissing {
                path: loaded.directory.clone(),
                runner: runner_name.clone(),
            })?;
        let mut input_packet_ids = crate::packet_schemas::declared_input_packet_ids(&runner.inputs);
        for package_tool in loaded.package.tools.values() {
            input_packet_ids.extend(crate::packet_schemas::declared_input_packet_ids(
                &package_tool.tool.inputs,
            ));
        }
        let input_packet_schemas = input_packet_ids
            .into_iter()
            .map(|packet| {
                let schema = loaded
                    .resolved_input_packet_schemas
                    .get(&packet)
                    .ok_or_else(|| SkillInspectionError::ClosureInvalid {
                        runner: runner_name.clone(),
                        problem: "omitted an admitted input packet schema",
                    })?;
                Ok(InputPacketSchemaBinding {
                    packet,
                    schema_digest: schema.schema.sha256.clone(),
                })
            })
            .collect::<Result<Vec<_>, SkillInspectionError>>()?;
        walk.closure
            .package_bindings
            .insert(ExecutionPackageBinding {
                skill: loaded.package.skill.name.clone(),
                runner: runner_name.clone(),
                package_digest: loaded.package.package_digest.clone(),
                source_kind: if materialize_local {
                    ExecutionPackageSourceKind::SourceRoot
                } else {
                    ExecutionPackageSourceKind::Registry
                },
                input_packet_schemas,
                source_path: package_root.to_string_lossy().into_owned(),
                source_files: loaded.package.source.files.keys().cloned().collect(),
            });
        if materialize_local {
            let packet_ids = walk.closure.local_packages.entry(package_root).or_default();
            packet_ids.extend(crate::packet_schemas::declared_runner_packet_ids(runner));
            for package_tool in loaded.package.tools.values() {
                packet_ids.extend(crate::packet_schemas::declared_tool_packet_ids(
                    &package_tool.tool,
                ));
            }
        }
        walk.closure
            .profiles
            .insert(format!("{profile_path}#{runner_name}"));
        self.walk_graph(
            loaded.clone(),
            GraphWalkContext {
                directory: &skill_directory,
                profile_path: &profile_path,
                edge_depth,
                materialize_local,
            },
            &runner.graph,
            walk,
        )
    }

    fn walk_graph(
        &mut self,
        loaded: Arc<LoadedSkillPackage>,
        context: GraphWalkContext<'_>,
        graph: &runx_parser::ExecutionGraph,
        walk: &mut ExecutionWalkState<'_>,
    ) -> Result<(), SkillInspectionError> {
        if context.materialize_local {
            let package_root = canonical_directory(&loaded.package_root, "bound graph package")?;
            let packet_ids = walk.closure.local_packages.entry(package_root).or_default();
            for step in &graph.steps {
                packet_ids.extend(crate::packet_schemas::declared_artifact_packet_ids(
                    step.artifacts.as_ref(),
                ));
            }
        }
        for step in &graph.steps {
            match &step.kind {
                StepKind::Tool(ToolTarget::Catalog(tool)) => {
                    walk.closure.components.insert(format!("tool:{tool}"));
                }
                StepKind::Tool(ToolTarget::Worker(worker)) => {
                    walk.closure.components.insert("worker".to_owned());
                    walk.closure.extend_environment(&worker.environment);
                }
                StepKind::Tool(ToolTarget::Process(process)) => {
                    walk.closure
                        .components
                        .insert(format!("process:{}", process.command));
                    walk.closure.extend_environment(&process.environment);
                }
                StepKind::Operation(target) => {
                    walk.closure.components.insert(match &target.operation {
                        Some(operation) => format!("operation:{}/{operation}", target.requirement),
                        None => format!("operation:{}", target.requirement),
                    });
                }
                StepKind::Agent(agent) => {
                    walk.closure.agent_acts = walk.closure.agent_acts.saturating_add(1);
                    walk.closure.declared_artifact |= step.artifacts.is_some();
                    // The turn may call whatever its allowance names, so the
                    // closure holds it; its child skills are walked below.
                    for tool in &agent.allowance.tools {
                        walk.closure.components.insert(format!("tool:{tool}"));
                    }
                    for allowed in &agent.allowance.operations {
                        walk.closure.components.insert(format!(
                            "operation:{}/{}",
                            allowed.requirement, allowed.operation
                        ));
                    }
                }
                StepKind::Ask(_) | StepKind::Skill(_) => {}
            }
            for child in crate::execution::graph::child_skill_steps(step) {
                let step = child.as_ref();
                let Some(resolved) =
                    self.resolve_step_skill(context.directory, context.profile_path, step)?
                else {
                    continue;
                };
                let ResolvedStepSkill {
                    edge,
                    static_external_name,
                    nested,
                } = resolved;
                let reference = step_reference(step);
                if context.materialize_local
                    && !is_registry_step_ref(reference)
                    && let Some(nested) = nested.as_ref()
                {
                    let source_package_root =
                        canonical_directory(&loaded.package_root, "bound graph package")?;
                    let target_package_root =
                        canonical_directory(&nested.loaded.package_root, "referenced sub-skill")?;
                    if source_package_root != target_package_root {
                        walk.closure.local_skill_edges.insert(LocalSkillEdge {
                            source_package_root,
                            graph_directory: canonical_directory(
                                context.directory,
                                "bound graph directory",
                            )?,
                            reference: reference.to_owned(),
                            target_package_root,
                        });
                    }
                }
                walk.closure.skill_edges.insert(edge.clone());
                if nested.is_none() {
                    walk.closure.unresolved_skill_edges.insert(edge);
                }
                if context.edge_depth.records_direct_edges() {
                    record_direct_external_skill_edge(
                        static_external_name,
                        nested.as_ref(),
                        step,
                        &self.package_root,
                        &mut walk.closure.direct_external_skill_edges,
                    );
                }
                if let Some(nested) = nested {
                    let nested_materialize_local =
                        context.materialize_local && !is_registry_step_ref(reference);
                    self.walk_runner(
                        RunnerWalkTarget {
                            loaded: nested.loaded,
                            skill_directory: nested.canonical_directory,
                            profile_path: nested.profile_path,
                            runner_name: nested.runner_name,
                            edge_depth: EdgeDepth::Nested,
                            materialize_local: nested_materialize_local,
                        },
                        walk,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn resolve_step_skill(
        &mut self,
        graph_directory: &Path,
        profile_path: &str,
        step: &GraphStep,
    ) -> Result<Option<ResolvedStepSkill>, SkillInspectionError> {
        let Some(target) = step.skill() else {
            return Ok(None);
        };
        let reference = target.reference.as_str();
        let requested_runner = target.runner.as_deref().unwrap_or("default");
        if reference.starts_with('$') || (is_registry_step_ref(reference) && self.env.is_none()) {
            return Ok(Some(ResolvedStepSkill {
                edge: format!("{reference}#{requested_runner}"),
                static_external_name: registry_skill_name(reference),
                nested: None,
            }));
        }
        let empty_env = BTreeMap::new();
        let env = self.env.unwrap_or(&empty_env);
        if self.unheld_registry_refs == UnheldRegistryRefs::Report
            && is_registry_step_ref(reference)
            && !crate::execution::graph::registry_holds_step_skill(step, reference, env)?
        {
            return Ok(Some(ResolvedStepSkill {
                edge: format!("{reference}#{requested_runner}"),
                static_external_name: registry_skill_name(reference),
                nested: None,
            }));
        }
        let loaded_step = crate::execution::graph::load_step_skill_package(
            graph_directory,
            step,
            crate::execution::graph::StepSkillLoadOptions { env },
        )?;
        let nested = Arc::new(loaded_step.package);
        let canonical_directory = canonical_directory(&nested.directory, "referenced sub-skill")?;
        let manifest =
            nested
                .manifest()
                .ok_or_else(|| SkillInspectionError::SubSkillManifestMissing {
                    path: nested.directory.clone(),
                })?;
        let nested_runner =
            crate::execution::graph::select_step_runner(manifest, target.runner.as_deref())?
                .name
                .clone();
        let nested_profile = if is_registry_step_ref(reference) {
            nested
                .profile_path
                .clone()
                .unwrap_or_else(|| runx_parser::PROFILE_FILE.to_owned())
        } else {
            nested_profile_path(profile_path, reference)?
        };
        Ok(Some(ResolvedStepSkill {
            edge: format!("{}#{nested_runner}", nested.package.skill.name),
            static_external_name: None,
            nested: Some(ResolvedNestedSkill {
                loaded: nested,
                canonical_directory,
                profile_path: nested_profile,
                runner_name: nested_runner,
            }),
        }))
    }
}

struct RunnerWalkTarget {
    loaded: Arc<LoadedSkillPackage>,
    skill_directory: PathBuf,
    profile_path: String,
    runner_name: String,
    edge_depth: EdgeDepth,
    materialize_local: bool,
}

#[derive(Clone, Copy)]
struct GraphWalkContext<'a> {
    directory: &'a Path,
    profile_path: &'a str,
    edge_depth: EdgeDepth,
    materialize_local: bool,
}

struct ExecutionWalkState<'a> {
    closure: &'a mut ClosureAccumulator,
    visited: &'a mut BTreeSet<(PathBuf, String, bool)>,
}

struct ResolvedStepSkill {
    edge: String,
    static_external_name: Option<String>,
    nested: Option<ResolvedNestedSkill>,
}

struct ResolvedNestedSkill {
    loaded: Arc<LoadedSkillPackage>,
    canonical_directory: PathBuf,
    profile_path: String,
    runner_name: String,
}

fn record_direct_external_skill_edge(
    static_external_name: Option<String>,
    nested: Option<&ResolvedNestedSkill>,
    step: &GraphStep,
    package_root: &Path,
    edges: &mut BTreeSet<DirectExternalSkillEdge>,
) {
    if let Some(nested) = nested {
        if !nested.canonical_directory.starts_with(package_root) {
            edges.insert(DirectExternalSkillEdge {
                skill: nested.loaded.package.skill.name.clone(),
                runner: nested.runner_name.clone(),
            });
        }
    } else if let Some(skill) = static_external_name {
        edges.insert(DirectExternalSkillEdge {
            skill,
            runner: step
                .skill()
                .and_then(|target| target.runner.as_deref())
                .unwrap_or("default")
                .to_owned(),
        });
    }
}

/// A skill step's reference; empty for any other step.
fn step_reference(step: &GraphStep) -> &str {
    step.skill().map_or("", |target| target.reference.as_str())
}

fn canonical_directory(path: &Path, label: &'static str) -> Result<PathBuf, SkillInspectionError> {
    path.canonicalize()
        .map_err(|source| SkillInspectionError::Canonicalize {
            label,
            path: path.to_path_buf(),
            source,
        })
}

fn serialize_closure(closure: ClosureAccumulator) -> Result<JsonValue, SkillInspectionError> {
    let environment = EnvironmentRequirements {
        required: closure.environment_required.iter().cloned().collect(),
        optional: closure
            .environment_optional
            .difference(&closure.environment_required)
            .cloned()
            .collect(),
    };
    let components = closure.components.into_iter().collect::<Vec<_>>();
    let package_bindings = closure.package_bindings.into_iter().collect::<Vec<_>>();
    let connector_bindings = closure.connector_bindings.into_iter().collect::<Vec<_>>();
    let unresolved_skill_edges = closure
        .unresolved_skill_edges
        .into_iter()
        .collect::<Vec<_>>();
    let output = ExecutionClosure {
        closure_digest: execution_closure_digest(
            &package_bindings,
            &connector_bindings,
            &unresolved_skill_edges,
        ),
        runtime_release: crate::EXECUTION_RUNTIME_RELEASE.to_owned(),
        fully_bound: unresolved_skill_edges.is_empty(),
        summary: execution_summary(&components, closure.agent_acts, closure.declared_artifact),
        components,
        skill_edges: closure.skill_edges.into_iter().collect(),
        direct_external_skill_edges: closure.direct_external_skill_edges.into_iter().collect(),
        unresolved_skill_edges,
        package_bindings,
        connector_bindings,
        agent_acts: u64::try_from(closure.agent_acts).unwrap_or(u64::MAX),
        declared_artifact: closure.declared_artifact,
        profiles: closure.profiles.into_iter().collect(),
        environment,
    };
    let serialized = serde_json::to_vec(&output).map_err(|source| SkillInspectionError::Json {
        context: "serializing execution closure",
        source,
    })?;
    serde_json::from_slice(&serialized).map_err(|source| SkillInspectionError::Json {
        context: "projecting execution closure",
        source,
    })
}

fn execution_closure_digest(
    package_bindings: &[ExecutionPackageBinding],
    connector_bindings: &[ExecutionConnectorBinding],
    unresolved_skill_edges: &[String],
) -> String {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"runx.execution-closure.v1\0");
    append_digest_field(&mut canonical, crate::EXECUTION_RUNTIME_RELEASE.as_bytes());
    for binding in package_bindings {
        append_digest_field(&mut canonical, binding.skill.as_bytes());
        append_digest_field(&mut canonical, binding.runner.as_bytes());
        append_digest_field(&mut canonical, binding.package_digest.as_bytes());
        for packet in &binding.input_packet_schemas {
            append_digest_field(&mut canonical, packet.packet.as_bytes());
            append_digest_field(&mut canonical, packet.schema_digest.as_bytes());
        }
    }
    for binding in connector_bindings {
        append_digest_field(&mut canonical, b"connector");
        append_digest_field(&mut canonical, binding.connector.as_str().as_bytes());
        append_digest_field(&mut canonical, binding.manifest_digest.as_str().as_bytes());
        append_digest_field(&mut canonical, binding.connector_digest.as_str().as_bytes());
    }
    for edge in unresolved_skill_edges {
        append_digest_field(&mut canonical, b"unresolved");
        append_digest_field(&mut canonical, edge.as_bytes());
    }
    sha256_prefixed(&canonical)
}

fn append_digest_field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn registry_skill_name(reference: &str) -> Option<String> {
    if !is_registry_step_ref(reference) {
        return None;
    }
    crate::registry::parse_registry_ref(reference)
        .skill_id
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn is_registry_step_ref(reference: &str) -> bool {
    reference.starts_with("registry:")
        || reference.starts_with("runx-registry:")
        || reference.starts_with("runx://skill/")
}

fn nested_profile_path(
    current_profile: &str,
    reference: &str,
) -> Result<String, SkillInspectionError> {
    let current_dir = Path::new(current_profile)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    normalize_relative_path(current_dir.join(reference).join(runx_parser::PROFILE_FILE)).ok_or_else(
        || SkillInspectionError::ProfileEscape {
            reference: reference.to_owned(),
        },
    )
}

fn normalize_relative_path(path: PathBuf) -> Option<String> {
    let mut normalized: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => normalized.push(value.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.last().is_some_and(|segment| segment != "..") {
                    normalized.pop();
                } else {
                    normalized.push("..".to_owned());
                }
            }
            Component::Prefix(_) | Component::RootDir => return None,
        }
    }
    Some(normalized.join("/"))
}

fn execution_summary(components: &[String], agent_acts: usize, declared_artifact: bool) -> String {
    let agent_summary = match (agent_acts, declared_artifact) {
        (0, _) => None,
        (1, true) => Some("1 agent act -> declared artifact".to_owned()),
        (count, true) => Some(format!("{count} agent acts -> declared artifact")),
        (1, false) => Some("1 agent act".to_owned()),
        (count, false) => Some(format!("{count} agent acts")),
    };
    match (components.is_empty(), agent_summary) {
        (true, Some(agent)) => agent,
        (false, Some(agent)) => format!("{}; {agent}", components.join(", ")),
        (false, None) => components.join(", "),
        (true, None) => "none".to_owned(),
    }
}
