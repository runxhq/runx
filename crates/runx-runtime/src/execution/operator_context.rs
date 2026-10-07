// Module rationale: transitive preflight discovery keeps traversal,
// admission provenance, limits, and its focused fixtures in one auditable module.
//! Runtime-owned preflight expansion for the complete skill chain shown to an
//! operator before execution. Discovery uses the same validated graph, child
//! skill, registry admission, and context-skill loaders as execution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{
    ContextEntry, ExecutionBoundaryKind, ExecutionBoundaryObservation, JsonObject, JsonValue,
    sha256_prefixed,
};
use runx_parser::{ExecutionGraph, GraphRunTarget, GraphStep, SkillRunnerDefinition, SourceKind};
use serde::{Deserialize, Serialize};

use crate::RuntimeEffectRegistry;
use crate::RuntimeError;
use crate::services::{WorkspaceEnv, merge_inferred_tool_roots};
use crate::tool_catalogs::{ToolCatalogError, ToolInspectOptions, resolve_local_tool};

use super::graph::{
    LoadedStepSkill, LoadedStepSkillRegistryProvenance, StepSkillLoadOptions, load_step_skill,
};
use super::skill_context::load_context_skills;
use super::skill_front::SkillRunError;
use super::skill_front::runner_manifest::selected_runner;

const MAX_CHAIN_DEPTH: usize = 16;
const MAX_CHAIN_NODES: usize = 128;
const MAX_CHAIN_CONTENT_BYTES: usize = 4 * 1024 * 1024;
const OPERATOR_CONTEXT_CREATED_AT: &str = "operator-context-preflight";

#[derive(Clone, Debug)]
pub struct SkillOperatorContextOptions {
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    max_depth: usize,
    max_nodes: usize,
    max_content_bytes: usize,
    effects: RuntimeEffectRegistry,
}

impl SkillOperatorContextOptions {
    #[must_use]
    pub fn new(env: BTreeMap<String, String>, cwd: PathBuf) -> Self {
        Self {
            env,
            cwd,
            max_depth: MAX_CHAIN_DEPTH,
            max_nodes: MAX_CHAIN_NODES,
            max_content_bytes: MAX_CHAIN_CONTENT_BYTES,
            effects: RuntimeEffectRegistry::default(),
        }
    }

