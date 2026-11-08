// Module rationale: execution-closure inspection keeps package traversal,
// registry-edge classification, cycle detection, and summary projection in one
// canonical walk.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use runx_contracts::connector::{ConnectorName, Digest};
use runx_contracts::{EnvironmentRequirements, JsonValue, sha256_prefixed};
use runx_parser::{StepKind, ToolTarget, WorkflowStep};
use serde::Serialize;

use super::super::LoadedSkillPackage;
use super::SkillInspectionError;
use crate::identity::PackageIdentity;
use crate::kernel::connectors::LoadedConnector;
use crate::registry::SignedPackage;

#[derive(Default)]
struct ClosureAccumulator {
    components: BTreeSet<String>,
    skill_edges: BTreeSet<String>,
    direct_external_skill_edges: BTreeSet<DirectExternalSkillEdge>,
    unresolved_skill_edges: BTreeSet<String>,
    /// Registry context skills the project does not hold; reported, never
    /// part of the closure.
    unheld_context_registry_refs: BTreeSet<String>,
    /// Every registry ref a step or context skill of the closure names,
    /// held or not.
    registry_refs: BTreeSet<String>,
    package_bindings: BTreeSet<ExecutionPackageBinding>,
    /// Each connector the closure binds, as its package resolved it.
    connector_bindings: BTreeMap<ExecutionConnectorBinding, Arc<LoadedConnector>>,
    local_packages: BTreeMap<PathBuf, BTreeSet<runx_contracts::PacketId>>,
    local_skill_edges: BTreeSet<LocalSkillEdge>,
    /// Every package the closure binds, as it was loaded, by its canonical
    /// directory.
    packages: BTreeMap<PathBuf, Arc<LoadedSkillPackage>>,
    /// Every runner the closure binds, by its package's directory: where a run
    /// may admit an operation.
    runners: BTreeSet<(PathBuf, String)>,
    profiles: BTreeSet<String>,
    environment_required: BTreeSet<String>,
    environment_optional: BTreeSet<String>,
    agent_acts: usize,
    declared_artifact: bool,
    /// Who each package the closure binds is, by its package root, when the
    /// walk binds a run (D30).
    identities: BTreeMap<PathBuf, BoundIdentity>,
    /// Each `project:<name>` the closure binds, and the directory that is it.
    project_roots: BTreeMap<String, PathBuf>,
}

/// Who a bound package is, and whether that is its own identity or the one
/// of the package that carries it.
struct BoundIdentity {
    identity: PackageIdentity,
    own: bool,
}

/// How the walk reached a package, which is what its identity follows from.
enum Reach {
    /// Bound by itself: a run's root, or a fixture package a test runs.
    Root,
    /// A `registry:` reference: its installed copy, verified, or the
    /// person's own copy, which answers their own references (§6.5).
    Registry(Option<SignedPackage>),
    /// `runx/<name>`, the skill this runx bundles.
    Bundled(String),
    /// A relative `skill:` path, incorporated in the package rooted here.
    Incorporated(PathBuf),
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
    pub workflow_directory: PathBuf,
    pub reference: String,
    pub target_package_root: PathBuf,
}

#[derive(Default)]
pub(crate) struct LocalExecutionClosure {
    /// Each package the closure runs, with the packets its runners declare.
    pub packages: BTreeMap<PathBuf, BTreeSet<runx_contracts::PacketId>>,
    pub skill_edges: BTreeSet<LocalSkillEdge>,
}

