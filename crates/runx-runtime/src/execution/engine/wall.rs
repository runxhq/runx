//! What `effects: none` refuses before a run starts (vocabulary-and-loops.md,
//! "`effects: none` is how to constrain everything beneath a step"): what's
//! certain to change something beneath a step under it. That is a fixed step
//! that changes something, a script's own listed calls, and the same in the
//! closure of each method a step lists. A run that reaches one is refused as
//! it starts, so it runs no step at all.
//!
//! A change is an operation that writes, a core tool that changes the
//! workspace or runs a command, and a tool that runs a command. A read a
//! process carries out is its author's promise, so it passes only from a
//! connector that is verified or that a person trusted (D33).
//!
//! An agent's calls and loads, at any depth, are its choices: admission
//! decides each as it's made, and refuses any write or loaded process under
//! `none` (`floor.rs`). So is an append to `private`'s events, which keeps the
//! person's own words only when the call's stream is theirs (D22).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use runx_contracts::connector::OperationAccess;
use runx_contracts::{AuthorityVerb, Effects, ExecutionBoundaryKind, TrustTier};
use runx_parser::{ConnectorRequirement, StepKind, ToolTarget, Workflow, WorkflowStep};

use super::Runtime;
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::kernel::connectors::{ConnectorScope, LoadedConnector};
use crate::paths::RunxPaths;

/// The refusal a run of `workflow` from `workflow_dir` meets before it starts
/// for what a step under `effects: none` is certain to change, if any: the
/// first such step, with everything beneath it that would.
pub(crate) fn refusal<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    workflow: &Workflow,
) -> Result<Option<RuntimeError>, RuntimeError>
where
    A: SkillAdapter,
{
    let authority = runtime.options.run_authority.as_ref();
    let mut wall = Wall {
        runtime,
        scopes: BTreeMap::new(),
        vouched: BTreeMap::new(),
    };
    for step in &workflow.steps {
        if crate::kernel::authority::step_effects(authority, step) != Effects::None {
            continue;
        }
        let mut found = Vec::new();
        wall.step(workflow_dir, step, &format!("step {}", step.id), &mut found)?;
        for listed in crate::execution::workflow::child_skill_steps(step) {
            wall.listed(workflow_dir, &listed, &mut found)?;
        }
        if !found.is_empty() {
            return Ok(Some(refused(step, found)));
        }
    }
    Ok(None)
}

/// One change a step under `none` would make, and where.
#[derive(PartialEq, Eq)]
struct Change {
    what: String,
    at: String,
    verb: AuthorityVerb,
}

/// The run's refusal for `found`, beneath `step`.
fn refused(step: &WorkflowStep, found: Vec<Change>) -> RuntimeError {
    let mut unique = Vec::new();
    for change in found {
        if !unique.contains(&change) {
            unique.push(change);
        }
    }
    let found = unique;
    let verb = if found
        .iter()
        .any(|change| change.verb == AuthorityVerb::Write)
    {
        AuthorityVerb::Write
    } else {
        AuthorityVerb::Execute
    };
    let what = found
        .iter()
        .map(|change| format!("{}, in {}", change.what, change.at))
        .collect::<Vec<_>>()
        .join("; ");
    RuntimeError::AuthorityDenied {
        verb,
        step_id: step.id.clone(),
        reason: format!(
            "the run's effects are none, so it changes nothing: {what}{} refused before the run starts",
            if found.len() == 1 {
                ", is"
            } else {
                ", are each"
            }
        ),
    }
}

struct Wall<'a, A> {
    runtime: &'a Runtime<A>,
    /// Each package directory's connectors, read once: building a scope
    /// parses every connector manifest the project holds.
    scopes: BTreeMap<std::path::PathBuf, Arc<ConnectorScope>>,
    /// Whether each connector a process carries out a read for is verified
    /// or trusted, by its implementation's digest.
    vouched: BTreeMap<String, bool>,
}

