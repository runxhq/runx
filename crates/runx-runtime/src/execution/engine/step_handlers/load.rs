//! A turn's load (agent-sessions §4.1). The package a call names resolves as
//! `runx skill` and `runx connector` resolve it; the turn's catalog rules
//! admit it by who it is, as binding sets it (D30), and its tier; and what it
//! binds must agree with the connectors the turn already reaches. A load adds
//! no capability by where a file sits (D12): the project's own is admitted
//! when it reaches only what the turn already reaches, and otherwise waits on
//! the person's trust, as a package below its rule's tier does. Trust is
//! recorded in the person's home for that exact closure, which covers every
//! connector's implementation. Its output carries a skill's manual and
//! methods, what it bound and the calls the turn offers from then on. A
//! skill's load binds the method it names or the skill's only method; one that
//! binds none binds no closure, so it asks no trust.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::connector::ConnectorBinding;
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ApprovalGate, AuthorityVerb, BoundClosure, CallTarget, CatalogRule, JsonObject, JsonValue,
    LoadedMethod, LoadedMethodInput, LoadedPackage, ProjectBinding, ResolutionRequest, SkillManual,
    TrustTier, TurnLoad, TurnTool,
};
use runx_core::state_machine::StepAdmissionWitness;
use runx_parser::{LoadStep, LoadTarget};

use super::{EphemeralValue, InvocationOutput, StepHandlerCtx, StepRun, seal_executed_step};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::approval::ApprovalResolution;
use crate::execution::output_projection::project_step_claim;
use crate::identity::PackageIdentity;
use crate::kernel::connectors::{ConnectorScope, LoadedConnector};
use crate::paths::RunxPaths;
use crate::receipts::StepSeal;
use crate::registry::resolve::{
    AbsentPins, Holder, ResolvedSkillRef, SkillResolverOptions, install_connector,
    resolve_registry_target, resolve_skill_ref_details,
};

/// What the rules make of a package.
#[derive(Debug, PartialEq, Eq)]
enum Ruling {
    Admitted,
    /// The person trusts it, or the load is refused: it is below every
    /// catalog rule's tier, or it is the project's own and reaches what the
    /// turn does not. `reason` is what the person is asked.
    AsksTrust {
        reason: String,
    },
    Refused(String),
}

/// A package found and bound.
struct Bound {
    package: LoadedPackage,
    /// Who it is, as binding set it.
    identity: PackageIdentity,
    /// A skill's manual and every method it declares.
    manual: Option<SkillManual>,
    methods: Vec<LoadedMethod>,
    tools: Vec<TurnTool>,
    /// The closures a person's trust covers.
    closures: Vec<String>,
    /// Each connector it reaches, at the exact revision it binds.
    connectors: Vec<ConnectorBinding>,
    /// Each skill package of the person's own it binds as itself.
    projects: Vec<ProjectBinding>,
    /// What the person is shown when it asks for their trust.
    declares: JsonValue,
}