#[derive(Serialize)]
struct ExecutionClosure {
    closure_digest: String,
    runx_version: String,
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
    connector: ConnectorName,
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
    packet: runx_contracts::PacketId,
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

/// What inspection does with a registry ref the project does not hold.
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
    let method_names = loaded
        .manifest()
        .map(|manifest| manifest.methods.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut inspector = ExecutionClosureInspector::new(loaded, env, unheld_registry_refs)?;
    method_names
        .into_iter()
        .map(|method_name| {
            let closure = inspector.inspect_root_method(&method_name)?;
            Ok((method_name, closure))
        })
        .collect()
}

/// What a run of a runner admits: the closure of `loaded`'s runner
/// `method_name`, every package it binds as it was loaded, by canonical
/// directory, and who each is, by package root, with each signature verified
/// against its exact bytes (D30).
pub(super) type AdmittedClosure = (
    JsonValue,
    BTreeMap<PathBuf, Arc<LoadedSkillPackage>>,
    BTreeMap<PathBuf, PackageIdentity>,
);

pub(super) fn inspect_method_execution_closure(
    loaded: Arc<LoadedSkillPackage>,
    method_name: &str,
    env: &BTreeMap<String, String>,
) -> Result<AdmittedClosure, SkillInspectionError> {
    let mut inspector =
        ExecutionClosureInspector::new(loaded, Some(env), UnheldRegistryRefs::Reject)?;
    inspector.identify = true;
    let mut closure = inspector.inspect_root_method_accumulator(method_name)?;
    let packages = std::mem::take(&mut closure.packages);
    let identities = std::mem::take(&mut closure.identities)
        .into_iter()
        .map(|(root, bound)| (root, bound.identity))
        .collect();
    Ok((serialize_closure(closure)?, packages, identities))
}

/// Every runner the closure of `loaded`'s runner `method_name` binds, its
/// child skills' included, with the package each runs from. A registry child
/// the project does not hold is left out, as the closure reports it.
pub(super) fn inspect_method_closure_methods(
    loaded: Arc<LoadedSkillPackage>,
    method_name: &str,
    env: &BTreeMap<String, String>,
) -> Result<Vec<(Arc<LoadedSkillPackage>, String)>, SkillInspectionError> {
    let mut inspector =
        ExecutionClosureInspector::new(loaded, Some(env), UnheldRegistryRefs::Report)?;
    let closure = inspector.inspect_root_method_accumulator(method_name)?;
    Ok(closure
        .runners
        .iter()
        .filter_map(|(directory, runner)| {
            closure
                .packages
                .get(directory)
                .map(|package| (Arc::clone(package), runner.clone()))
        })
        .collect())
}

pub(super) fn inspect_local_execution_closure(
    loaded: Arc<LoadedSkillPackage>,
    env: &BTreeMap<String, String>,
) -> Result<LocalExecutionClosure, SkillInspectionError> {
    let method_names = loaded
        .manifest()
        .map(|manifest| manifest.methods.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut inspector =
        ExecutionClosureInspector::new(loaded, Some(env), UnheldRegistryRefs::Reject)?;
    let mut closure = LocalExecutionClosure::default();
    for method_name in method_names {
        merge_local_execution_closure(
            &mut closure,
            inspector
                .inspect_root_method_accumulator(&method_name)?
                .into_local_execution_closure(),
        );
    }
    for package in fixture_packages(&inspector.root) {
        merge_local_execution_closure(
            &mut closure,
            inspector
                .inspect_fixture_package_accumulator(&package)?
                .into_local_execution_closure(),
        );
    }
    Ok(closure)
}

/// What a test runs: one of its package's methods.
pub(crate) enum CaseClosureTarget<'a> {
    Runner(&'a str),
}

/// Every connector a harness case's run binds, each as the package that
/// declares it resolves it: the whole closure of the case's runner or graph,
/// its registry children materialized into the case's project as the run
/// materializes them. A child no registry holds is the run's to refuse.
pub(super) fn case_closure_connectors(
    loaded: Arc<LoadedSkillPackage>,
    target: CaseClosureTarget<'_>,
    env: &BTreeMap<String, String>,
) -> Result<Vec<Arc<LoadedConnector>>, SkillInspectionError> {
    let mut inspector =
        ExecutionClosureInspector::new(loaded, Some(env), UnheldRegistryRefs::Report)?;
    let closure = match target {
        CaseClosureTarget::Runner(runner) => inspector.inspect_root_method_accumulator(runner)?,
    };
    Ok(closure.connector_bindings.into_values().collect())
}

/// The fixture packages a package's tests run, each once.
fn fixture_packages(loaded: &LoadedSkillPackage) -> BTreeSet<String> {
    loaded
        .package
        .test_fixtures
        .values()
        .filter_map(|test| test.package.clone())
        .collect()
}

/// A catalog rule as a closure names it.
fn load_rule_label(rule: &runx_contracts::CatalogRule) -> String {
    match rule.catalog {
        Some(catalog) => format!("catalog:{}", catalog.trust.as_str()),
        None => "project".to_owned(),
    }
}

/// The registry refs a package's runners and harness graphs name that the
/// environment's project does not hold: what is fetched into it before the
/// package's closure can be bound.
pub(super) fn unheld_registry_refs(
    loaded: Arc<LoadedSkillPackage>,
    env: &BTreeMap<String, String>,
) -> Result<BTreeSet<String>, SkillInspectionError> {
    // An unresolved edge reads `<ref>#<runner>`.
    Ok(closure_accumulators(loaded, env)?
        .into_iter()
        .flat_map(|accumulator| {
            let context = accumulator.unheld_context_registry_refs;
            accumulator
                .unresolved_skill_edges
                .into_iter()
                .filter_map(|edge| {
                    let reference = edge
                        .rsplit_once('#')
                        .map_or(edge.as_str(), |(reference, _)| reference);
                    runx_parser::registry_ref(reference).map(|_| reference.to_owned())
                })
                .chain(context)
                .collect::<Vec<_>>()
        })
        .collect())
}

/// Every registry ref a package's runners and harness graphs name, through
/// the children `env`'s project holds: the registry skills a run of it in
/// that project reads.
pub(super) fn closure_registry_refs(
    loaded: Arc<LoadedSkillPackage>,
    env: &BTreeMap<String, String>,
) -> Result<BTreeSet<String>, SkillInspectionError> {
    Ok(closure_accumulators(loaded, env)?
        .into_iter()
        .flat_map(|accumulator| accumulator.registry_refs)
        .collect())
}

/// The closure of each of a package's runners and harness graphs, walked in
/// `env`, with registry refs it does not hold reported.
fn closure_accumulators(
    loaded: Arc<LoadedSkillPackage>,
    env: &BTreeMap<String, String>,
) -> Result<Vec<ClosureAccumulator>, SkillInspectionError> {
    let method_names = loaded
        .manifest()
        .map(|manifest| manifest.methods.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let packages = fixture_packages(&loaded);
    let mut inspector =
        ExecutionClosureInspector::new(loaded, Some(env), UnheldRegistryRefs::Report)?;
    let mut accumulators = Vec::new();
    for method_name in method_names {
        accumulators.push(inspector.inspect_root_method_accumulator(&method_name)?);
    }
    for package in packages {
        accumulators.push(inspector.inspect_fixture_package_accumulator(&package)?);
    }
    Ok(accumulators)
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
    /// Each package root's connectors, read once per walk: building a scope
    /// parses every connector manifest the project holds.
    connector_scopes: BTreeMap<PathBuf, Arc<crate::kernel::connectors::ConnectorScope>>,
    /// Whether the walk binds a run, and so sets who each package is.
    identify: bool,
}

impl<'a> ExecutionClosureInspector<'a> {
    fn connector_scope(
        &mut self,
        package_root: &Path,
    ) -> Result<Arc<crate::kernel::connectors::ConnectorScope>, SkillInspectionError> {
        if let Some(scope) = self.connector_scopes.get(package_root) {
            return Ok(Arc::clone(scope));
        }
        let scope = match self.env {
            Some(env) => crate::kernel::connectors::ConnectorScope::in_project(
                package_root,
                &crate::paths::RunxPaths::resolve(env, package_root),
            ),
            None => crate::kernel::connectors::ConnectorScope::at(package_root),
        }
        .map(Arc::new)
        .map_err(|error| SkillInspectionError::ConnectorBinding(error.to_string()))?;
        self.connector_scopes
            .insert(package_root.to_path_buf(), Arc::clone(&scope));
        Ok(scope)
    }

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
            connector_scopes: BTreeMap::new(),
            identify: false,
        })
    }