impl<A> Wall<'_, A>
where
    A: SkillAdapter,
{
    fn env(&self) -> &BTreeMap<String, String> {
        &self.runtime.options.env
    }

    /// What every method in the closure of the method `listed` runs would
    /// change: each step, as its own step would.
    fn listed(
        &mut self,
        workflow_dir: &Path,
        listed: &WorkflowStep,
        found: &mut Vec<Change>,
    ) -> Result<(), RuntimeError> {
        let Some(target) = listed.skill() else {
            return Ok(());
        };
        // A method named only as the run goes is an agent's or a script's
        // choice, which its call's admission decides.
        if target.skill.starts_with('$') {
            return Ok(());
        }
        let package = crate::execution::workflow::load_step_skill_package(
            workflow_dir,
            listed,
            crate::execution::workflow::StepSkillLoadOptions::of_run(&self.runtime.options),
        )?
        .package;
        let manifest =
            package
                .manifest()
                .ok_or_else(|| RuntimeError::UnsupportedMethodSelection {
                    method: target
                        .method
                        .clone()
                        .unwrap_or_else(|| "default".to_owned()),
                })?;
        let method =
            crate::execution::workflow::select_step_method(manifest, target.method.as_deref())?
                .name
                .clone();
        let methods = crate::skill_package::reached_methods(package, &method, self.env()).map_err(
            |error| match error {
                crate::skill_package::SkillInspectionError::Runtime(error) => *error,
                other => RuntimeError::effect_state(
                    "walking a listed method's closure",
                    other.to_string(),
                ),
            },
        )?;
        for (package, method) in methods {
            let Some(definition) = package
                .manifest()
                .and_then(|manifest| manifest.methods.get(&method))
            else {
                continue;
            };
            for step in &definition.workflow.steps {
                let at = format!("{}#{method}'s step {}", package.package.skill.name, step.id);
                self.step(&package.directory, step, &at, found)?;
            }
        }
        Ok(())
    }

    /// What `step` itself would change: a fixed step's own act, or a
    /// script's listed tools and operations. A turn's calls are its agent's.
    fn step(
        &mut self,
        workflow_dir: &Path,
        step: &WorkflowStep,
        at: &str,
        found: &mut Vec<Change>,
    ) -> Result<(), RuntimeError> {
        match &step.kind {
            StepKind::Operation(target) => {
                let Some(declared) = target.declared.as_ref() else {
                    // Admission refuses a step its skill declares no
                    // connector for.
                    return Ok(());
                };
                let operations = match (&target.operation, target.from.as_slice()) {
                    (Some(operation), _) => vec![operation.as_str().to_owned()],
                    (None, from) => from.iter().map(|op| op.as_str().to_owned()).collect(),
                };
                let scope = self.scope(workflow_dir, target.loaded.is_some())?;
                self.operations(
                    (&scope, workflow_dir),
                    &target.requirement,
                    declared,
                    &operations,
                    at,
                    found,
                )?;
            }
            StepKind::Tool(ToolTarget::Catalog(tool)) => {
                self.tool(workflow_dir, tool, at, found)?
            }
            StepKind::Tool(ToolTarget::Process(process)) => found.push(Change {
                what: format!(
                    "running {}",
                    std::iter::once(process.command.as_str())
                        .chain(process.args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" ")
                ),
                at: at.to_owned(),
                verb: AuthorityVerb::Execute,
            }),
            StepKind::Script(script) => {
                for tool in &script.allowance.tools {
                    self.tool(workflow_dir, tool.as_str(), at, found)?;
                }
                if script.allowance.operations.is_empty() {
                    return Ok(());
                }
                let scope = self.scope(workflow_dir, false)?;
                for allowed in &script.allowance.operations {
                    let key = allowed.requirement.as_str();
                    // A call its step requires no connector for is refused
                    // as the run starts, with the script's declared ones.
                    let Some(declared) = script.connectors.get(key) else {
                        continue;
                    };
                    self.operations(
                        (&scope, workflow_dir),
                        key,
                        declared,
                        &[allowed.operation.as_str().to_owned()],
                        at,
                        found,
                    )?;
                }
            }
            StepKind::Agent(_)
            | StepKind::Tool(ToolTarget::Worker(_))
            | StepKind::Ask(_)
            | StepKind::Skill(_)
            | StepKind::Load(_) => {}
        }
        Ok(())
    }

    /// What a call of catalog tool `tool` would change: a core tool that
    /// changes something, or a tool runx runs as a process.
    fn tool(
        &self,
        workflow_dir: &Path,
        tool: &str,
        at: &str,
        found: &mut Vec<Change>,
    ) -> Result<(), RuntimeError> {
        let options = &self.runtime.options;
        let what = if crate::tool_catalogs::native::is_core_tool(tool) {
            if crate::tool_catalogs::native::only_reads(tool) {
                return Ok(());
            }
            match crate::tool_catalogs::native::execution_boundary(tool, &options.effects) {
                Some(ExecutionBoundaryKind::TrustedHostProcess) => {
                    format!("the {tool} tool, which runs a command")
                }
                _ => format!("the {tool} tool, which changes the workspace"),
            }
        } else {
            // A tool runx can't resolve runs nothing, and its call is
            // refused when it's made.
            let Ok(inspected) = crate::tool_catalogs::dispatch::inspect_catalog_tool(
                tool,
                &options.env,
                workflow_dir,
                &options.effects,
            ) else {
                return Ok(());
            };
            if inspected.execution != "process" {
                return Ok(());
            }
            format!("the {tool} tool, which runs a command")
        };
        found.push(Change {
            what,
            at: at.to_owned(),
            verb: AuthorityVerb::Execute,
        });
        Ok(())
    }

    /// What calls of `operations` on requirement `key`, as `declared`, would
    /// change, on every connector it may resolve to. None named means every
    /// one the requirement lists.
    fn operations(
        &mut self,
        (scope, workflow_dir): (&ConnectorScope, &Path),
        key: &str,
        declared: &ConnectorRequirement,
        operations: &[String],
        at: &str,
        found: &mut Vec<Change>,
    ) -> Result<(), RuntimeError> {
        let connectors = if declared.choose.is_some() {
            declared.from.clone()
        } else {
            vec![key.to_owned()]
        };
        let operations = if operations.is_empty() {
            declared
                .operations
                .iter()
                .map(|operation| operation.as_str().to_owned())
                .collect()
        } else {
            operations.to_vec()
        };
        for reference in connectors {
            // A connector that can't be resolved runs nothing, and admission
            // refuses its call with the reason.
            let Ok(connector) = scope.requirement(&reference, declared) else {
                continue;
            };
            for name in &operations {
                self.operation(workflow_dir, &connector, name, at, found)?;
            }
        }
        Ok(())
    }

    /// What a call of `name` on `connector` would change: a write, or a read
    /// a process carries out for a connector no one vouched for.
    fn operation(
        &mut self,
        workflow_dir: &Path,
        connector: &LoadedConnector,
        name: &str,
        at: &str,
        found: &mut Vec<Change>,
    ) -> Result<(), RuntimeError> {
        let Some((_, operation, _)) = connector.operation(name) else {
            return Ok(());
        };
        let change = match operation.access() {
            OperationAccess::Mutate => {
                if crate::kernel::admission::is_private(connector) && name == "events.append" {
                    return Ok(());
                }
                Change {
                    what: format!("the {}/{name} write", connector.name),
                    at: at.to_owned(),
                    verb: AuthorityVerb::Write,
                }
            }
            OperationAccess::Read => {
                if !crate::kernel::operation::run_by_a_process(&connector.manifest, operation)
                    || self.vouched_for(workflow_dir, connector)?
                {
                    return Ok(());
                }
                Change {
                    what: format!(
                        "the {}/{name} read, which a process carries out for a connector neither verified nor trusted",
                        connector.name
                    ),
                    at: at.to_owned(),
                    verb: AuthorityVerb::Execute,
                }
            }
        };
        found.push(change);
        Ok(())
    }

    /// Whether `connector` is verified, at the `verified` tier or above, or
    /// its implementation is one a person trusted. The project's own has no
    /// tier, and a home that fell back to the workspace holds no one's trust.
    fn vouched_for(
        &mut self,
        workflow_dir: &Path,
        connector: &LoadedConnector,
    ) -> Result<bool, RuntimeError> {
        let digest = connector.connector_digest.as_str().to_owned();
        if let Some(vouched) = self.vouched.get(&digest) {
            return Ok(*vouched);
        }
        let env = self.env();
        let paths = RunxPaths::resolve(env, workflow_dir);
        // A signature its identity refuses verifies nothing.
        let verified = crate::identity::connector_identity(connector, env, &paths)
            .ok()
            .is_some_and(|identity| {
                let stated = match &identity {
                    crate::identity::PackageIdentity::Signed(signed) => {
                        Some(crate::registry::tiers::stated_tier(signed, env, &paths))
                    }
                    _ => None,
                };
                identity
                    .tier(stated)
                    .is_some_and(|tier| tier.meets(TrustTier::Verified))
            });
        let vouched = verified
            || (!paths.home_fell_back()
                && crate::registry::trusted(&paths, &digest).map_err(|reason| {
                    RuntimeError::effect_state("reading a person's trust", reason)
                })?);
        self.vouched.insert(digest, vouched);
        Ok(vouched)
    }

    /// The connectors a step in `workflow_dir` reaches: the workspace's for
    /// one a load bound.
    fn scope(
        &mut self,
        workflow_dir: &Path,
        loaded: bool,
    ) -> Result<Arc<ConnectorScope>, RuntimeError> {
        let paths = RunxPaths::resolve(self.env(), workflow_dir);
        let directory = if loaded {
            paths.workspace().to_path_buf()
        } else {
            workflow_dir.to_path_buf()
        };
        if let Some(scope) = self.scopes.get(&directory) {
            return Ok(Arc::clone(scope));
        }
        let scope = Arc::new(
            ConnectorScope::in_project(&directory, &paths).map_err(|error| {
                RuntimeError::effect_state(
                    "reading the connectors a step reaches",
                    error.to_string(),
                )
            })?,
        );
        self.scopes.insert(directory, Arc::clone(&scope));
        Ok(scope)
    }
}