pub(super) fn run_load_step<A>(
    request: StepHandlerCtx<'_, A>,
    load: &LoadStep,
) -> Result<StepRun, RuntimeError>
where
    A: SkillAdapter,
{
    let StepHandlerCtx {
        runtime,
        workflow_dir,
        workflow_name,
        step,
        attempt,
        host,
        ..
    } = request;
    let env = &runtime.options.env;
    let paths = RunxPaths::resolve(env, workflow_dir);
    let refused = |reason: String| RuntimeError::AuthorityDenied {
        verb: AuthorityVerb::Execute,
        step_id: step.id.clone(),
        reason,
    };
    let bound = match &load.target {
        LoadTarget::Skill { skill, method } => bind_skill(skill, method.as_deref(), &paths, env),
        LoadTarget::Connector { name } => bind_connector(name, &paths, env),
    }
    .map_err(refused)?;
    if let Some(reason) = revision_refusal(&bound, &load.held) {
        return Err(refused(reason));
    }
    if let Some(reason) = project_refusal(&bound, &load.projects) {
        return Err(refused(reason));
    }
    let tier = bound
        .identity
        .tier(stated_tier(&bound.identity, &load.rules, env, &paths));
    let mut bound = bound;
    report_tier(&mut bound.package, tier);
    let what = describe(&bound.package);
    match ruling(
        &bound.identity,
        tier,
        &beyond(&bound, &load.held),
        &load.rules,
        &what,
    ) {
        Ruling::Admitted => {}
        Ruling::Refused(reason) => return Err(refused(reason)),
        Ruling::AsksTrust { reason } => {
            let trusted = bound
                .closures
                .iter()
                .map(|closure| crate::registry::trusted(&paths, closure))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|why| {
                    refused(format!(
                        "{what} waits on the person's trust, which can't be read: {why}"
                    ))
                })?;
            if !trusted.iter().all(|trusted| *trusted) {
                ask_trust(runtime, workflow_dir, step, &bound, reason, &paths, host)?;
            }
        }
    }
    let output = TurnLoad {
        package: bound.package,
        manual: bound.manual,
        methods: bound.methods,
        tools: bound.tools,
        connectors: bound.connectors,
        projects: bound.projects,
    };
    let value = JsonValue::from_serialize(&output)
        .map_err(|error| RuntimeError::effect_state("reporting a load", error.to_string()))?;
    let JsonValue::Object(outputs) = value else {
        return Err(RuntimeError::effect_state(
            "reporting a load",
            "a load reports an object".to_owned(),
        ));
    };
    let output = InvocationOutput::runtime_success(JsonValue::Object(outputs.clone()), 0);
    let projection = project_step_claim(outputs, None);
    let receipt = seal_executed_step(
        StepSeal {
            scope: runtime.scope(workflow_name),
            step_id: &step.id,
            attempt,
            output: &output,
            claim: &projection.outputs,
            created_at: &runtime.options.clock.now(),
            authority: crate::receipts::SealAuthority::default(),
            call: None,
            admission_refs: Vec::new(),
            child_receipts: &[],
            descendant_receipts: &[],
            ending: None,
        },
        runtime.options.signature_policy(),
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill: "load".to_owned(),
        method: None,
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: Vec::new(),
        recorded_nested: Vec::new(),
        refusal: None,
        failure: None,
        admission_witness,
        settlement: None,
        labels: runx_contracts::ArtifactLabels::CONFIGURED_MODEL,
        input_digest: None,
        credential_fingerprint: None,
    })
}