    /// Sets who the package `loaded` is, reached by `reach`: a package's own
    /// identity stands over one it is carried under, and two directories are
    /// never one `project:<name>`.
    fn record_identity(
        &self,
        loaded: &LoadedSkillPackage,
        package_root: &Path,
        reach: Reach,
        walk: &mut ExecutionWalkState<'_>,
    ) -> Result<(), SkillInspectionError> {
        let Some(env) = self.env.filter(|_| self.identify) else {
            return Ok(());
        };
        let refused = SkillInspectionError::Identity;
        let (identity, own) = match reach {
            Reach::Root | Reach::Registry(None) => (
                crate::identity::skill_identity(loaded, env).map_err(refused)?,
                true,
            ),
            Reach::Registry(Some(signed)) => (PackageIdentity::Signed(signed), true),
            Reach::Bundled(reference) => (
                crate::identity::bundled_skill_identity(&reference, loaded, env)
                    .map_err(refused)?,
                true,
            ),
            Reach::Incorporated(carrier) => {
                if carrier == package_root {
                    return Ok(());
                }
                // Code a package carries binds as that package, and a
                // signature of its own is still checked: one that fails is
                // refused, whatever carries it.
                if crate::registry::carries_signature(package_root) {
                    crate::identity::skill_identity(loaded, env).map_err(refused)?;
                }
                let carried = walk
                    .closure
                    .identities
                    .get(&carrier)
                    .map(|bound| bound.identity.clone())
                    .ok_or_else(|| {
                        refused(format!(
                            "{} is carried by {}, which the closure has not bound",
                            package_root.display(),
                            carrier.display()
                        ))
                    })?;
                (carried, false)
            }
        };
        if walk
            .closure
            .identities
            .get(package_root)
            .is_some_and(|existing| existing.own || !own)
        {
            return Ok(());
        }
        if let (true, PackageIdentity::Project { name }) = (own, &identity) {
            if let Some(other) = walk
                .closure
                .project_roots
                .get(name)
                .filter(|other| other.as_path() != package_root)
            {
                return Err(refused(format!(
                    "two directories bind as {} in one run, {} and {}; give one another name",
                    identity.name(),
                    other.display(),
                    package_root.display()
                )));
            }
            walk.closure
                .project_roots
                .insert(name.clone(), package_root.to_path_buf());
        }
        walk.closure
            .identities
            .insert(package_root.to_path_buf(), BoundIdentity { identity, own });
        Ok(())
    }

