// Module rationale: transitive preflight discovery keeps traversal,
// admission provenance, limits, and its focused fixtures in one auditable module.
//! Runtime-owned preflight expansion for the complete skill chain shown to an
//! operator before execution. Discovery uses the same validated workflow, child
//! skill, registry admission, and context-skill loaders as execution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{
    ContextEntry, ExecutionBoundaryKind, ExecutionBoundaryObservation, JsonObject, JsonValue,
    sha256_prefixed,
};
use runx_parser::{
    LoadTarget, SkillMethodDefinition, StepKind, ToolTarget, Workflow, WorkflowStep,
};
use serde::{Deserialize, Serialize};

use crate::RuntimeEffectRegistry;
use crate::RuntimeError;
use crate::services::{WorkspaceEnv, merge_inferred_tool_roots};
use crate::tool_catalogs::{ToolCatalogError, ToolInspectOptions, resolve_local_tool};

use super::workflow::{
    LoadedStepSkill, LoadedStepSkillRegistryProvenance, StepSkillLoadOptions, child_skill_steps,
    load_step_skill,
};
use super::skill_context::load_context_skills;
use super::skill_front::SkillRunError;
use super::skill_front::method_manifest::selected_method;

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
    pub method: SkillOperatorContextMethod,
    pub steps: Vec<SkillOperatorContextStep>,
    pub tools: Vec<SkillOperatorContextTool>,
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
    pub bundle_digest: Option<String>,
    /// The registry key its signature verified under. Its tier is a
    /// statement apart, read only when a decision needs it (D30).
    pub key_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOperatorContextDocument {
    pub path: Option<PathBuf>,
    pub source_label: String,
    pub digest: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextMethod {
    pub name: String,
    pub selection: String,
    pub requested_name: Option<String>,
    pub raw: JsonValue,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextStep {
    pub node_path: String,
    pub definition: WorkflowStep,
    pub context_skills: Vec<SkillOperatorContextContextSkill>,
    pub tools: Vec<String>,
    pub execution_boundary: Option<ExecutionBoundaryObservation>,
    /// The child skills the step may run: a skill step's target, or each
    /// child skill an agent turn's allowance names.
    pub children: Vec<SkillOperatorContextNode>,
}

impl SkillOperatorContextStep {
    #[must_use]
    pub fn target_label(&self) -> String {
        match &self.definition.kind {
            StepKind::Skill(target) => match &target.method {
                Some(method) => format!("skill {} method {method}", target.skill),
                None => format!("skill {}", target.skill),
            },
            StepKind::Tool(ToolTarget::Catalog(name)) => format!("tool {name}"),
            StepKind::Tool(ToolTarget::Worker(worker)) => format!("worker {}", worker.module),
            StepKind::Tool(ToolTarget::Process(process)) => {
                format!("process {}", process.command)
            }
            StepKind::Operation(target) => match &target.operation {
                Some(operation) => format!("operation {}/{operation}", target.requirement),
                None => format!("operation {}", target.requirement),
            },
            StepKind::Agent(agent) => format!("agent task {}", agent.task),
            StepKind::Ask(ask) => format!("ask {}", ask.gate),
            StepKind::Load(load) => match &load.target {
                LoadTarget::Skill { skill, .. } => format!("load skill {skill}"),
                LoadTarget::Connector { name } => format!("load connector {name}"),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkillOperatorContextContextSkill {
    pub reference: String,
    pub artifact_digest: String,
    pub artifact_bytes: u64,
    pub artifact: JsonObject,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillOperatorContextTool {
    pub name: String,
    pub source: String,
    pub path: Option<PathBuf>,
    pub digest: Option<String>,
    pub content: Option<String>,
    pub declared_artifact_output: bool,
    pub execution_boundary: ExecutionBoundaryObservation,
}

/// A chain that could not be expanded, and the node it stopped at.
#[derive(Debug)]
pub(crate) struct BlockedChain {
    /// The deepest node whose expansion failed, `entry` for the entry skill.
    pub(crate) node_path: String,
    pub(crate) error: SkillRunError,
}

pub(crate) fn load_skill_operator_context_chain_from_package(
    loaded: &crate::LoadedSkillPackage,
    selected_method_name: Option<&str>,
    options: SkillOperatorContextOptions,
) -> Result<SkillOperatorContextChain, Box<BlockedChain>> {
    let mut blocked_at = None;
    expand_chain(loaded, selected_method_name, options, &mut blocked_at).map_err(|error| {
        Box::new(BlockedChain {
            node_path: blocked_at.unwrap_or_else(|| "entry".to_owned()),
            error,
        })
    })
}

fn expand_chain(
    loaded: &crate::LoadedSkillPackage,
    selected_method_name: Option<&str>,
    options: SkillOperatorContextOptions,
    blocked_at: &mut Option<String>,
) -> Result<SkillOperatorContextChain, SkillRunError> {
    let manifest = loaded.manifest().cloned().ok_or_else(|| {
        SkillRunError::Invalid(format!(
            "skill package {} does not declare run.yaml methods",
            loaded.directory.display()
        ))
    })?;
    let skill_dir = loaded.directory.clone();
    let manual_path = loaded.package_root.join("SKILL.md");
    let manual_markdown = loaded.package.manual_markdown.clone();
    let manual_digest = loaded.package.manual_digest.clone();
    let method = selected_method(&manifest, selected_method_name)?.clone();
    let workspace =
        WorkspaceEnv::new(options.env.clone(), options.cwd.clone()).map_err(RuntimeError::from)?;
    let env = workspace.skill_env_for_skill(&skill_dir);
    let mut state = ExpansionState::new(options);
    let entry = state.expand_method_node(NodeInput {
        node_path: "entry".to_owned(),
        package: local_package(&skill_dir, None),
        skill_dir,
        manual_path,
        manual_markdown,
        manual_digest,
        method,
        requested_method: selected_method_name.map(str::to_owned),
        env,
        depth: 0,
    });
    *blocked_at = state.blocked_at.take();
    let entry = entry?;
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
    /// The deepest node whose expansion failed.
    blocked_at: Option<String>,
}

struct NodeInput {
    node_path: String,
    package: SkillOperatorContextPackage,
    skill_dir: PathBuf,
    manual_path: PathBuf,
    manual_markdown: String,
    manual_digest: String,
    method: SkillMethodDefinition,
    requested_method: Option<String>,
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
            blocked_at: None,
        }
    }

    fn expand_method_node(
        &mut self,
        input: NodeInput,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        let result = self.expand_admitted_node(&input);
        // The deepest failing node records itself first as the error unwinds.
        if result.is_err() && self.blocked_at.is_none() {
            self.blocked_at = Some(input.node_path.clone());
        }
        result
    }

    fn expand_admitted_node(
        &mut self,
        input: &NodeInput,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        self.admit_node(input.depth)?;
        let identity = node_identity(&input.package, &input.skill_dir, &input.method.name)?;
        if !self.ancestry.insert(identity.clone()) {
            return Err(blocked(format!(
                "operator context chain contains a cycle at {} ({identity})",
                input.node_path
            )));
        }

        let result = self.build_method_node(input);
        self.ancestry.remove(&identity);
        result
    }

    fn build_method_node(
        &mut self,
        input: &NodeInput,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        let skill_markdown = self.manual_document(
            &input.manual_path,
            &input.manual_markdown,
            &input.manual_digest,
        )?;
        let raw = JsonValue::Object(input.method.raw.clone());
        self.add_bytes(serialized_bytes(&raw)?)?;
        let workflow = &input.method.workflow;
        let tool_names = referenced_tools(workflow);
        let tools = self.load_tools(&input.skill_dir, &tool_names, &input.env)?;
        let steps = self.expand_workflow_steps(
            &input.node_path,
            &input.skill_dir,
            workflow,
            &input.env,
            input.depth,
        )?;
        validate_workflow_result_contracts(&input.node_path, workflow, &steps, &tools)?;
        Ok(SkillOperatorContextNode {
            node_path: input.node_path.clone(),
            package: input.package.clone(),
            skill_markdown,
            method: method_context(&input.method, input.requested_method.as_deref(), raw),
            steps,
            tools,
        })
    }

    fn expand_workflow_steps(
        &mut self,
        parent_path: &str,
        workflow_dir: &Path,
        workflow: &Workflow,
        env: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<Vec<SkillOperatorContextStep>, SkillRunError> {
        workflow
            .steps
            .iter()
            .map(|step| self.expand_workflow_step(parent_path, workflow_dir, step, env, depth))
            .collect()
    }

    // Function rationale: one workflow-step expansion must resolve
    // its exclusive target, attached context, child provenance, and recursion together.
    fn expand_workflow_step(
        &mut self,
        parent_path: &str,
        workflow_dir: &Path,
        step: &WorkflowStep,
        env: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<SkillOperatorContextStep, SkillRunError> {
        let node_path = format!("{parent_path}.{}", step.id);
        self.add_bytes(serialized_bytes(step)?)?;
        // Catalog tool admission and manifest resolution are handled for the
        // containing node after every step is expanded; the kernel admits an
        // operation when it runs.
        let context_skills = match &step.kind {
            StepKind::Agent(_) => self.load_step_context(workflow_dir, step, env)?,
            _ => Vec::new(),
        };
        let mut children = Vec::new();
        for child in child_skill_steps(step) {
            let Some(target) = child.skill() else {
                continue;
            };
            let loaded =
                load_step_skill(workflow_dir, &child, StepSkillLoadOptions::from_disk(env))?;
            let child_path = match &step.kind {
                StepKind::Skill(_) => node_path.clone(),
                _ => format!("{node_path}[{}]", target.skill),
            };
            children.push(self.expand_loaded_child(
                child_path,
                &target.skill,
                target.method.as_deref(),
                loaded,
                env,
                depth + 1,
            )?);
        }
        let tools = step_tools(step);
        let execution_boundary = match &step.kind {
            StepKind::Tool(ToolTarget::Catalog(tool)) => {
                crate::tool_catalogs::native::execution_boundary(tool, &self.options.effects)
                    .map(boundary)
            }
            StepKind::Tool(ToolTarget::Worker(_)) => {
                Some(boundary(ExecutionBoundaryKind::DeterministicWorker))
            }
            StepKind::Tool(ToolTarget::Process(_)) => {
                Some(boundary(ExecutionBoundaryKind::TrustedHostProcess))
            }
            StepKind::Agent(_) => Some(boundary(ExecutionBoundaryKind::RemoteProvider)),
            StepKind::Operation(_) | StepKind::Ask(_) | StepKind::Skill(_) | StepKind::Load(_) => {
                None
            }
        };
        Ok(SkillOperatorContextStep {
            node_path,
            definition: step.clone(),
            context_skills,
            tools,
            execution_boundary,
            children,
        })
    }

    fn expand_loaded_child(
        &mut self,
        node_path: String,
        reference: &str,
        requested_method: Option<&str>,
        loaded: LoadedStepSkill,
        env: &BTreeMap<String, String>,
        depth: usize,
    ) -> Result<SkillOperatorContextNode, SkillRunError> {
        let package = loaded_package(&loaded, reference);
        let mut child_env = env.clone();
        merge_inferred_tool_roots(&mut child_env, &loaded.directory);
        self.expand_method_node(NodeInput {
            node_path,
            package,
            skill_dir: loaded.directory,
            manual_path: loaded.manual_path,
            manual_markdown: loaded.manual_markdown.as_ref().to_owned(),
            manual_digest: loaded.manual_digest,
            method: loaded.method,
            requested_method: requested_method.map(str::to_owned),
            env: child_env,
            depth,
        })
    }

    fn load_step_context(
        &mut self,
        workflow_dir: &Path,
        step: &WorkflowStep,
        env: &BTreeMap<String, String>,
    ) -> Result<Vec<SkillOperatorContextContextSkill>, SkillRunError> {
        let entries = load_context_skills(
            &step.id,
            workflow_dir,
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
            .get("skill")
            .and_then(JsonValue::as_str)
            .ok_or_else(|| blocked("resolved context skill is missing string field skill"))?;
        if resolved_reference != reference {
            return Err(blocked(format!(
                "resolved context skill '{resolved_reference}' does not match declared skill '{reference}'"
            )));
        }
        let artifact_bytes = usize::try_from(entry.origin.size_bytes)
            .map_err(|_| blocked("resolved context skill artifact size exceeds this platform"))?;
        self.add_bytes(artifact_bytes)?;
        Ok(SkillOperatorContextContextSkill {
            reference: reference.to_owned(),
            artifact_digest: entry.origin.digest.as_str().to_owned(),
            artifact_bytes: entry.origin.size_bytes,
            artifact: entry.data,
        })
    }

    fn manual_document(
        &mut self,
        path: &Path,
        content: &str,
        digest: &str,
    ) -> Result<SkillOperatorContextDocument, SkillRunError> {
        self.add_bytes(content.len())?;
        Ok(SkillOperatorContextDocument {
            path: Some(path.to_path_buf()),
            source_label: path.to_string_lossy().into_owned(),
            digest: digest.to_owned(),
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
                let workspace_root = crate::RunxPaths::resolve(env, skill_dir)
                    .workspace()
                    .to_path_buf();
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
                        digest: Some(sha256_prefixed(content.as_bytes())),
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
                            digest: Some(sha256_prefixed(tool.content.as_bytes())),
                            content: Some(tool.content),
                            declared_artifact_output:
                                crate::output_contract::declares_output_contract(
                                    tool.outputs.as_ref(),
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

/// The method a node runs: the one its reference named, or the package's
/// default, its one method or its `default_method`.
fn method_context(
    method: &SkillMethodDefinition,
    requested_method: Option<&str>,
    raw: JsonValue,
) -> SkillOperatorContextMethod {
    let selection = if requested_method.is_some() {
        "requested"
    } else {
        "default"
    };
    SkillOperatorContextMethod {
        name: method.name.clone(),
        selection: selection.to_owned(),
        requested_name: requested_method.map(str::to_owned),
        raw,
    }
}

const fn boundary(kind: ExecutionBoundaryKind) -> ExecutionBoundaryObservation {
    ExecutionBoundaryObservation { kind }
}

fn validate_workflow_result_contracts(
    node_path: &str,
    workflow: &Workflow,
    steps: &[SkillOperatorContextStep],
    tools: &[SkillOperatorContextTool],
) -> Result<(), SkillRunError> {
    for result_step_id in &workflow.result_from {
        let step = steps
            .iter()
            .find(|step| step.definition.id == *result_step_id)
            .ok_or_else(|| {
                blocked(format!(
                    "operator context workflow result producer '{node_path}.{result_step_id}' was not expanded"
                ))
            })?;
        if !step_declares_result(step, tools) {
            return Err(blocked(format!(
                "workflow result producer '{}' declares no semantic output contract; declare its outputs, or the output result_as names, before execution",
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
    let definition = &step.definition;
    match &definition.kind {
        // An ask resolves to its decision; an operation always emits its
        // `runx.provider.operation.v1` packet; a child skill's own workflow was
        // validated to declare its result when it was expanded.
        StepKind::Ask(_) | StepKind::Operation(_) | StepKind::Skill(_) | StepKind::Load(_) => true,
        StepKind::Agent(_) | StepKind::Tool(ToolTarget::Worker(_) | ToolTarget::Process(_)) => {
            crate::output_contract::declares_output_contract(
                definition.outputs.as_ref(),
                definition.artifacts.as_ref(),
            )
        }
        StepKind::Tool(ToolTarget::Catalog(tool_name)) => {
            crate::output_contract::declares_output_contract(None, definition.artifacts.as_ref())
                || (definition.artifacts.is_none()
                    && tools
                        .iter()
                        .find(|tool| tool.name == *tool_name)
                        .is_some_and(|tool| tool.declared_artifact_output))
        }
    }
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

/// A registry child's source, as its context names it.
const REGISTRY_SOURCE: &str = "runx-registry";
const REGISTRY_SOURCE_LABEL: &str = "runx registry";

fn loaded_package(loaded: &LoadedStepSkill, reference: &str) -> SkillOperatorContextPackage {
    match loaded.registry.as_ref() {
        Some(registry) => SkillOperatorContextPackage {
            directory: loaded.directory.clone(),
            reference: Some(reference.to_owned()),
            source: REGISTRY_SOURCE.to_owned(),
            source_label: REGISTRY_SOURCE_LABEL.to_owned(),
            registry: Some(registry_context(registry)),
        },
        None => local_package(&loaded.directory, Some(reference.to_owned())),
    }
}

fn registry_context(value: &LoadedStepSkillRegistryProvenance) -> SkillOperatorContextRegistry {
    SkillOperatorContextRegistry {
        reference: value.reference.clone(),
        source: REGISTRY_SOURCE.to_owned(),
        source_label: REGISTRY_SOURCE_LABEL.to_owned(),
        skill_id: value.signed.name.clone(),
        version: value.signed.version.clone(),
        digest: value.digest.clone(),
        bundle_digest: value.bundle_digest.clone(),
        key_id: value.signed.key_id.clone(),
    }
}

fn node_identity(
    package: &SkillOperatorContextPackage,
    skill_dir: &Path,
    method_name: &str,
) -> Result<String, SkillRunError> {
    if let Some(registry) = &package.registry {
        return Ok(format!(
            "registry:{}@{}:{}:{}",
            registry.skill_id,
            registry.version,
            registry
                .bundle_digest
                .as_deref()
                .unwrap_or(&registry.digest),
            method_name
        ));
    }
    let canonical = fs::canonicalize(skill_dir).map_err(|source| {
        RuntimeError::io(
            format!("canonicalizing skill directory {}", skill_dir.display()),
            source,
        )
    })?;
    Ok(format!("local:{}:{method_name}", canonical.display()))
}

fn referenced_tools(workflow: &Workflow) -> BTreeSet<String> {
    workflow.steps.iter().flat_map(step_tools).collect()
}

fn step_tools(step: &WorkflowStep) -> Vec<String> {
    match &step.kind {
        runx_parser::StepKind::Agent(agent) => agent
            .allowance
            .tools
            .iter()
            .map(|tool| tool.as_str().to_owned())
            .collect(),
        _ => step.catalog_tool().map(str::to_owned).into_iter().collect(),
    }
}

struct ResolvedOperatorContextTool {
    path: PathBuf,
    content: String,
    /// The outputs the manifest declares, and the artifacts they keep.
    outputs: Option<runx_contracts::JsonObject>,
    artifacts: Option<runx_parser::OutputArtifacts>,
    execution_boundary: ExecutionBoundaryObservation,
}

fn resolve_referenced_local_tool(
    skill_dir: &Path,
    name: &str,
    env: &BTreeMap<String, String>,
) -> Result<Option<ResolvedOperatorContextTool>, SkillRunError> {
    let options = ToolInspectOptions {
        root: crate::RunxPaths::resolve(env, skill_dir)
            .workspace()
            .to_path_buf(),
        tool: name.to_owned(),
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
        Ok(resolution) => Ok(Some(ResolvedOperatorContextTool {
            path: resolution.manifest_path,
            content: resolution.manifest_source,
            outputs: resolution.tool.outputs,
            artifacts: resolution.tool.artifacts,
            execution_boundary: boundary(ExecutionBoundaryKind::TrustedHostProcess),
        })),
        Err(ToolCatalogError::NotFound(_) | ToolCatalogError::Unqualified(_)) => Ok(None),
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

    fn load_skill_operator_context_chain(
        skill_path: &Path,
        selected_method_name: Option<&str>,
        options: SkillOperatorContextOptions,
    ) -> Result<SkillOperatorContextChain, SkillRunError> {
        let loaded = crate::load_validated_skill_package(skill_path)?;
        load_skill_operator_context_chain_from_package(&loaded, selected_method_name, options)
            .map_err(|blocked| blocked.error)
    }

    /// Every method of every bundled core skill expands its operator context
    /// under the effects the CLI runs with, as a run prepares it.
    #[test]
    fn every_bundled_core_method_expands_its_operator_context() -> Result<(), Box<dyn Error>> {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let bundle: serde_json::Value = serde_json::from_str(&fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("bundled-skills.json"),
        )?)?;
        let skills = bundle["skills"].as_array().ok_or("bundle omitted skills")?;
        let effects = RuntimeEffectRegistry::local()?;
        let mut failures = Vec::new();
        for entry in skills {
            let skill_id = entry["skill_id"]
                .as_str()
                .ok_or("bundle skill omitted id")?;
            let name = skill_id
                .strip_prefix("runx/")
                .ok_or("bundle skill has foreign publisher")?;
            let directory = repo_root.join("skills").join(name);
            let loaded = match crate::load_validated_skill_package(&directory) {
                Ok(loaded) => loaded,
                Err(error) => {
                    failures.push(format!("{name}: {error}"));
                    continue;
                }
            };
            let Some(manifest) = loaded.manifest() else {
                continue;
            };
            let roots = [repo_root.join("tools"), directory.join("tools")]
                .into_iter()
                .filter(|path| path.is_dir())
                .collect::<Vec<_>>();
            let mut env = BTreeMap::new();
            if !roots.is_empty() {
                env.insert(
                    "RUNX_TOOL_ROOTS".to_owned(),
                    std::env::join_paths(roots)?.to_string_lossy().into_owned(),
                );
            }
            for method_name in manifest.methods.keys() {
                if let Err(blocked) = load_skill_operator_context_chain_from_package(
                    &loaded,
                    Some(method_name),
                    SkillOperatorContextOptions::new(env.clone(), repo_root.clone())
                        .with_effects(effects.clone()),
                ) {
                    failures.push(format!("{name}#{method_name}: {}", blocked.error));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "every bundled core method must expand its operator context:\n{}",
            failures.join("\n")
        );
        Ok(())
    }

    #[test]
    fn operator_context_expands_local_child_agent_method() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(
            &child,
            "child",
            "# Child contract\n\n## Agent task: review\n\nReview.",
        )?;
        write_file(
            &child.join("run.yaml"),
            r#"minimum_runx: 0.12.0
methods:
  review:
    steps:
      - id: review
        agent: review
        outputs:
          result: {type: object}
"#,
        )?;
        write_file(
            &entry.join("run.yaml"),
            r#"minimum_runx: 0.12.0
methods:
  main:
    result_from: [review]
    steps:
      - id: review
        skill: ./child
        outputs:
          review_result: {}
        result_as: review_result
"#,
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        let child = chain.entry.steps[0]
            .children
            .first()
            .ok_or("missing child")?;
        assert_eq!(child.node_path, "entry.review");
        assert_eq!(child.method.name, "review");
        assert!(child.skill_markdown.content.contains("# Child contract"));
        Ok(())
    }

    #[test]
    fn operator_context_expands_each_child_a_turn_may_call() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_skill(
            &entry,
            "entry",
            "# Entry\n\n## Agent task: answer\n\nAnswer.",
        )?;
        write_skill(
            &child,
            "child",
            "# Child contract\n\n## Agent task: review\n\nReview.",
        )?;
        write_file(
            &child.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  review:\n    steps:\n      - id: review\n        agent: review\n        inputs: $input\n        outputs:\n          verdict: {type: string}\n",
        )?;
        write_file(
            &entry.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  answer:\n    steps:\n      - id: answer\n        agent: answer\n        calls:\n          skills: [./child]\n        inputs: $input\n        outputs:\n          answer: {type: string}\n",
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        let [child] = chain.entry.steps[0].children.as_slice() else {
            return Err("the turn's child skill is expanded once".into());
        };
        assert_eq!(child.node_path, "entry.answer[./child]");
        assert!(child.skill_markdown.content.contains("# Child contract"));
        Ok(())
    }

    #[test]
    fn operator_context_uses_child_workflow_dir_for_inner_context() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        let rubric = child.join("context/rubric");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child\n\n## Agent task: judge\n\nJudge.")?;
        write_skill(&rubric, "rubric", "child-local rubric")?;
        write_file(
            &child.join("run.yaml"),
            r#"minimum_runx: 0.12.0
methods:
  graph:
    result_from: [judge]
    steps:
      - id: judge
        agent: judge
        context:
          skills:
            - ./context/rubric
        outputs:
          judgment: {}
        result_as: judgment
"#,
        )?;
        write_entry_workflow(&entry, "./child", "")?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        let child = chain.entry.steps[0]
            .children
            .first()
            .ok_or("missing child")?;
        let context = &child.steps[0].context_skills[0];
        assert_eq!(context.reference, "./context/rubric");
        assert_eq!(
            context.artifact.get("path").and_then(JsonValue::as_str),
            rubric.join("SKILL.md").canonicalize()?.to_str()
        );
        assert_eq!(
            context.artifact.get("content").and_then(JsonValue::as_str),
            Some("---\nname: rubric\ndescription: rubric fixture.\n---\nchild-local rubric\n")
        );
        assert!(context.artifact.contains_key("manual_digest"));
        Ok(())
    }

    #[test]
    fn operator_context_rejects_context_on_skill_step() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        let child = entry.join("child");
        write_skill(&entry, "entry", "# Entry")?;
        write_skill(&child, "child", "# Child\n\n## Agent task: judge\n\nJudge.")?;
        write_skill(&entry.join("context/rubric"), "rubric", "rubric")?;
        write_file(
            &child.join("run.yaml"),
            r#"minimum_runx: 0.12.0
methods:
  graph:
    result_from: [judge]
    steps:
      - id: judge
        agent: judge
        outputs:
          result: {type: object}
"#,
        )?;
        write_entry_workflow(
            &entry,
            "./child",
            "        context:\n          skills:\n            - ./context/rubric\n",
        )?;

        let error = operator_context_error(
            load_skill_operator_context_chain(
                &entry,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "child workflow context must fail",
        )?;
        assert!(
            error
                .to_string()
                .contains("context does not apply to this skill step")
        );
        Ok(())
    }

    #[test]
    fn operator_context_rejects_a_registry_child_the_project_does_not_hold()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_entry_workflow(&entry, "registry:acme/child@1.0.0", "")?;

        let error = operator_context_error(
            load_skill_operator_context_chain(
                &entry,
                None,
                SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
            ),
            "an unheld registry child must fail",
        )?;
        assert!(
            error
                .to_string()
                .contains("is not installed in this project")
        );
        Ok(())
    }

    /// A registry child the project holds keeps its registry provenance:
    /// its identity as its signature states it, and the digests of its
    /// installed files.
    #[test]
    fn operator_context_reads_an_installed_registry_childs_provenance_from_its_signature()
    -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let project = temp.path().join(".runx");
        let child = project.join("added-skills/acme/registry-child/1.0.0");
        write_skill(
            &child,
            "registry-child",
            "# Registry Child\n\n## Agent task: review\n\nReview.",
        )?;
        write_file(
            &child.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  agent:\n    steps:\n      - id: agent\n        agent: review\n        outputs:\n          result: {type: object}\n",
        )?;
        write_file(
            &child.join("references/rubric.md"),
            "registry package rubric",
        )?;
        crate::registry::signature::testing::sign(
            &child,
            runx_contracts::RegistrySubjectKind::Skill,
            "acme/registry-child",
            "1.0.0",
        )?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_entry_workflow(&entry, "registry:acme/registry-child@1.0.0", "")?;
        let mut env = crate::registry::signature::testing::trusting("acme")?;
        env.insert(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            temp.path().to_string_lossy().into_owned(),
        );
        env.insert(
            crate::paths::RUNX_HOME_ENV.to_owned(),
            temp.path().join("home").to_string_lossy().into_owned(),
        );

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(env, temp.path().to_path_buf()),
        )?;
        let child = chain.entry.steps[0]
            .children
            .first()
            .ok_or("missing registry child")?;
        let registry = child
            .package
            .registry
            .as_ref()
            .ok_or("missing registry provenance")?;
        assert_eq!(registry.reference, "registry:acme/registry-child@1.0.0");
        assert_eq!(registry.skill_id, "acme/registry-child");
        assert_eq!(registry.version, "1.0.0");
        assert_eq!(registry.key_id, crate::registry::signature::testing::KEY_ID);
        assert_eq!(registry.source, "runx-registry");
        assert_eq!(
            registry.digest,
            runx_contracts::sha256_prefixed(
                b"---\nname: registry-child\ndescription: registry-child fixture.\n---\n# Registry Child\n\n## Agent task: review\n\nReview.\n"
            )
        );
        assert_eq!(
            registry.bundle_digest.as_deref(),
            Some(
                runx_contracts::sha256_prefixed(
                    br#"{"files":[{"content":"registry package rubric","path":"references/rubric.md"}]}"#
                )
                .as_str()
            )
        );
        assert_eq!(child.package.source, "runx-registry");
        Ok(())
    }

    #[test]
    fn operator_context_rejects_cycles_and_depth_overflow() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_entry_workflow(&entry, ".", "")?;
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
            write_entry_workflow(&previous, &format!("./child-{index}"), "")?;
            previous = next;
        }
        write_skill(
            &previous,
            "entry",
            "# Child\n\n## Agent task: done\n\nDone.",
        )?;
        write_file(
            &previous.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  agent:\n    steps:\n      - id: agent\n        agent: done\n        outputs:\n          result: {type: object}\n",
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
        write_skill(
            &child,
            "child",
            "# Child\n\n## Agent task: review\n\nReview.",
        )?;
        write_file(
            &child.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  agent:\n    steps:\n      - id: agent\n        agent: review\n        outputs:\n          result: {type: object}\n",
        )?;
        write_file(
            &entry.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    result_from: [second]\n    steps:\n      - id: first\n        skill: ./child\n      - id: second\n        skill: ./child\n        outputs:\n          review_result: {}\n        result_as: review_result\n",
        )?;
        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        assert_eq!(chain.node_count, 3);
        assert!(
            chain
                .entry
                .steps
                .iter()
                .all(|step| step.children.len() == 1)
        );

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
  "process": {
    "command": "true",
    "args": [],
    "inputs_via": "none"
  },
  "outputs": {
    "record_result": {}
  },
  "result_as": "record_result"
}
"#,
        )?;
        write_file(
            &entry.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    result_from: [record]\n    steps:\n      - id: record\n        tool: example.record\n",
        )?;

        let chain = load_skill_operator_context_chain(
            &entry,
            None,
            SkillOperatorContextOptions::new(BTreeMap::new(), temp.path().to_path_buf()),
        )?;
        assert_eq!(chain.entry.steps[0].tools, ["example.record"]);
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
                .is_some_and(|content| content.contains("\"command\": \"true\""))
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
  "process": {
    "command": "sh",
    "args": ["-c", "touch \"$RUNX_CWD/tool-ran\""],
    "inputs_via": "none"
  }
}
"#,
        )?;
        write_file(
            &entry.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    result_from: [record]\n    steps:\n      - id: record\n        tool: example.record\n",
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
    fn operator_context_surfaces_native_tool_contract() -> Result<(), Box<dyn Error>> {
        let temp = tempdir()?;
        let entry = temp.path().join("entry");
        write_skill(&entry, "entry", "# Entry")?;
        write_file(
            &entry.join("run.yaml"),
            "minimum_runx: 0.12.0\nmethods:\n  main:\n    result_from: [apply]\n    steps:\n      - id: apply\n        tool: runx.skill.apply\n",
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
        assert!(chain.entry.tools[0].digest.is_some());
        Ok(())
    }

    fn write_skill(dir: &Path, name: &str, body: &str) -> Result<(), Box<dyn Error>> {
        fs::create_dir_all(dir)?;
        write_file(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: {name} fixture.\n---\n{body}\n"),
        )
    }

    fn write_entry_workflow(
        dir: &Path,
        child_ref: &str,
        extra: &str,
    ) -> Result<(), Box<dyn Error>> {
        write_file(
            &dir.join("run.yaml"),
            &format!(
                "minimum_runx: 0.12.0\nmethods:\n  main:\n    result_from: [child]\n    steps:\n      - id: child\n        skill: {child_ref}\n        outputs:\n          child_result: {{}}\n        result_as: child_result\n{extra}"
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