/// The skill `reference` names, as a load resolves it and as every call of a
/// method it bound resolves it again: a project's own skill by its path in
/// the workspace, any other by its name and version, wherever runx holds it.
pub(crate) fn resolve_loaded(
    reference: &str,
    paths: &RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<ResolvedSkillRef, String> {
    resolve_skill_ref_details(
        Path::new(reference),
        paths.workspace(),
        SkillResolverOptions {
            env,
            registry: None,
            expected_digest: None,
            absent_pins: AbsentPins::Fail,
            holder: Holder::Project,
        },
    )
}

/// The skill `reference` names, with `method` bound, or the skill's only
/// method when it names none. A skill of several methods loaded without one
/// binds none: the load reads its manual and methods, and binds no closure.
fn bind_skill(
    reference: &str,
    method: Option<&str>,
    paths: &RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<Bound, String> {
    let resolved = resolve_loaded(reference, paths, env)?;
    let directory = canonical(&resolved.runnable_path)?;
    let loaded = Arc::new(
        crate::load_validated_skill_package(&directory).map_err(|error| error.to_string())?,
    );
    let manifest = loaded
        .manifest()
        .ok_or_else(|| format!("{reference} has no run.yaml method to load"))?;
    let methods = match method {
        Some(method) if manifest.methods.contains_key(method) => vec![method.to_owned()],
        Some(method) => return Err(format!("{reference} has no method {method}")),
        None if manifest.methods.len() == 1 => manifest.methods.keys().cloned().collect(),
        None => Vec::new(),
    };
    let package_digest = loaded.package.package_digest.clone();
    let description = loaded.package.skill.description.clone();
    // The package's name, which a bound method's call names it by and
    // resolves again.
    let package_name = match (&resolved.skill_id, &resolved.version) {
        (Some(skill_id), Some(version)) => format!("{skill_id}@{version}"),
        _ => directory_name(&directory, paths)?,
    };
    let skill: NonEmptyString = package_name.clone().into();
    let mut tools = Vec::new();
    let mut closures = Vec::new();
    let mut connectors = Vec::new();
    let mut identity = None;
    let mut projects = BTreeMap::new();
    for method in &methods {
        let binding = crate::skill_package::verify_loaded_execution_binding(
            Arc::clone(&loaded),
            method,
            env,
            None,
            None,
        )
        .map_err(|error| format!("{reference}#{method}: {error}"))?;
        if identity.is_none() {
            identity = binding.packages.identity(&loaded.package_root).cloned();
        }
        for (name, root) in binding.packages.projects() {
            agree_project(&mut projects, name, root, reference)?;
        }
        let closure_digest = binding
            .closure_digest
            .ok_or_else(|| format!("{reference}#{method}: its closure is not fully bound"))?;
        for connector in crate::skill_package::case_closure_connectors(
            &loaded,
            crate::skill_package::CaseClosureTarget::Method(method),
            env,
        )
        .map_err(|error| format!("{reference}#{method}: {error}"))?
        {
            agree(&mut connectors, &connector, reference)?;
        }
        tools.push(
            super::super::turn::skill_descriptor(
                &super::super::turn::loaded_tool_name(&package_name, method),
                description.clone(),
                &manifest.methods[method],
                CallTarget::Skill {
                    skill: skill.clone(),
                    method: Some(method.as_str().into()),
                    bound: Some(BoundClosure {
                        package_digest: package_digest.as_str().into(),
                        closure_digest: closure_digest.as_str().into(),
                    }),
                },
            )
            .map_err(|error| error.to_string())?,
        );
        closures.push(closure_digest);
    }
    let reached = connectors
        .iter()
        .map(|binding| binding.connector.as_str())
        .collect::<Vec<_>>();
    let declares = JsonValue::from_serialize(&serde_json::json!({
        "methods": methods,
        "connectors": reached,
    }))
    .map_err(|error| error.to_string())?;
    // A load that binds no method binds no closure, so it sets who the
    // package is from the package alone.
    let identity = match identity {
        Some(identity) => identity,
        None => crate::identity::skill_identity(&loaded, env)?,
    };
    if let PackageIdentity::Project { name } = &identity {
        agree_project(&mut projects, name, &loaded.package_root, reference)?;
    }
    let projects = projects
        .iter()
        .map(|(name, root)| crate::identity::project_binding(name, root, paths))
        .collect();
    Ok(Bound {
        package: LoadedPackage::Skill {
            skill: package_name.into(),
            trust_tier: None,
            publisher: identity.publisher().and_then(NonEmptyString::new),
            package_digest: package_digest.into(),
        },
        identity,
        manual: Some(SkillManual {
            digest: loaded.package.manual_digest.as_str().into(),
            content: loaded.package.manual_markdown.clone(),
        }),
        methods: manifest
            .methods
            .values()
            .map(|declared| LoadedMethod {
                name: declared.name.as_str().into(),
                purpose: declared
                    .description
                    .as_deref()
                    .and_then(NonEmptyString::new),
                inputs: declared
                    .inputs
                    .iter()
                    .map(|(name, input)| LoadedMethodInput {
                        name: name.as_str().into(),
                        input_type: input.input_type.as_str().into(),
                        required: input.required,
                    })
                    .collect(),
                bound: methods.contains(&declared.name),
            })
            .collect(),
        tools,
        closures,
        connectors,
        projects,
        declares,
    })
}

/// One closure a person trusted ahead of any turn that loads it (`runx
/// trust`): the package and the method it is the closure of, the connectors
/// it binds, which a turn reaches only once it loads them, and the package's
/// tier.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct StatedTrust {
    pub package: String,
    pub method: String,
    pub closure_digest: String,
    pub connectors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<TrustTier>,
}

/// The person trusts the skill `reference`'s `method`, or each method it
/// declares when they name none, ahead of any turn that loads it. It binds
/// as a load binds, so its closures are the ones a load of it asks trust
/// for, and records each in the person's home as their approval of a load's
/// trust records it, read by the same check. Running it is the person's
/// statement, so a process a run started, which carries the workspace
/// lock's token, is refused: an agent working in a run can't state it.
///
/// # Errors
///
/// Returns why it is refused, why the package doesn't bind, or why the
/// trust can't be recorded.
pub fn state_trust(
    reference: &str,
    method: Option<&str>,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<Vec<StatedTrust>, String> {
    if env.contains_key(crate::workspace_lock::RUNX_WORKSPACE_LOCK_TOKEN_ENV) {
        return Err(
            "runx trust is the person's statement, and this runs inside a runx run: run it yourself, outside any run"
                .to_owned(),
        );
    }
    let paths = RunxPaths::resolve(env, cwd);
    let first = bind_skill(reference, method, &paths, env)?;
    // A load of a skill of several methods without one binds none, so the
    // person trusts each method's closure.
    let bound = if first.closures.is_empty() {
        first
            .methods
            .iter()
            .map(|declared| bind_skill(reference, Some(declared.name.as_str()), &paths, env))
            .collect::<Result<Vec<_>, _>>()?
    } else {
        vec![first]
    };
    let mut stated = Vec::new();
    for bound in bound {
        let tier = bound.identity.tier(match &bound.identity {
            PackageIdentity::Signed(signed) => {
                Some(crate::registry::tiers::stated_tier(signed, env, &paths))
            }
            PackageIdentity::Bundled { .. } | PackageIdentity::Project { .. } => None,
        });
        let LoadedPackage::Skill { skill, .. } = &bound.package else {
            return Err(format!("{reference} is no skill"));
        };
        let method = bound
            .methods
            .iter()
            .find(|declared| declared.bound)
            .map(|declared| declared.name.as_str().to_owned())
            .ok_or_else(|| format!("{reference} bound no method"))?;
        let connectors = bound
            .connectors
            .iter()
            .map(|binding| binding.connector.as_str().to_owned())
            .collect::<Vec<_>>();
        for closure in &bound.closures {
            crate::registry::record_trust(
                &paths,
                &crate::registry::TrustRecord {
                    closure_digest: closure.clone(),
                    package: describe(&bound.package),
                    tier,
                },
            )?;
            stated.push(StatedTrust {
                package: skill.as_str().to_owned(),
                method: method.clone(),
                closure_digest: closure.clone(),
                connectors: connectors.clone(),
                tier,
            });
        }
    }
    Ok(stated)
}

/// The connector `name` (or `name@version`) names: one the project holds,
/// else the version the hosted registry publishes, added to the project.
fn bind_connector(
    name: &str,
    paths: &RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<Bound, String> {
    let scope =
        || ConnectorScope::in_project(paths.workspace(), paths).map_err(|error| error.to_string());
    let connector = match scope()?.named(name) {
        Ok(connector) => connector,
        Err(local) => {
            let (reference, version) = name
                .split_once('@')
                .map_or((name, None), |(reference, version)| {
                    (reference, Some(version))
                });
            let connector_name = runx_contracts::connector::ConnectorName::new(reference)
                .ok_or_else(|| format!("{name} is not a connector name"))?;
            let target = resolve_registry_target(None, env)
                .map_err(|error| format!("{local}; {}", error.into_message()))?;
            install_connector(
                &connector_name,
                version,
                None,
                &target.registry_url,
                env,
                paths.workspace(),
            )
            .map_err(crate::registry::resolve::ResolveError::into_message)?
            .ok_or_else(|| format!("{local}; and no registry publishes connector {name}"))?;
            scope()?.named(name).map_err(|error| error.to_string())?
        }
    };
    let identity = crate::identity::connector_identity(&connector, env, paths)?;
    let mut connectors = Vec::new();
    agree(&mut connectors, &connector, name)?;
    // Each operation as a call on the connector, pinned to the manifest the
    // load bound.
    let declared = runx_parser::ConnectorRequirement {
        operations: connector.manifest.operations.keys().cloned().collect(),
        manifest_digest: Some(connector.manifest_digest.clone()),
        choose: None,
        from: Vec::new(),
    };
    let id = connector.name.as_str().to_owned();
    let tools = connector
        .manifest
        .operations
        .iter()
        .map(|(operation, declaration)| {
            super::super::turn::operation_descriptor(
                &runx_contracts::AllowedOperation {
                    requirement: id.as_str().into(),
                    operation: operation.as_str().into(),
                },
                &declared,
                std::slice::from_ref(&id),
                &[(id.clone(), declaration.clone())],
            )
            .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let declares = JsonValue::from_serialize(&serde_json::json!({
        "operations": connector
            .manifest
            .operations
            .iter()
            .map(|(operation, declared)| serde_json::json!({
                "operation": operation.as_str(),
                "access": declared.access().as_str(),
                "description": declared.description,
            }))
            .collect::<Vec<_>>(),
    }))
    .map_err(|error| error.to_string())?;
    Ok(Bound {
        package: LoadedPackage::Connector {
            connector: connector.name.as_str().into(),
            trust_tier: None,
            publisher: identity.publisher().and_then(NonEmptyString::new),
            manifest_digest: connector.manifest_digest.as_str().into(),
        },
        identity,
        manual: None,
        methods: Vec::new(),
        tools,
        closures: vec![connector.connector_digest.as_str().to_owned()],
        connectors,
        // A connector is never a `project:<name>` skill package.
        projects: Vec::new(),
        declares,
    })
}

/// What the rules make of the package `identity` names, at `tier` (a
/// project package has none), that reaches `beyond`, the connectors it binds
/// that the turn does not already reach.
fn ruling(
    identity: &PackageIdentity,
    tier: Option<TrustTier>,
    beyond: &[&ConnectorBinding],
    rules: &[CatalogRule],
    what: &str,
) -> Ruling {
    let project = identity.is_project();
    match tier.filter(|_| !project) {
        None if !rules.iter().any(|rule| rule.project) => Ruling::Refused(format!(
            "{what} is the project's own, and the turn's rules admit only the catalog"
        )),
        // Where a file sits adds no capability (D12): the project's own that
        // reaches only what the turn already reaches adds none.
        None if beyond.is_empty() => Ruling::Admitted,
        None => Ruling::AsksTrust {
            reason: format!(
                "Trust {what}, the project's own, which reaches connector {} the turn does not already reach.",
                beyond
                    .iter()
                    .map(|binding| binding.connector.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        },
        Some(tier) => {
            let minimums = rules
                .iter()
                .filter_map(|rule| rule.catalog.map(|catalog| catalog.trust))
                .collect::<Vec<_>>();
            if minimums.is_empty() {
                return Ruling::Refused(format!(
                    "{what} is a registry's, and the turn's rules admit only the project's own"
                ));
            }
            if minimums.iter().any(|minimum| tier.meets(*minimum)) {
                return Ruling::Admitted;
            }
            let minimum = minimums
                .into_iter()
                .reduce(|lowest, minimum| {
                    if lowest.meets(minimum) {
                        minimum
                    } else {
                        lowest
                    }
                })
                .unwrap_or(TrustTier::Verified);
            Ruling::AsksTrust {
                reason: format!(
                    "Trust {what}, a {} package, below the turn's {} rule.",
                    tier.as_str(),
                    minimum.as_str()
                ),
            }
        }
    }
}

/// Why `bound` would make two directories one `project:<name>` in the run:
/// each skill package of the person's own is one directory for the whole
/// run, since the two would own the same streams (D30).
fn project_refusal(bound: &Bound, held: &[ProjectBinding]) -> Option<String> {
    bound.projects.iter().find_map(|binding| {
        held.iter()
            .find(|held| held.name == binding.name && held.directory != binding.directory)
            .map(|held| {
                format!(
                    "{} binds {} as project:{}, and this run already binds {} as project:{}; two directories are never one package in a run",
                    describe(&bound.package),
                    binding.directory,
                    binding.name,
                    held.directory,
                    held.name
                )
            })
    })
}

/// Record that `reference` binds `root` as `project:<name>`, refusing a
/// second directory under that name.
fn agree_project(
    projects: &mut BTreeMap<String, PathBuf>,
    name: &str,
    root: &Path,
    reference: &str,
) -> Result<(), String> {
    match projects.get(name) {
        Some(other) if other != root => Err(format!(
            "{reference} binds two directories as project:{name}, {} and {}",
            other.display(),
            root.display()
        )),
        Some(_) => Ok(()),
        None => {
            projects.insert(name.to_owned(), root.to_path_buf());
            Ok(())
        }
    }
}

/// Why what `bound` reaches disagrees with what the turn already binds.
fn revision_refusal(bound: &Bound, held: &[ConnectorBinding]) -> Option<String> {
    bound.connectors.iter().find_map(|binding| {
        held.iter()
            .find(|held| {
                held.connector == binding.connector
                    && held.manifest_digest != binding.manifest_digest
            })
            .map(|held| {
                format!(
                    "{} binds connector {} at manifest {}, and this run already binds it at {}; one run binds one revision of a name",
                    describe(&bound.package),
                    binding.connector,
                    binding.manifest_digest,
                    held.manifest_digest
                )
            })
    })
}

/// The connectors `bound` reaches that the turn does not already reach at
/// that exact implementation: a changed process under an unchanged manifest
/// is one it doesn't.
fn beyond<'a>(bound: &'a Bound, held: &[ConnectorBinding]) -> Vec<&'a ConnectorBinding> {
    bound
        .connectors
        .iter()
        .filter(|binding| !held.contains(binding))
        .collect()
}

/// Ask the person to trust `bound`, telling them `reason`; record their
/// trust for its closures in their home.
fn ask_trust<A>(
    runtime: &super::Runtime<A>,
    workflow_dir: &Path,
    step: &runx_parser::WorkflowStep,
    bound: &Bound,
    reason: String,
    paths: &RunxPaths,
    host: &mut dyn crate::host::Host,
) -> Result<(), RuntimeError> {
    let options = &runtime.options;
    let what = describe(&bound.package);
    let gate_id = format!("{}/trust", step.id);
    let request_id = options.run.address(&gate_id);
    let mut gate = ApprovalGate {
        id: gate_id.into(),
        reason: reason.into(),
        gate_type: Some("trust".to_owned()),
        summary: Some(JsonObject::from([(
            "step".to_owned(),
            JsonValue::String(step.id.clone()),
        )])),
        payload: None,
        payload_artifact: None,
    };
    let payload = JsonValue::from_serialize(&serde_json::json!({
        "package": bound.package,
        "closures": bound.closures,
        "declares": bound.declares,
    }))
    .map_err(|error| RuntimeError::effect_state("describing a load", error.to_string()))?;
    crate::approval::carry_payload(
        &mut gate,
        payload,
        &RunxPaths::resolve(&options.env, workflow_dir).artifacts(),
        options.run.run_id(),
    )
    .map_err(|message| RuntimeError::effect_state("carrying a load", message))?;
    let resolution = crate::approval::request_approval(host, request_id.clone(), gate.clone())
        .map_err(|error| RuntimeError::effect_state("asking for trust", error.to_string()))?;
    match resolution {
        ApprovalResolution::Approved { .. } => {
            let tier = reported_tier(&bound.package);
            for closure in &bound.closures {
                crate::registry::record_trust(
                    paths,
                    &crate::registry::TrustRecord {
                        closure_digest: closure.clone(),
                        package: what.clone(),
                        tier,
                    },
                )
                .map_err(|why| RuntimeError::AuthorityDenied {
                    verb: AuthorityVerb::Execute,
                    step_id: step.id.clone(),
                    reason: format!(
                        "the person trusted {what}, and their trust can't be recorded: {why}"
                    ),
                })?;
            }
            Ok(())
        }
        ApprovalResolution::Declined { reason, .. } => Err(RuntimeError::Declined {
            step_id: step.id.clone(),
            reason: reason.unwrap_or_else(|| format!("the person did not trust {what}")),
        }),
        ApprovalResolution::Pending { .. } => Err(RuntimeError::ResolutionPending {
            step_id: step.id.clone(),
            reason: format!("{} waits on the person's trust", step.id),
            request: Box::new(ResolutionRequest::Approval {
                id: request_id.into(),
                gate: Box::new(gate),
            }),
            child: None,
        }),
    }
}

/// Record `connector` among what a load binds, refusing a second revision.
fn agree(
    connectors: &mut Vec<ConnectorBinding>,
    connector: &LoadedConnector,
    what: &str,
) -> Result<(), String> {
    let binding = connector.binding();
    if let Some(other) = connectors.iter().find(|other| {
        other.connector == binding.connector && other.manifest_digest != binding.manifest_digest
    }) {
        return Err(format!(
            "{what} reaches two revisions of connector {} (manifest {} and {})",
            connector.name, other.manifest_digest, binding.manifest_digest
        ));
    }
    if !connectors.contains(&binding) {
        connectors.push(binding);
    }
    Ok(())
}

/// A signed package's tier, as its fresh statement gives it, read only when a
/// rule asks for more than `community` (D30); otherwise none is read, and the
/// package reads `community`.
fn stated_tier(
    identity: &PackageIdentity,
    rules: &[CatalogRule],
    env: &BTreeMap<String, String>,
    paths: &RunxPaths,
) -> Option<TrustTier> {
    let PackageIdentity::Signed(signed) = identity else {
        return None;
    };
    rules
        .iter()
        .any(|rule| {
            rule.catalog
                .is_some_and(|catalog| catalog.trust != TrustTier::Community)
        })
        .then(|| crate::registry::tiers::stated_tier(signed, env, paths))
}

/// Report the tier the load's decision read.
fn report_tier(package: &mut LoadedPackage, tier: Option<TrustTier>) {
    match package {
        LoadedPackage::Skill { trust_tier, .. } | LoadedPackage::Connector { trust_tier, .. } => {
            *trust_tier = tier;
        }
    }
}

/// The tier the load reported, which a person's trust is recorded at.
fn reported_tier(package: &LoadedPackage) -> Option<TrustTier> {
    match package {
        LoadedPackage::Skill { trust_tier, .. } | LoadedPackage::Connector { trust_tier, .. } => {
            *trust_tier
        }
    }
}

fn describe(package: &LoadedPackage) -> String {
    match package {
        LoadedPackage::Skill { skill, .. } => format!("skill {skill}"),
        LoadedPackage::Connector { connector, .. } => format!("connector {connector}"),
    }
}

/// A project package's name: its path in the workspace.
fn directory_name(directory: &Path, paths: &RunxPaths) -> Result<String, String> {
    directory
        .strip_prefix(canonical(paths.workspace())?)
        .map(|relative| relative.to_string_lossy().into_owned())
        .map_err(|_| format!("{} lies outside the workspace", directory.display()))
}

fn canonical(path: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use runx_contracts::CatalogTrust;
    use runx_contracts::connector::{ConnectorName, Digest};

    use super::*;

    fn catalog(trust: TrustTier) -> CatalogRule {
        CatalogRule {
            catalog: Some(CatalogTrust { trust }),
            project: false,
        }
    }

    const PROJECT: CatalogRule = CatalogRule {
        catalog: None,
        project: true,
    };

    fn signed() -> PackageIdentity {
        PackageIdentity::Signed(crate::registry::SignedPackage {
            kind: runx_contracts::RegistrySubjectKind::Skill,
            name: "acme/echo".to_owned(),
            version: "1.0.0".to_owned(),
            content_digest: format!("sha256:{}", "a".repeat(64)),
            key_id: "operator-key".to_owned(),
        })
    }

    fn project() -> PackageIdentity {
        PackageIdentity::Project {
            name: "x".to_owned(),
        }
    }

    /// `name` at manifest `manifest` and implementation `implementation`,
    /// each digest the hex character given, repeated.
    fn binding(
        name: &str,
        manifest: char,
        implementation: char,
    ) -> Result<ConnectorBinding, &'static str> {
        let digest = |hex: char| {
            Digest::new(format!("sha256:{}", hex.to_string().repeat(64))).ok_or("a digest")
        };
        Ok(ConnectorBinding {
            connector: ConnectorName::new(name).ok_or("a connector name")?,
            manifest_digest: digest(manifest)?,
            connector_digest: digest(implementation)?,
        })
    }

    fn reaching(connectors: Vec<ConnectorBinding>) -> Bound {
        Bound {
            package: LoadedPackage::Connector {
                connector: "github".into(),
                trust_tier: None,
                publisher: None,
                manifest_digest: "sha256:b".into(),
            },
            identity: project(),
            projects: Vec::new(),
            manual: None,
            methods: Vec::new(),
            tools: Vec::new(),
            closures: Vec::new(),
            connectors,
            declares: JsonValue::Null,
        }
    }

    #[test]
    fn a_rule_admits_by_identity_and_tier() {
        let rules = [catalog(TrustTier::Verified), PROJECT];
        assert_eq!(
            ruling(&project(), None, &[], &rules, "it"),
            Ruling::Admitted
        );
        assert_eq!(
            ruling(&signed(), Some(TrustTier::FirstParty), &[], &rules, "it"),
            Ruling::Admitted
        );
        assert_eq!(
            ruling(&signed(), Some(TrustTier::Verified), &[], &rules, "it"),
            Ruling::Admitted
        );
        assert_eq!(
            ruling(&signed(), Some(TrustTier::Community), &[], &rules, "it"),
            Ruling::AsksTrust {
                reason: "Trust it, a community package, below the turn's verified rule.".to_owned()
            }
        );
    }

    /// Where a file sits adds no capability (D12): the project's own that
    /// reaches a connector the turn does not, at that exact implementation,
    /// waits on the person's trust. A registry package's tier decides alone.
    #[test]
    fn the_projects_own_reaching_beyond_the_turn_asks_trust() -> Result<(), &'static str> {
        let held = [binding("marker", 'a', 'b')?];
        let same = reaching(vec![binding("marker", 'a', 'b')?]);
        assert!(beyond(&same, &held).is_empty());
        // A changed process under an unchanged manifest is another
        // implementation, which the turn does not reach.
        let changed = reaching(vec![binding("marker", 'a', 'c')?]);
        let reached = beyond(&changed, &held);
        assert_eq!(reached, [&binding("marker", 'a', 'c')?]);
        assert_eq!(revision_refusal(&changed, &held), None);

        let rules = [PROJECT];
        assert_eq!(
            ruling(&project(), None, &[], &rules, "skill x"),
            Ruling::Admitted
        );
        assert_eq!(
            ruling(&project(), None, &reached, &rules, "skill x"),
            Ruling::AsksTrust {
                reason: "Trust skill x, the project's own, which reaches connector marker the turn does not already reach.".to_owned()
            }
        );
        assert_eq!(
            ruling(
                &signed(),
                Some(TrustTier::Verified),
                &reached,
                &[catalog(TrustTier::Verified)],
                "skill x"
            ),
            Ruling::Admitted
        );
        Ok(())
    }

    /// A connector's name doesn't say who published it, so its tier
    /// statement names the publisher, signed (D30). A connector runx
    /// publishes, stated first_party by the official key, loads under a
    /// verified rule with nothing asked; the official key stating first_party
    /// for another publisher's connector states nothing, so that connector
    /// reads community and waits on the person's trust. The official keys are
    /// built into the CLI, so the test key stands as one here.
    #[test]
    fn a_connector_runx_publishes_loads_under_a_verified_rule_and_another_publishers_waits()
    -> Result<(), String> {
        use crate::registry::RegistryKeyScope;
        use crate::registry::signature::testing as signing;
        use runx_contracts::{RegistrySubject, RegistrySubjectKind};

        let text = |error: std::io::Error| error.to_string();
        let workspace = tempfile::tempdir().map_err(text)?;
        let home = tempfile::tempdir().map_err(text)?;
        // No statement is ever read again from a registry here: the one this
        // runx is pointed at is loopback, and not the official one.
        let env = BTreeMap::from([
            (
                crate::paths::RUNX_CWD_ENV.to_owned(),
                workspace.path().display().to_string(),
            ),
            ("HOME".to_owned(), home.path().display().to_string()),
            (
                "RUNX_REGISTRY_URL".to_owned(),
                "http://127.0.0.1:9".to_owned(),
            ),
        ]);
        let paths = RunxPaths::resolve(&env, workspace.path());
        let keys = vec![signing::key(RegistryKeyScope::Official)?];
        let decide = |name: &str, publisher: &str| -> Result<Ruling, String> {
            let directory = workspace
                .path()
                .join(".runx/added-connectors")
                .join(name)
                .join("1.0.0");
            std::fs::create_dir_all(&directory).map_err(text)?;
            std::fs::write(
                directory.join("connector.yaml"),
                format!(
                    "schema: runx.connector.v1\nname: {name}\ntitle: Records\ndescription: Reads records.\nauth: {{none: {{}}}}\nplacements: {{local: {{auth: [none]}}}}\noperations:\n  record.read:\n    description: Read a record.\n    verb: read\n    scopes: [record.read]\n    target: {{template: \"ops://records\"}}\n"
                ),
            )
            .map_err(text)?;
            let manifest =
                signing::sign(&directory, RegistrySubjectKind::Connector, name, "1.0.0")?;
            let subject = RegistrySubject {
                kind: RegistrySubjectKind::Connector,
                name: name.to_owned(),
            };
            let statement = crate::registry::tiers::testing::statement(
                subject.clone(),
                "1.0.0",
                &manifest.content_digest,
                publisher,
                TrustTier::FirstParty,
                ("2026-11-01T00:00:00Z", "2099-01-01T00:00:00Z"),
            )?;
            // Served with the version from the official registry, as install
            // reads it, and kept in the home when it holds.
            crate::registry::tiers::accept_served(
                Some(&statement),
                crate::registry::tiers::StatedPackage {
                    subject: &subject,
                    version: "1.0.0",
                    content_digest: &manifest.content_digest,
                },
                &keys,
                &crate::registry::RegistryManifestSourceAuthority::OfficialRunx,
                crate::registry::tiers::tier_cache(&paths).as_deref(),
            );
            let connector =
                crate::kernel::connectors::load_installed(&directory).map_err(|e| e.to_string())?;
            let identity =
                crate::identity::connector_identity_under(&connector, || Ok(keys.clone()))?;
            let PackageIdentity::Signed(signed) = &identity else {
                return Err(format!("{name} binds by its signature: {identity:?}"));
            };
            let tier = identity.tier(Some(crate::registry::tiers::stated_tier_under(
                signed, &keys, &env, &paths,
            )));
            Ok(ruling(
                &identity,
                tier,
                &[],
                &[catalog(TrustTier::Verified)],
                &format!("connector {name}"),
            ))
        };
        assert_eq!(decide("github", "runx")?, Ruling::Admitted);
        assert_eq!(
            decide("stripe", "acme")?,
            Ruling::AsksTrust {
                reason:
                    "Trust connector stripe, a community package, below the turn's verified rule."
                        .to_owned()
            }
        );
        Ok(())
    }

    #[test]
    fn a_load_binding_another_revision_of_a_held_connector_is_refused() -> Result<(), &'static str>
    {
        let bound = reaching(vec![binding("github", 'b', 'c')?]);
        let reason = revision_refusal(&bound, &[binding("github", 'a', 'c')?]).unwrap_or_default();
        assert!(
            reason.contains("one run binds one revision of a name"),
            "{reason}"
        );
        assert_eq!(
            revision_refusal(&bound, &[binding("github", 'b', 'd')?]),
            None
        );
        assert_eq!(revision_refusal(&bound, &[]), None);
        Ok(())
    }

    #[test]
    fn a_rule_of_one_kind_refuses_the_other() {
        let refused =
            |identity: &PackageIdentity, tier: Option<TrustTier>, rule: CatalogRule| match ruling(
                identity,
                tier,
                &[],
                &[rule],
                "skill x",
            ) {
                Ruling::Refused(reason) => reason,
                other => format!("not refused: {other:?}"),
            };
        let reason = refused(&project(), None, catalog(TrustTier::Community));
        assert!(reason.contains("admit only the catalog"), "{reason}");
        let reason = refused(&signed(), Some(TrustTier::FirstParty), PROJECT);
        assert!(reason.contains("admit only the project's own"), "{reason}");
    }
}