    fn inspect_root_method(
        &mut self,
        method_name: &str,
    ) -> Result<JsonValue, SkillInspectionError> {
        serialize_closure(self.inspect_root_method_accumulator(method_name)?)
    }

    fn inspect_root_method_accumulator(
        &mut self,
        method_name: &str,
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
        self.walk_method(
            MethodWalkTarget {
                loaded: self.root.clone(),
                skill_directory: self.root_directory.clone(),
                profile_path,
                method_name: method_name.to_owned(),
                edge_depth: EdgeDepth::Direct,
                materialize_local: true,
                reach: Reach::Root,
            },
            &mut walk,
        )?;
        Ok(closure)
    }

    /// The closure of a fixture package a test runs: every method of the
    /// package at `package`, relative to the root package.
    fn inspect_fixture_package_accumulator(
        &mut self,
        package: &str,
    ) -> Result<ClosureAccumulator, SkillInspectionError> {
        let directory = canonical_directory(&self.package_root.join(package), "fixture package")?;
        let loaded = Arc::new(crate::load_validated_skill_package(&directory)?);
        let mut closure = ClosureAccumulator::default();
        let mut visited = BTreeSet::new();
        let mut walk = ExecutionWalkState {
            closure: &mut closure,
            visited: &mut visited,
        };
        let methods = loaded
            .manifest()
            .map(|manifest| manifest.methods.keys().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for method_name in methods {
            self.walk_method(
                MethodWalkTarget {
                    loaded: loaded.clone(),
                    skill_directory: directory.clone(),
                    profile_path: runx_parser::PROFILE_FILE.to_owned(),
                    method_name,
                    edge_depth: EdgeDepth::Direct,
                    materialize_local: true,
                    reach: Reach::Root,
                },
                &mut walk,
            )?;
        }
        Ok(closure)
    }

    fn walk_method(
        &mut self,
        target: MethodWalkTarget,
        walk: &mut ExecutionWalkState<'_>,
    ) -> Result<(), SkillInspectionError> {
        let MethodWalkTarget {
            loaded,
            skill_directory,
            profile_path,
            method_name,
            edge_depth,
            materialize_local,
            reach,
        } = target;
        let package_root = canonical_directory(&loaded.package_root, "bound skill package")?;
        self.record_identity(&loaded, &package_root, reach, walk)?;
        if !walk.visited.insert((
            skill_directory.clone(),
            method_name.clone(),
            materialize_local,
        )) {
            return Ok(());
        }
        walk.closure
            .packages
            .entry(skill_directory.clone())
            .or_insert_with(|| loaded.clone());
        walk.closure
            .runners
            .insert((skill_directory.clone(), method_name.clone()));
        let scope = self.connector_scope(&package_root)?;
        for (key, requirement) in loaded
            .manifest()
            .into_iter()
            .flat_map(|manifest| &manifest.connectors)
        {
            let references = if requirement.choose.is_some() {
                requirement
                    .from
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
                let binding = ExecutionConnectorBinding {
                    connector: connector.name.clone(),
                    manifest_digest: connector.manifest_digest.clone(),
                    connector_digest: connector.connector_digest.clone(),
                };
                // A closure binds one revision of a name (S15): a grant binds
                // one revision, and nothing chooses between two.
                if let Some(other) = walk.closure.connector_bindings.keys().find(|existing| {
                    existing.connector == binding.connector && **existing != binding
                }) {
                    return Err(SkillInspectionError::ConnectorBinding(format!(
                        "the closure reaches two revisions of connector {} (manifest {} and {}); one closure binds one revision of a name",
                        binding.connector, other.manifest_digest, binding.manifest_digest
                    )));
                }
                walk.closure.connector_bindings.insert(binding, connector);
            }
        }
        let runner = loaded
            .manifest()
            .and_then(|manifest| manifest.methods.get(&method_name))
            .ok_or_else(|| SkillInspectionError::SubSkillNamedMethodMissing {
                path: loaded.directory.clone(),
                runner: method_name.clone(),
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
                        runner: method_name.clone(),
                        problem: "omitted an admitted input packet schema",
                    })?;
                Ok(InputPacketSchemaBinding {
                    packet,
                    schema_digest: schema.schema.digest.clone(),
                })
            })
            .collect::<Result<Vec<_>, SkillInspectionError>>()?;
        walk.closure
            .package_bindings
            .insert(ExecutionPackageBinding {
                skill: loaded.package.skill.name.clone(),
                runner: method_name.clone(),
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
            packet_ids.extend(crate::packet_schemas::declared_method_packet_ids(runner));
            for package_tool in loaded.package.tools.values() {
                packet_ids.extend(crate::packet_schemas::declared_tool_packet_ids(
                    &package_tool.tool,
                ));
            }
        }
        walk.closure
            .profiles
            .insert(format!("{profile_path}#{method_name}"));
        self.walk_workflow(
            loaded.clone(),
            WorkflowWalkContext {
                directory: &skill_directory,
                profile_path: &profile_path,
                edge_depth,
                materialize_local,
            },
            &runner.graph,
            walk,
        )?;
        Ok(())
    }