    #[must_use]
    pub fn with_effects(mut self, effects: RuntimeEffectRegistry) -> Self {
        self.effects = effects;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextChain {
    pub entry: SkillOperatorContextNode,
    pub node_count: usize,
    pub content_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_content_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextNode {
    pub node_path: String,
    pub package: SkillOperatorContextPackage,
    pub skill_markdown: SkillOperatorContextDocument,
    pub runner: SkillOperatorContextRunner,
    pub steps: Vec<SkillOperatorContextStep>,
    pub tools: Vec<SkillOperatorContextTool>,
    pub terminal: SkillOperatorContextTerminal,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOperatorContextPackage {
    pub directory: PathBuf,
    pub reference: Option<String>,
    pub source: String,
    pub source_label: String,
    pub registry: Option<SkillOperatorContextRegistry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOperatorContextRegistry {
    pub reference: String,
    pub source: String,
    pub source_label: String,
    pub skill_id: String,
    pub version: String,
    pub digest: String,
    pub package_digest: Option<String>,
    pub trust_tier: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOperatorContextDocument {
    pub path: Option<PathBuf>,
    pub source_label: String,
    pub sha256: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextRunner {
    pub name: String,
    pub source_type: String,
    pub selection: String,
    pub requested_name: Option<String>,
    pub scopes: Vec<String>,
    pub execution_boundary: Option<ExecutionBoundaryObservation>,
    pub declared_source_output: bool,
    pub declared_artifact_output: bool,
    pub raw: JsonValue,
    pub allowed_tools: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextStep {
    pub node_path: String,
    pub definition: GraphStep,
    pub context_skills: Vec<SkillOperatorContextContextSkill>,
    pub tool_refs: Vec<String>,
    pub execution_boundary: Option<ExecutionBoundaryObservation>,
    pub child: Option<Box<SkillOperatorContextNode>>,
}

impl SkillOperatorContextStep {
    #[must_use]
    pub fn target_label(&self) -> String {
        let step = &self.definition;
        if let Some(reference) = &step.skill {
            return match &step.runner {
                Some(runner) => format!("skill {reference} runner {runner}"),
                None => format!("skill {reference}"),
            };
        }
        if let Some(name) = &step.tool {
            return format!("tool {name}");
        }
        if let Some(target) = &step.operation {
            return match &target.operation {
                Some(operation) => format!("operation {}/{operation}", target.requirement),
                None => format!("operation {}", target.requirement),
            };
        }
        match &step.run {
            Some(GraphRunTarget::Approval) => "run approval".to_owned(),
            Some(GraphRunTarget::Source(source)) => format!("run {}", source.source_type),
            None => "missing target".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextContextSkill {
    pub reference: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub artifact: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOperatorContextTool {
    pub name: String,
    pub source: String,
    pub path: Option<PathBuf>,
    pub sha256: Option<String>,
    pub content: Option<String>,
    pub declared_artifact_output: bool,
    pub execution_boundary: ExecutionBoundaryObservation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillOperatorContextTerminal {
    ExpandedGraph,
    Runner,
}

pub fn load_skill_operator_context_chain(
    skill_path: &Path,
    selected_runner_name: Option<&str>,
    options: SkillOperatorContextOptions,
) -> Result<SkillOperatorContextChain, SkillRunError> {
    let loaded = crate::load_validated_skill_package(skill_path)?;
    load_skill_operator_context_chain_from_package(&loaded, selected_runner_name, options)
}

pub(crate) fn load_skill_operator_context_chain_from_package(
    loaded: &crate::LoadedSkillPackage,
    selected_runner_name: Option<&str>,
    options: SkillOperatorContextOptions,
) -> Result<SkillOperatorContextChain, SkillRunError> {
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        SkillRunError::Invalid(format!(
            "skill package {} does not declare X.yaml runners",
            loaded.directory.display()
        ))
    })?;
    let skill_dir = loaded.directory.clone();
    let manual_path = loaded.package_root.join("SKILL.md");
    let manual_markdown = loaded.package.manual_markdown.clone();
    let manual_digest = loaded.package.manual_digest.clone();
    let runner = selected_runner(&manifest, selected_runner_name)?.clone();
    let workspace =
        WorkspaceEnv::new(options.env.clone(), options.cwd.clone()).map_err(RuntimeError::from)?;
    let env = workspace.skill_env_for_skill(&skill_dir);
    let mut state = ExpansionState::new(options);
    let entry = state.expand_runner_node(NodeInput {
        node_path: "entry".to_owned(),
        package: local_package(&skill_dir, None),
        skill_dir,
        manual_path,
        manual_markdown,
        manual_digest,
        runner,
        requested_runner: selected_runner_name.map(str::to_owned),
        env,
        depth: 0,
    })?;
    Ok(SkillOperatorContextChain {
        entry,
        node_count: state.node_count,
        content_bytes: state.content_bytes,
        max_depth: state.options.max_depth,
        max_nodes: state.options.max_nodes,
        max_content_bytes: state.options.max_content_bytes,
    })
}

struct ExpansionState {
    options: SkillOperatorContextOptions,
    node_count: usize,
    content_bytes: usize,
    ancestry: BTreeSet<String>,
}

struct NodeInput {
    node_path: String,
    package: SkillOperatorContextPackage,
    skill_dir: PathBuf,
    manual_path: PathBuf,
    manual_markdown: String,
    manual_digest: String,
    runner: SkillRunnerDefinition,
    requested_runner: Option<String>,
    env: BTreeMap<String, String>,
    depth: usize,
}

impl ExpansionState {
    fn new(options: SkillOperatorContextOptions) -> Self {
        Self {
            options,
            node_count: 0,
            content_bytes: 0,
            ancestry: BTreeSet::new(),
        }
    }

    fn expand_runner_node(
        &mut self,
        input: NodeInput,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        self.admit_node(input.depth)?;
        let identity = node_identity(&input.package, &input.skill_dir, &input.runner.name)?;
        if !self.ancestry.insert(identity.clone()) {
            return Err(blocked(format!(
                "operator context chain contains a cycle at {} ({identity})",
                input.node_path
            )));
        }

        let result = self.build_runner_node(&input);
        self.ancestry.remove(&identity);
        result
    }

    fn build_runner_node(
        &mut self,
        input: &NodeInput,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        let skill_markdown = self.manual_document(
            &input.manual_path,
            &input.manual_markdown,
            &input.manual_digest,
        )?;
        let raw = JsonValue::Object(input.runner.raw.clone());
        self.add_bytes(serialized_bytes(&raw)?)?;
        let graph = input.runner.source.graph.as_ref();
        let tool_names = referenced_tools(&input.runner, graph);
        let tools = self.load_tools(&input.skill_dir, &tool_names, &input.env)?;
        let steps = match graph {
            Some(graph) => self.expand_graph_steps(
                &input.node_path,
                &input.skill_dir,
                graph,
                &input.env,
                input.depth,
            )?,
            None => Vec::new(),
        };
        let (declared_source_output, declared_artifact_output) = match graph {
            Some(graph) => {
                validate_graph_result_contracts(&input.node_path, graph, &steps, &tools)?;
                (!graph.result_from.is_empty(), false)
            }
            None => (
                input
                    .runner
                    .source
                    .outputs
                    .as_ref()
                    .is_some_and(|outputs| !outputs.is_empty()),
                crate::output_contract::declares_output_contract(
                    None,
                    input.runner.artifacts.as_ref(),
                ),
            ),
        };
        let terminal = if graph.is_some() {
            SkillOperatorContextTerminal::ExpandedGraph
        } else {
            SkillOperatorContextTerminal::Runner
        };
        Ok(SkillOperatorContextNode {
            node_path: input.node_path.clone(),
            package: input.package.clone(),
            skill_markdown,
            runner: runner_context(
                &input.runner,
                input.requested_runner.as_deref(),
                declared_source_output,
                declared_artifact_output,
                raw,
            ),
            steps,
            tools,
            terminal,
        })
    }

    fn expand_graph_steps(
        &mut self,
        parent_path: &str,
        graph_dir: &Path,
        graph: &ExecutionGraph,
        env: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<Vec<SkillOperatorContextStep>, SkillRunError> {
        graph
            .steps
            .iter()
            .map(|step| self.expand_graph_step(parent_path, graph_dir, step, env, depth))
            .collect()
    }

    // Function rationale: one graph-step expansion must resolve
    // its exclusive target, attached context, child provenance, and recursion together.
    fn expand_graph_step(
        &mut self,
        parent_path: &str,
        graph_dir: &Path,
        step: &GraphStep,
        env: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<SkillOperatorContextStep, SkillRunError> {
        let node_path = format!("{parent_path}.{}", step.id);
        self.add_bytes(serialized_bytes(step)?)?;
        let mut context_skills = Vec::new();
        let mut child = None;
        if let Some(reference) = &step.skill {
            let loaded = load_step_skill(graph_dir, step, StepSkillLoadOptions { env })?;
            context_skills = self.load_step_context(graph_dir, step, env)?;
            if !context_skills.is_empty()
                && !matches!(
                    loaded.runner.source.source_type,
                    SourceKind::Agent | SourceKind::AgentStep
                )
            {
                return Err(RuntimeError::InvalidRunStep {
                    step_id: step.id.clone(),
                    reason: "context_skills is only supported for agent and agent-task steps"
                        .to_owned(),
                }
                .into());
            }
            child = Some(Box::new(self.expand_loaded_child(
                node_path.clone(),
                reference,
                step.runner.as_deref(),
                loaded,
                env,
                depth + 1,
            )?));
        } else if step.tool.is_some() || step.operation.is_some() {
            // Tool admission and manifest resolution are handled for the
            // containing node after every step has been expanded; the kernel
            // admits an operation when it runs.
        } else if step.run.is_some() {
            context_skills = self.load_step_context(graph_dir, step, env)?;
        } else {
            return Err(blocked(format!(
                "operator context graph step '{}' has no target",
                step.id
            )));
        }
        let tool_refs = step_tool_refs(step);
        let execution_boundary = if let Some(tool) = step.tool.as_deref() {
            crate::tool_catalogs::native::execution_boundary(tool, &self.options.effects)
                .map(boundary)
        } else {
            match &step.run {
                Some(GraphRunTarget::Source(source)) => {
                    execution_boundary_for_source(source.source_type)
                }
                Some(GraphRunTarget::Approval) | None => None,
            }
        };
        Ok(SkillOperatorContextStep {
            node_path,
            definition: step.clone(),
            context_skills,
            tool_refs,
            execution_boundary,
            child,
        })
    }

    fn expand_loaded_child(
        &mut self,
        node_path: String,
        reference: &str,
        requested_runner: Option<&str>,
        loaded: LoadedStepSkill,
        env: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        let package = loaded_package(&loaded, reference);
        let mut child_env = env.clone();
        merge_inferred_tool_roots(&mut child_env, &loaded.directory);
        self.expand_runner_node(NodeInput {
            node_path,
            package,
            skill_dir: loaded.directory,
            manual_path: loaded.manual_path,
            manual_markdown: loaded.manual_markdown.as_ref().to_owned(),
            manual_digest: loaded.manual_digest,
            runner: loaded.runner,
            requested_runner: requested_runner.map(str::to_owned),
            env: child_env,
            depth,
        })
    }

    fn load_step_context(
        &mut self,
        graph_dir: &Path,
        step: &GraphStep,
        env: &BTreeMap<String, String>,
    ) -> Result<Vec<SkillOperatorContextContextSkill>, SkillRunError> {
        let entries = load_context_skills(
            &step.id,
            graph_dir,
            &step.context_skills,
            env,
            OPERATOR_CONTEXT_CREATED_AT,
        )?;
        step.context_skills
            .iter()
            .zip(entries)
            .map(|(reference, entry)| self.context_skill(reference, entry))
            .collect()
    }

    fn context_skill(
        &mut self,
        reference: &str,
        entry: ContextEntry,
    ) -> Result<SkillOperatorContextContextSkill, SkillRunError> {
        let resolved_reference = entry
            .data
            .get("ref")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| blocked("resolved context skill is missing string field ref"))?;
        if resolved_reference != reference {
            return Err(blocked(format!(
                "resolved context skill ref '{resolved_reference}' does not match declared ref '{reference}'"
            )));
        }
        let artifact_bytes = usize::try_from(entry.meta.size_bytes)
            .map_err(|_| blocked("resolved context skill artifact size exceeds this platform"))?;
        self.add_bytes(artifact_bytes)?;
        Ok(SkillOperatorContextContextSkill {
            reference: reference.to_owned(),
            artifact_sha256: entry.meta.hash.as_str().to_owned(),
            artifact_bytes: entry.meta.size_bytes,
            artifact: entry.data,
        })
    }

    fn manual_document(
        &mut self,
        path: &Path,
        content: &str,
        sha256: &str,
    ) -> Result<SkillOperatorContextDocument, SkillRunError> {
        self.add_bytes(content.len())?;
        Ok(SkillOperatorContextDocument {
            path: Some(path.to_path_buf()),
            source_label: path.to_string_lossy().into_owned(),
            sha256: sha256.to_owned(),
            content: content.to_owned(),
        })
    }

    fn load_tools(
        &mut self,
        skill_dir: &Path,
        names: &BTreeSet<String>,
        env: &BTreeMap<String, String>,
    ) -> Result<Vec<SkillOperatorContextTool>, SkillRunError> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        names
            .iter()
            .map(|name| {
                let workspace_root = crate::config::resolve_runx_workspace_base(env, skill_dir);
                if let Some(report) = crate::tool_catalogs::native::inspect(
                    name,
                    &workspace_root,
                    &self.options.effects,
                ) {
                    let content = serde_json::to_string(&report.tool).map_err(|source| {
                        RuntimeError::json("serializing native tool operator context", source)
                    })?;
                    self.add_bytes(content.len())?;
                    return Ok(SkillOperatorContextTool {
                        name: name.clone(),
                        source: "native-runtime".to_owned(),
                        path: None,
                        sha256: Some(sha256_prefixed(content.as_bytes())),
                        content: Some(content),
                        declared_artifact_output: crate::output_contract::declares_output_contract(
                            None,
                            crate::tool_catalogs::native::artifacts(name, &self.options.effects)
                                .as_ref(),
                        ),
                        execution_boundary: boundary(
                            crate::tool_catalogs::native::execution_boundary(
                                name,
                                &self.options.effects,
                            )
                            .unwrap_or(ExecutionBoundaryKind::NativeCapability),
                        ),
                    });
                }
                match resolve_referenced_local_tool(skill_dir, name, env)? {
                    Some(tool) => {
                        self.add_bytes(tool.content.len())?;
                        Ok(SkillOperatorContextTool {
                            name: name.clone(),
                            source: "local-manifest".to_owned(),
                            path: Some(tool.path),
                            sha256: Some(sha256_prefixed(tool.content.as_bytes())),
                            content: Some(tool.content),
                            declared_artifact_output:
                                crate::output_contract::declares_output_contract(
                                    None,
                                    tool.artifacts.as_ref(),
                                ),
                            execution_boundary: tool.execution_boundary,
                        })
                    }
                    None => Err(blocked(format!(
                        "operator context could not resolve required local tool '{name}'"
                    ))),
                }
            })
            .collect()
    }

    fn admit_node(&mut self, depth: usize) -> Result<(), SkillRunError> {
        if depth > self.options.max_depth {
            return Err(blocked(format!(
                "operator context chain exceeds maximum depth {}",
                self.options.max_depth
            )));
        }
        self.node_count = self
            .node_count
            .checked_add(1)
            .ok_or_else(|| blocked("operator context node count overflow"))?;
        if self.node_count > self.options.max_nodes {
            return Err(blocked(format!(
                "operator context chain exceeds maximum node count {}",
                self.options.max_nodes
            )));
        }
        Ok(())
    }

    fn add_bytes(&mut self, bytes: usize) -> Result<(), SkillRunError> {
        self.content_bytes = self
            .content_bytes
            .checked_add(bytes)
            .ok_or_else(|| blocked("operator context content byte count overflow"))?;
        if self.content_bytes > self.options.max_content_bytes {
            return Err(blocked(format!(
                "operator context chain exceeds maximum content bytes {}",
                self.options.max_content_bytes
            )));
        }
        Ok(())
    }
}

fn runner_context(
    runner: &SkillRunnerDefinition,
    requested_runner: Option<&str>,
    declared_source_output: bool,
    declared_artifact_output: bool,
    raw: JsonValue,
) -> SkillOperatorContextRunner {
    let selection = if requested_runner.is_some() {
        "requested"
    } else if runner.default {
        "default"
    } else {
        "only"
    };
    SkillOperatorContextRunner {
        name: runner.name.clone(),
        source_type: runner.source.source_type.to_string(),
        selection: selection.to_owned(),
        requested_name: requested_runner.map(str::to_owned),
        scopes: runner.scopes.clone(),
        execution_boundary: execution_boundary_for_source(runner.source.source_type),
        declared_source_output,
        declared_artifact_output,
        raw,
        allowed_tools: runner.allowed_tools.clone().unwrap_or_default(),
    }
}

fn execution_boundary_for_source(source_type: SourceKind) -> Option<ExecutionBoundaryObservation> {
    let kind = match source_type {
        SourceKind::CliTool => ExecutionBoundaryKind::TrustedHostProcess,
        SourceKind::JavaScript => ExecutionBoundaryKind::DeterministicWorker,
        SourceKind::Agent | SourceKind::AgentStep => ExecutionBoundaryKind::RemoteProvider,
        SourceKind::Graph => return None,
    };
    Some(boundary(kind))
}

const fn boundary(kind: ExecutionBoundaryKind) -> ExecutionBoundaryObservation {
    ExecutionBoundaryObservation { kind }
}

fn validate_graph_result_contracts(
    node_path: &str,
    graph: &ExecutionGraph,
    steps: &[SkillOperatorContextStep],
    tools: &[SkillOperatorContextTool],
) -> Result<(), SkillRunError> {
    for result_step_id in &graph.result_from {
        let step = steps
            .iter()
            .find(|step| step.definition.id == *result_step_id)
            .ok_or_else(|| {
                blocked(format!(
                    "operator context graph result producer '{node_path}.{result_step_id}' was not expanded"
                ))
            })?;
        if !step_declares_result(step, tools) {
            return Err(blocked(format!(
                "graph result producer '{}' declares no semantic output contract; add run.outputs or artifacts.wrap_as/named_emits before execution",
                step.node_path
            )));
        }
    }
    Ok(())
}

fn step_declares_result(
    step: &SkillOperatorContextStep,
    tools: &[SkillOperatorContextTool],
) -> bool {
    // An approval resolves to its decision; an operation always emits its
    // `runx.provider.operation.v1` packet.
    if matches!(step.definition.run, Some(GraphRunTarget::Approval))
        || step.definition.operation.is_some()
    {
        return true;
    }
    let step_artifact_output =
        crate::output_contract::declares_output_contract(None, step.definition.artifacts.as_ref());
    if let Some(source) = step.definition.run.as_ref().and_then(|run| run.source()) {
        return crate::output_contract::declares_output_contract(
            source.outputs.as_ref(),
            step.definition.artifacts.as_ref(),
        );
    }
    if let Some(tool_name) = step.definition.tool.as_deref() {
        return step_artifact_output
            || (step.definition.artifacts.is_none()
                && tools
                    .iter()
                    .find(|tool| tool.name == tool_name)
                    .is_some_and(|tool| tool.declared_artifact_output));
    }
    if let Some(child) = step.child.as_deref() {
        return child.runner.declared_source_output
            || step_artifact_output
            || (step.definition.artifacts.is_none() && child.runner.declared_artifact_output);
    }
    false
}

fn local_package(skill_dir: &Path, reference: Option<String>) -> SkillOperatorContextPackage {
    SkillOperatorContextPackage {
        directory: skill_dir.to_path_buf(),
        reference,
        source: "local-path".to_owned(),
        source_label: skill_dir.to_string_lossy().into_owned(),
        registry: None,
    }
}

fn loaded_package(loaded: &LoadedStepSkill, reference: &str) -> SkillOperatorContextPackage {
    match loaded.registry.as_ref() {
        Some(registry) => SkillOperatorContextPackage {
            directory: loaded.directory.clone(),
            reference: Some(reference.to_owned()),
            source: registry.source.clone(),
            source_label: registry.source_label.clone(),
            registry: Some(registry_context(registry)),
        },
        None => local_package(&loaded.directory, Some(reference.to_owned())),
    }
}

fn registry_context(value: &LoadedStepSkillRegistryProvenance) -> SkillOperatorContextRegistry {
    SkillOperatorContextRegistry {
        reference: value.reference.clone(),
        source: value.source.clone(),
        source_label: value.source_label.clone(),
        skill_id: value.skill_id.clone(),
        version: value.version.clone(),
        digest: value.digest.clone(),
        package_digest: value.package_digest.clone(),
        trust_tier: value.trust_tier.clone(),
    }
}

fn node_identity(
    package: &SkillOperatorContextPackage,
    skill_dir: &Path,
    runner_name: &str,
) -> Result<String, SkillRunError> {
    if let Some(registry) = &package.registry {
        return Ok(format!(
            "registry:{}@{}:{}:{}",
            registry.skill_id,
            registry.version,
            registry
                .package_digest
                .as_deref()
                .unwrap_or(&registry.digest),
            runner_name
        ));
    }
    let canonical = fs::canonicalize(skill_dir).map_err(|source| {
        RuntimeError::io(
            format!("canonicalizing skill directory {}", skill_dir.display()),
            source,
        )
    })?;
    Ok(format!("local:{}:{runner_name}", canonical.display()))
}

fn referenced_tools(
    runner: &SkillRunnerDefinition,
    graph: Option<&ExecutionGraph>,
) -> BTreeSet<String> {
    let mut names = runner
        .allowed_tools
        .iter()
        .flatten()
        .cloned()
        .collect::<BTreeSet<_>>();
    if let Some(graph) = graph {
        for step in &graph.steps {
            names.extend(step_tool_refs(step));
        }
    }
    names
}

fn step_tool_refs(step: &GraphStep) -> Vec<String> {
    let mut names = step
        .allowed_tools
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect::<BTreeSet<_>>();
    if let Some(tool) = &step.tool {
        names.insert(tool.clone());
    }
    names.into_iter().collect()
}

struct ResolvedOperatorContextTool {
    path: PathBuf,
    content: String,
    artifacts: Option<runx_parser::SkillArtifactContract>,
    execution_boundary: ExecutionBoundaryObservation,
}

fn resolve_referenced_local_tool(
    skill_dir: &Path,
    name: &str,
    env: &BTreeMap<String, String>,
) -> Result<Option<ResolvedOperatorContextTool>, SkillRunError> {
    let options = ToolInspectOptions {
        root: crate::config::resolve_runx_workspace_base(env, skill_dir),
        tool_ref: name.to_owned(),
        source: None,
        search_from_directory: skill_dir.to_path_buf(),
        tool_roots: env
            .get("RUNX_TOOL_ROOTS")
            .map(|value| {
                std::env::split_paths(value)
                    .filter(|path| !path.as_os_str().is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        allow_explicit_manifest_path: true,
    };
    match resolve_local_tool(&options) {
        Ok(resolution) => {
            let execution_boundary = execution_boundary_for_source(
                resolution.tool.source.source_type,
            )
            .ok_or_else(|| blocked(format!("local tool '{name}' cannot use a graph source")))?;
            Ok(Some(ResolvedOperatorContextTool {
                path: resolution.manifest_path,
                content: resolution.manifest_source,
                artifacts: resolution.tool.artifacts,
                execution_boundary,
            }))
        }
        Err(ToolCatalogError::NotFound(_)) => Ok(None),
        Err(ToolCatalogError::InvalidRequest(message))
            if message.contains("must include a namespace") =>
        {
            Ok(None)
        }
        Err(error) => Err(blocked(format!(
            "operator context could not resolve tool '{name}': {error}"
        ))),
    }
}

fn serialized_bytes(value: &impl serde::Serialize) -> Result<usize, SkillRunError> {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .map_err(|source| RuntimeError::json("serializing operator context content", source).into())
}

fn blocked(message: impl Into<String>) -> SkillRunError {
    SkillRunError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn operator_context_expands_local_child_agent_runner() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child contract")?;
        write_file(
            &child.join("X.yaml"),
            r#"skill: child
runners:
  review:
    default: true
    type: agent-task
    agent: reviewer
    task: review
"#,
        )?;
        write_file(
            &entry.join("X.yaml"),
            r#"skill: entry
runners:
  main:
    default: true
    type: graph
    graph:
      name: entry
      result_from: [review]
      steps:
        - id: review
          skill: ./child
          artifacts:
            wrap_as: review_result
"#,
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        let child = chain.entry.steps[0].child.as_ref().ok_or("missing child")?;
        assert_eq!(child.node_path, "entry.review");
        assert_eq!(child.runner.name, "review");
        assert!(child.skill_markdown.content.contains("# Child contract"));
        assert_eq!(child.terminal, SkillOperatorContextTerminal::Runner);
        Ok(())
    }

    #[test]
    fn operator_context_uses_child_graph_dir_for_inner_context() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        let rubric = child.join("context/rubric");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child")?;
        write_skill(&rubric, "rubric", "child-local rubric")?;
        write_file(
            &child.join("X.yaml"),
            r#"skill: child
runners:
  graph:
    default: true
    type: graph
    graph:
      name: child
      result_from: [judge]
      steps:
        - id: judge
          run:
            type: agent-task
            agent: reviewer
            task: judge
          context_skills:
            - ./context/rubric
          artifacts:
            wrap_as: judgment
"#,
        )?;
        write_entry_graph(&entry, "./child", "")?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        let child = chain.entry.steps[0].child.as_ref().ok_or("missing child")?;
        let context = &child.steps[0].context_skills[0];
        assert_eq!(context.reference, "./context/rubric");
        assert_eq!(
            context.artifact.get("path").and_then(JsonValue::as_str),
            rubric.join("SKILL.md").canonicalize()?.to_str()
        );
        assert_eq!(
            context.artifact.get("content").and_then(JsonValue::as_str),
            Some("---\nname: rubric\n---\nchild-local rubric\n")
        );
        assert!(context.artifact.contains_key("manual_sha256"));
        Ok(())
    }

    #[test]
    fn operator_context_uses_parent_graph_dir_for_child_agent_context() -> Result<(), Box<dyn Error>>
    {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        let rubric = entry.join("context/rubric");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child")?;
        write_skill(&rubric, "rubric", "parent-local rubric")?;
        write_file(
            &child.join("X.yaml"),
            r#"skill: child
runners:
  agent:
    default: true
    type: agent-task
    agent: reviewer
    task: judge
"#,
        )?;
        write_entry_graph(
            &entry,
            "./child",
            "          context_skills:\n            - ./context/rubric\n",
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        let context = &chain.entry.steps[0].context_skills[0];
        assert_eq!(
            context.artifact.get("path").and_then(JsonValue::as_str),
            rubric.join("SKILL.md").canonicalize()?.to_str()
        );
        assert_eq!(
            context.artifact.get("content").and_then(JsonValue::as_str),
            Some("---\nname: rubric\n---\nparent-local rubric\n")
        );
        Ok(())
    }

    #[test]
    fn operator_context_rejects_context_on_child_graph() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child")?;
        write_skill(&entry.join("context/rubric"), "rubric", "rubric")?;
        write_file(
            &child.join("X.yaml"),
            r#"skill: child
runners:
  graph:
    default: true
    type: graph
    graph:
      name: child
      result_from: [judge]
      steps:
        - id: judge
          run:
            type: agent-task
            agent: reviewer
            task: judge
"#,
        )?;
        write_entry_graph(
            &entry,
            "./child",
            "          context_skills:\n            - ./context/rubric\n",
        )?;

        let error = operator_context_error(
            load_skill_operator_context_chain(
                &entry,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "child graph context must fail",
        )?;
        assert!(
            error
                .to_string()
                .contains("context_skills is only supported for agent and agent-task steps")
        );
        Ok(())
    }

    #[test]
    fn operator_context_rejects_registry_child_without_registry_env() -> Result<(), Box<dyn Error>>
    {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_entry_graph(&entry, "registry:acme/child@1.0.0", "")?;

        let error = operator_context_error(
            load_skill_operator_context_chain(
                &entry,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "missing registry env must fail",
        )?;
        assert!(
            error
                .to_string()
                .contains("RUNX_REGISTRY_DIR is not configured")
        );
        Ok(())
    }

    #[test]
    fn operator_context_includes_admitted_registry_child_provenance() -> Result<(), Box<dyn Error>>
    {
        use crate::registry::{
            FileRegistryStore, IngestSkillOptions, RegistryPackageFile, ingest_skill_markdown,
        };

        let temp = tempdir()?;
        let registry_dir = temp.path().join("registry");
        let store = FileRegistryStore::new(&registry_dir);
        ingest_skill_markdown(
            &store,
            "---\nname: registry-child\n---\n# Registry Child\n",
            IngestSkillOptions {
                owner: Some("acme".to_owned()),
                version: Some("1.0.0".to_owned()),
                created_at: Some("2026-07-12T00:00:00Z".to_owned()),
                profile_document: Some(
                    "skill: registry-child\nrunners:\n  agent:\n    default: true\n    type: agent-task\n    agent: reviewer\n    task: review\n"
                        .to_owned(),
                ),
                package_files: vec![RegistryPackageFile {
                    path: "references/rubric.md".to_owned(),
                    content: "registry package rubric".to_owned(),
                }],
                ..IngestSkillOptions::default()
            },
        )?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_entry_graph(&entry, "registry:acme/registry-child@1.0.0", "")?;
        let env = [(
            "RUNX_REGISTRY_DIR".to_owned(),
            registry_dir.to_string_lossy().into_owned(),
        )]
        .into_iter()
        .collect();

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(env, temp.path().to_path_buf()),
        )?;
        let child = chain.entry.steps[0]
            .child
            .as_ref()
            .ok_or("missing registry child")?;
        let registry = child
            .package
            .registry
            .as_ref()
            .ok_or("missing registry provenance")?;
        assert_eq!(registry.reference, "registry:acme/registry-child@1.0.0");
        assert_eq!(registry.skill_id, "acme/registry-child");
        assert_eq!(registry.version, "1.0.0");
        assert!(registry.digest.starts_with("sha256:"));
        assert!(registry.package_digest.is_some());
        assert_eq!(registry.trust_tier, "community");
        assert!(!registry.source.is_empty());
        assert!(!registry.source_label.is_empty());
        Ok(())
    }

    #[test]
    fn operator_context_rejects_cycles_and_depth_overflow() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_entry_graph(&entry, ".", "")?;
        let error = operator_context_error(
            load_skill_operator_context_chain(
                &entry,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "cycle must fail",
        )?;
        assert!(error.to_string().contains("contains a cycle"));

        let mut previous = temp.path().join("deep-entry");
        write_skill(&previous, "entry", "# Deep")?;
        let root = previous.clone();
        for index in 0..=MAX_CHAIN_DEPTH {
            let next = previous.join(format!("child-{index}"));
            write_skill(&next, "entry", "# Child")?;
            write_entry_graph(&previous, &format!("./child-{index}"), "")?;
            previous = next;
        }
        write_file(
            &previous.join("X.yaml"),
            "skill: entry\nrunners:\n  agent:\n    default: true\n    type: agent-task\n    agent: reviewer\n    task: done\n",
        )?;
        let error = operator_context_error(
            load_skill_operator_context_chain(
                &root,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "depth overflow must fail",
        )?;
        assert!(
            error.to_string().contains("exceeds maximum depth"),
            "unexpected depth error: {error}"
        );
        Ok(())
    }

    #[test]
    fn operator_context_allows_repeated_dag_child_and_enforces_size_limits()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child")?;
        write_file(
            &child.join("X.yaml"),
            "skill: child\nrunners:\n  agent:\n    default: true\n    type: agent-task\n    agent: reviewer\n    task: review\n",
        )?;
        write_file(
            &entry.join("X.yaml"),
            "skill: entry\nrunners:\n  main:\n    default: true\n    type: graph\n    graph:\n      name: entry\n      result_from: [second]\n      steps:\n        - id: first\n          skill: ./child\n        - id: second\n          skill: ./child\n          artifacts:\n            wrap_as: review_result\n",
        )?;
        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        assert_eq!(chain.node_count, 3);
        assert!(chain.entry.steps.iter().all(|step| step.child.is_some()));

        let mut options =
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf());
        options.max_content_bytes = 1;
        let error = operator_context_error(
            load_skill_operator_context_chain(&entry, None, options),
            "content size limit must fail",
        )?;
        assert!(error.to_string().contains("maximum content bytes"));

        let mut options =
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf());
        options.max_nodes = 1;
        let error = operator_context_error(
            load_skill_operator_context_chain(&entry, None, options),
            "node count limit must fail",
        )?;
        assert!(error.to_string().contains("maximum node count"));
        Ok(())
    }

    #[test]
    fn operator_context_surfaces_local_tool_manifest() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_file(
            &entry.join("tools/example/record/manifest.json"),
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.record",
  "source": {
    "type": "cli-tool",
    "command": "true",
    "args": [],
    "input_mode": "none"
  },
  "artifacts": {
    "wrap_as": "record_result"
  }
}
"#,
        )?;
        write_file(
            &entry.join("X.yaml"),
            "skill: entry\nrunners:\n  main:\n    default: true\n    type: graph\n    graph:\n      name: entry\n      result_from: [record]\n      steps:\n        - id: record\n          tool: example.record\n          idempotency_key: record-1\n",
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        assert_eq!(chain.entry.steps[0].tool_refs, ["example.record"]);
        assert_eq!(chain.entry.tools.len(), 1);
        assert_eq!(chain.entry.tools[0].name, "example.record");
        assert_eq!(chain.entry.tools[0].source, "local-manifest");
        assert!(chain.entry.tools[0].declared_artifact_output);
        assert_eq!(
            chain.entry.tools[0].execution_boundary.kind,
            ExecutionBoundaryKind::TrustedHostProcess
        );
        assert!(
            chain.entry.tools[0]
                .content
                .as_deref()
                .is_some_and(|content| content.contains("cli-tool"))
        );
        Ok(())
    }

    #[test]
    fn operator_context_rejects_result_tool_without_semantic_output_before_execution()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let sentinel = temp.path().join("tool-ran");
        write_skill(&entry, "entry", "# Entry")?;
        write_file(
            &entry.join("tools/example/record/manifest.json"),
            r#"{
  "schema": "runx.tool.manifest.v1",
  "name": "example.record",
  "source": {
    "type": "cli-tool",
    "command": "sh",
    "args": ["-c", "touch \"$RUNX_CWD/tool-ran\""],
    "input_mode": "none"
  }
}
"#,
        )?;
        write_file(
            &entry.join("X.yaml"),
            "skill: entry\nrunners:\n  main:\n    default: true\n    type: graph\n    graph:\n      name: entry\n      result_from: [record]\n      steps:\n        - id: record\n          tool: example.record\n          idempotency_key: record-1\n",
        )?;

        let error = operator_context_error(
            load_skill_operator_context_chain(
                &entry,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "missing result contract must fail during operator-context preparation",
        )?;

        assert!(
            error
                .to_string()
                .contains("entry.record' declares no semantic output contract"),
            "unexpected preflight error: {error}"
        );
        assert!(!sentinel.exists(), "preflight executed the mutating tool");
        Ok(())
    }

    #[test]
    fn operator_context_surfaces_typed_native_data_operation() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_file(
            &entry.join("X.yaml"),
            "skill: entry\nrunners:\n  main:\n    default: true\n    type: graph\n    graph:\n      name: entry\n      result_from: [append]\n      steps:\n        - id: append\n          tool: data.append_event\n",
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;

        assert_eq!(chain.entry.tools.len(), 1);
        assert_eq!(chain.entry.tools[0].name, "data.append_event");
        assert_eq!(chain.entry.tools[0].source, "native-runtime");
        assert!(chain.entry.tools[0].path.is_none());
        assert!(
            chain.entry.tools[0]
                .content
                .as_deref()
                .is_some_and(|content| content.contains("runx-runtime/event-store"))
        );
        Ok(())
    }

    #[test]
    #[cfg(feature = "catalog")]
    fn operator_context_surfaces_native_tool_contract() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_file(
            &entry.join("X.yaml"),
            "skill: entry\nrunners:\n  main:\n    default: true\n    type: graph\n    graph:\n      name: entry\n      result_from: [apply]\n      steps:\n        - id: apply\n          tool: runx.skill.apply\n          idempotency_key: apply-1\n",
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;

        assert_eq!(chain.entry.tools.len(), 1);
        assert_eq!(chain.entry.tools[0].name, "runx.skill.apply");
        assert_eq!(chain.entry.tools[0].source, "native-runtime");
        assert!(chain.entry.tools[0].path.is_none());
        assert!(
            chain.entry.tools[0]
                .content
                .as_deref()
                .is_some_and(|content| content.contains("runx.skill.apply"))
        );
        assert!(chain.entry.tools[0].sha256.is_some());
        Ok(())
    }

    fn write_skill(dir: &Path, name: &str, body: &str) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(dir)?;
        write_file(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\n---\n{body}\n"),
        )
    }

    fn write_entry_graph(dir: &Path, child_ref: &str, extra: &str) -> Result<(), Box<dyn Error>> {
        write_file(
            &dir.join("X.yaml"),
            &format!(
                "skill: entry\nrunners:\n  main:\n    default: true\n    type: graph\n    graph:\n      name: entry\n      result_from: [child]\n      steps:\n        - id: child\n          skill: {child_ref}\n          artifacts:\n            wrap_as: child_result\n{extra}"
            ),
        )
    }

    fn write_file(path: &Path, content: &str) -> Result<(), Box<dyn Error>> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }

    fn operator_context_error(
        result: Result<SkillOperatorContextChain, SkillRunError>,
        message: &'static str,
    ) -> Result<SkillRunError, Box<dyn Error>> {
        match result {
            Ok(_) => Err(message.into()),
            Err(error) => Ok(error),
        }
    }
}