    fn walk_workflow(
        &mut self,
        loaded: Arc<LoadedSkillPackage>,
        context: WorkflowWalkContext<'_>,
        graph: &runx_parser::Workflow,
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
            walk.closure.registry_refs.extend(
                step.context_skills
                    .iter()
                    .filter(|reference| runx_parser::registry_ref(reference).is_some())
                    .cloned(),
            );
            if self.unheld_registry_refs == UnheldRegistryRefs::Report
                && let Some(env) = self.env
            {
                for reference in &step.context_skills {
                    if runx_parser::registry_ref(reference).is_some()
                        && !crate::execution::skill_context::registry_holds_context_skill(
                            reference, env,
                        )?
                    {
                        walk.closure
                            .unheld_context_registry_refs
                            .insert(reference.clone());
                    }
                }
            }
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
                    // What it may load is the rule's to admit at the call,
                    // so the closure holds the rule.
                    for rule in agent.allowance.rules() {
                        walk.closure
                            .components
                            .insert(format!("load:{}", load_rule_label(rule)));
                    }
                }
                StepKind::Ask(_) | StepKind::Skill(_) | StepKind::Load(_) => {}
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
                if runx_parser::registry_ref(reference).is_some() {
                    walk.closure.registry_refs.insert(reference.to_owned());
                }
                // A bundled name is a requirement of the runtime, never a
                // package the closure carries (D30).
                let carried = runx_parser::registry_ref(reference).is_none()
                    && !crate::registry::is_bundled_reference(reference);
                if context.materialize_local
                    && carried
                    && let Some(nested) = nested.as_ref()
                {
                    let source_package_root =
                        canonical_directory(&loaded.package_root, "bound graph package")?;
                    let target_package_root =
                        canonical_directory(&nested.loaded.package_root, "referenced sub-skill")?;
                    if source_package_root != target_package_root {
                        walk.closure.local_skill_edges.insert(LocalSkillEdge {
                            source_package_root,
                            workflow_directory: canonical_directory(
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
                    let reach = if runx_parser::registry_ref(reference).is_some() {
                        Reach::Registry(nested.signed)
                    } else if crate::registry::is_bundled_reference(reference) {
                        Reach::Bundled(reference.to_owned())
                    } else {
                        Reach::Incorporated(canonical_directory(
                            &loaded.package_root,
                            "bound graph package",
                        )?)
                    };
                    self.walk_method(
                        MethodWalkTarget {
                            loaded: nested.loaded,
                            skill_directory: nested.canonical_directory,
                            profile_path: nested.profile_path,
                            method_name: nested.method_name,
                            edge_depth: EdgeDepth::Nested,
                            materialize_local: context.materialize_local && carried,
                            reach,
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
        workflow_directory: &Path,
        profile_path: &str,
        step: &WorkflowStep,
    ) -> Result<Option<ResolvedStepSkill>, SkillInspectionError> {
        let Some(target) = step.skill() else {
            return Ok(None);
        };
        let reference = target.skill.as_str();
        let requested_method = target.method.as_deref().unwrap_or("default");
        if reference.starts_with('$')
            || (runx_parser::registry_ref(reference).is_some() && self.env.is_none())
        {
            return Ok(Some(ResolvedStepSkill {
                edge: format!("{reference}#{requested_method}"),
                static_external_name: registry_skill_name(reference),
                nested: None,
            }));
        }
        let empty_env = BTreeMap::new();
        let env = self.env.unwrap_or(&empty_env);
        if self.unheld_registry_refs == UnheldRegistryRefs::Report
            && runx_parser::registry_ref(reference).is_some()
            && !crate::execution::graph::registry_holds_step_skill(reference, env)?
        {
            return Ok(Some(ResolvedStepSkill {
                edge: format!("{reference}#{requested_method}"),
                static_external_name: registry_skill_name(reference),
                nested: None,
            }));
        }
        let loaded_step = crate::execution::graph::load_step_skill_package(
            workflow_directory,
            step,
            crate::execution::graph::StepSkillLoadOptions::from_disk(env),
        )?;
        let signed = loaded_step.registry.map(|registry| registry.signed);
        let nested = loaded_step.package;
        let canonical_directory = canonical_directory(&nested.directory, "referenced sub-skill")?;
        let manifest =
            nested
                .manifest()
                .ok_or_else(|| SkillInspectionError::SubSkillManifestMissing {
                    path: nested.directory.clone(),
                })?;
        let nested_method =
            crate::execution::graph::select_step_method(manifest, target.method.as_deref())?
                .name
                .clone();
        let nested_profile = if runx_parser::registry_ref(reference).is_some()
            || crate::registry::is_bundled_reference(reference)
        {
            nested
                .profile_path
                .clone()
                .unwrap_or_else(|| runx_parser::PROFILE_FILE.to_owned())
        } else {
            nested_profile_path(profile_path, reference)?
        };
        Ok(Some(ResolvedStepSkill {
            edge: format!("{}#{nested_method}", nested.package.skill.name),
            static_external_name: None,
            nested: Some(ResolvedNestedSkill {
                loaded: nested,
                canonical_directory,
                profile_path: nested_profile,
                method_name: nested_method,
                signed,
            }),
        }))
    }
}

struct MethodWalkTarget {
    loaded: Arc<LoadedSkillPackage>,
    skill_directory: PathBuf,
    profile_path: String,
    method_name: String,
    edge_depth: EdgeDepth,
    materialize_local: bool,
    reach: Reach,
}

#[derive(Clone, Copy)]
struct WorkflowWalkContext<'a> {
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
    method_name: String,
    /// What a registry child's verified signature says it is.
    signed: Option<SignedPackage>,
}

fn record_direct_external_skill_edge(
    static_external_name: Option<String>,
    nested: Option<&ResolvedNestedSkill>,
    step: &WorkflowStep,
    package_root: &Path,
    edges: &mut BTreeSet<DirectExternalSkillEdge>,
) {
    if let Some(nested) = nested {
        if !nested.canonical_directory.starts_with(package_root) {
            edges.insert(DirectExternalSkillEdge {
                skill: nested.loaded.package.skill.name.clone(),
                runner: nested.method_name.clone(),
            });
        }
    } else if let Some(skill) = static_external_name {
        edges.insert(DirectExternalSkillEdge {
            skill,
            runner: step
                .skill()
                .and_then(|target| target.method.as_deref())
                .unwrap_or("default")
                .to_owned(),
        });
    }
}

/// A skill step's reference; empty for any other step.
fn step_reference(step: &WorkflowStep) -> &str {
    step.skill().map_or("", |target| target.skill.as_str())
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
    let connector_bindings = closure.connector_bindings.into_keys().collect::<Vec<_>>();
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
        runx_version: crate::RUNX_VERSION.to_owned(),
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

/// The digest of what a closure binds: each package and runner, each
/// connector's manifest and implementation, and each edge left unresolved.
/// runx's version is left out, so a person's trust in a closure and a closure
/// pinned elsewhere survive an upgrade.
fn execution_closure_digest(
    package_bindings: &[ExecutionPackageBinding],
    connector_bindings: &[ExecutionConnectorBinding],
    unresolved_skill_edges: &[String],
) -> String {
    let mut canonical = Vec::new();
    canonical.extend_from_slice(b"runx.execution-closure.v1\0");
    for binding in package_bindings {
        append_digest_field(&mut canonical, binding.skill.as_bytes());
        append_digest_field(&mut canonical, binding.runner.as_bytes());
        append_digest_field(&mut canonical, binding.package_digest.as_bytes());
        for packet in &binding.input_packet_schemas {
            append_digest_field(&mut canonical, packet.packet.as_str().as_bytes());
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
    crate::registry::parse_registry_ref(runx_parser::registry_ref(reference)?)
        .skill_id
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The digest is exactly its length-prefixed fields, and no version of
    /// runx is one of them.
    #[test]
    fn a_closure_digest_leaves_runx_version_out() -> Result<(), &'static str> {
        let digest =
            |hex: &str| Digest::new(format!("sha256:{}", hex.repeat(64))).ok_or("a valid digest");
        let package = digest("a")?;
        let packages = [ExecutionPackageBinding {
            skill: "notes".to_owned(),
            runner: "jot".to_owned(),
            package_digest: package.as_str().to_owned(),
            source_kind: ExecutionPackageSourceKind::SourceRoot,
            input_packet_schemas: Vec::new(),
            source_path: "skills/notes".to_owned(),
            source_files: vec!["SKILL.md".to_owned(), "run.yaml".to_owned()],
        }];
        let connectors = [ExecutionConnectorBinding {
            connector: ConnectorName::new("marker").ok_or("a valid name")?,
            manifest_digest: digest("b")?,
            connector_digest: digest("c")?,
        }];
        assert_eq!(
            execution_closure_digest(&packages, &connectors, &[]),
            "sha256:35d3a1537190dcd745b59333128a84b045510f7b112e69e3c6694e6c723597ad"
        );
        Ok(())
    }
}
