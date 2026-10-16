//! A turn's load (agent-sessions §4.1). The package a call names resolves as
//! `runx skill` and `runx connector` resolve it; the turn's catalog rules
//! admit it by where it comes from and its tier; what it binds must agree with
//! the connectors the turn already reaches; and a package below its rule's
//! tier waits on the person's trust, which the project records for that exact
//! closure. Its output names what it bound and the calls the turn offers from
//! then on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{
    ApprovalGate, AuthorityVerb, BoundClosure, CatalogRule, JsonObject, JsonValue, LoadedPackage,
    ResolutionRequest, TrustTier, TurnLoad, TurnTarget, TurnTool,
};
use runx_core::state_machine::StepAdmissionWitness;
use runx_parser::{LoadStep, LoadTarget};

use super::{EphemeralValue, InvocationOutput, StepHandlerCtx, StepRun, seal_executed_step};
use crate::RuntimeError;
use crate::adapter::SkillAdapter;
use crate::approval::ApprovalResolution;
use crate::execution::output_projection::project_step_claim;
use crate::kernel::connectors::{ConnectorScope, LoadedConnector};
use crate::paths::RunxPaths;
use crate::receipts::StepSeal;
use crate::registry::resolve::{
    AbsentPins, RegistryTarget, SkillRefKind, SkillResolverOptions, install_connector,
    resolve_registry_target, resolve_skill_ref_details,
};

/// Where a package comes from, as a rule reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Origin {
    /// The project authored it.
    Project,
    /// A registry published it, at a tier, under a publisher.
    Registry {
        tier: TrustTier,
        publisher: Option<String>,
    },
}

/// What the rules make of a package.
#[derive(Debug, PartialEq, Eq)]
enum Ruling {
    Admitted,
    /// It is below every catalog rule's tier, the lowest of which is
    /// `minimum`: the person trusts it, or the load is refused.
    AsksTrust {
        minimum: TrustTier,
    },
    Refused(String),
}

/// A package found and bound.
struct Bound {
    package: LoadedPackage,
    origin: Origin,
    tools: Vec<TurnTool>,
    /// The execution closures a person's trust covers.
    closures: Vec<String>,
    connectors: BTreeMap<String, String>,
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
        graph_dir,
        graph_name,
        step,
        attempt,
        host,
        ..
    } = request;
    let env = &runtime.options.env;
    let paths = RunxPaths::resolve(env, graph_dir);
    let refused = |reason: String| RuntimeError::AuthorityDenied {
        verb: AuthorityVerb::Execute,
        step_id: step.id.clone(),
        reason,
    };
    let bound = match &load.target {
        LoadTarget::Skill { reference, runner } => {
            bind_skill(reference, runner.as_deref(), &paths, env)
        }
        LoadTarget::Connector { name } => bind_connector(name, &paths, env),
    }
    .map_err(refused)?;
    if let Some(reason) = revision_refusal(&bound, &load.held) {
        return Err(refused(reason));
    }
    match ruling(&bound.origin, &load.rules, &describe(&bound.package)) {
        Ruling::Admitted => {}
        Ruling::Refused(reason) => return Err(refused(reason)),
        Ruling::AsksTrust { minimum } => {
            let trusted = bound
                .closures
                .iter()
                .map(|closure| crate::registry::trusted(&paths, closure))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|message| RuntimeError::effect_state("reading trust", message))?;
            if !trusted.iter().all(|trusted| *trusted) {
                ask_trust(runtime, graph_dir, step, &bound, minimum, &paths, host)?;
            }
        }
    }
    let output = TurnLoad {
        package: bound.package,
        tools: bound.tools,
        connectors: bound
            .connectors
            .into_iter()
            .map(|(name, digest)| (name, digest.into()))
            .collect(),
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
            scope: runtime.scope(graph_name),
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
            closure: None,
        },
        runtime.options.signature_policy(),
    )?;
    let admission_witness = StepAdmissionWitness::local_runtime(&step.id, receipt.id.as_str());
    Ok(StepRun {
        step_id: step.id.clone(),
        attempt,
        skill: "load".to_owned(),
        runner: None,
        contract: projection.outputs,
        artifact_outputs: projection.artifact_outputs,
        ephemeral_contract: EphemeralValue::default(),
        outcome: output.into(),
        receipt,
        nested_receipts: Vec::new(),
        refusal: None,
        admission_witness,
        settlement: None,
    })
}

/// The skill `reference` names, with `runner` or every runner bound.
fn bind_skill(
    reference: &str,
    runner: Option<&str>,
    paths: &RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<Bound, String> {
    let resolved = resolve_skill_ref_details(
        Path::new(reference),
        paths.workspace(),
        SkillResolverOptions {
            env,
            registry: None,
            expected_digest: None,
            absent_pins: AbsentPins::Fail,
        },
    )?;
    let directory = canonical(&resolved.runnable_path)?;
    let origin = match resolved.kind {
        SkillRefKind::Registry => Origin::Registry {
            tier: resolved
                .trust_tier
                .as_deref()
                .and_then(TrustTier::from_name)
                .ok_or_else(|| format!("registry skill {reference} carries no trust tier"))?,
            publisher: resolved.skill_id.as_deref().and_then(publisher_of),
        },
        SkillRefKind::Bundled => Origin::Registry {
            tier: TrustTier::FirstParty,
            publisher: Some("runx".to_owned()),
        },
        _ => recorded_origin(paths, &directory)?,
    };
    let loaded = Arc::new(
        crate::load_validated_skill_package(&directory).map_err(|error| error.to_string())?,
    );
    let manifest = loaded
        .manifest()
        .ok_or_else(|| format!("{reference} has no run.yaml runner to load"))?;
    let runners = match runner {
        Some(runner) if manifest.runners.contains_key(runner) => vec![runner.to_owned()],
        Some(runner) => return Err(format!("{reference} has no runner {runner}")),
        None => manifest.runners.keys().cloned().collect(),
    };
    let package_digest = loaded.package.package_digest.clone();
    let description = loaded.package.skill.description.clone();
    let package_name = match (&resolved.skill_id, &resolved.version) {
        (Some(skill_id), Some(version)) => format!("{skill_id}@{version}"),
        _ => directory_name(&directory, paths)?,
    };
    let workspace_path = directory
        .strip_prefix(canonical(paths.workspace())?)
        .map(Path::to_path_buf)
        .map_err(|_| format!("{reference} lies outside the workspace"))?;
    let skill: NonEmptyString = workspace_path.to_string_lossy().into_owned().into();
    let mut tools = Vec::new();
    let mut closures = Vec::new();
    let mut connectors = BTreeMap::new();
    for runner in &runners {
        let binding = crate::skill_package::verify_loaded_execution_binding(
            Arc::clone(&loaded),
            runner,
            env,
            None,
            None,
        )
        .map_err(|error| format!("{reference}#{runner}: {error}"))?;
        let closure_digest = binding.closure_digest.ok_or_else(|| {
            format!("{reference}#{runner}: its execution closure is not fully bound")
        })?;
        for connector in crate::skill_package::case_closure_connectors(
            &loaded,
            crate::skill_package::CaseClosureTarget::Runner(runner),
            env,
        )
        .map_err(|error| format!("{reference}#{runner}: {error}"))?
        {
            agree(&mut connectors, &connector, reference)?;
        }
        tools.push(
            super::super::turn::skill_descriptor(
                &super::super::turn::loaded_tool_name(&package_name, runner),
                description.clone(),
                &manifest.runners[runner],
                TurnTarget::Skill {
                    skill: skill.clone(),
                    runner: Some(runner.as_str().into()),
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
    let declares = JsonValue::from_serialize(&serde_json::json!({
        "runners": runners,
        "connectors": connectors.keys().collect::<Vec<_>>(),
    }))
    .map_err(|error| error.to_string())?;
    let (trust_tier, publisher) = tier_and_publisher(&origin);
    Ok(Bound {
        package: LoadedPackage::Skill {
            skill: package_name.into(),
            trust_tier,
            publisher,
            package_digest: package_digest.into(),
        },
        origin,
        tools,
        closures,
        connectors,
        declares,
    })
}

/// The connector `name` (or `name@version`) names: one the project holds,
/// else the version the hosted registry publishes, added to the project.
fn bind_connector(
    name: &str,
    paths: &RunxPaths,
    env: &BTreeMap<String, String>,
) -> Result<Bound, String> {
    let scope = || {
        ConnectorScope::at(paths.workspace())
            .map(|scope| scope.in_project(paths.connectors()))
            .map_err(|error| error.to_string())
    };
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
            let RegistryTarget::Remote { registry_url } =
                resolve_registry_target(None, env, paths.workspace())
            else {
                return Err(local.to_string());
            };
            install_connector(
                &connector_name,
                version,
                None,
                &registry_url,
                env,
                paths.workspace(),
            )
            .map_err(crate::registry::resolve::ResolveError::into_message)?
            .ok_or_else(|| format!("{local}; and no registry publishes connector {name}"))?;
            scope()?.named(name).map_err(|error| error.to_string())?
        }
    };
    let origin = connector_origin(&connector, paths)?;
    let mut connectors = BTreeMap::new();
    agree(&mut connectors, &connector, name)?;
    // Each operation as a call on the connector, pinned to the manifest the
    // load bound.
    let declared = runx_parser::ConnectorRequirement {
        operations: connector.manifest.operations.keys().cloned().collect(),
        manifest_digest: Some(connector.manifest_digest.clone()),
        connector_from: None,
        allowed_connectors: Vec::new(),
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
    let (trust_tier, publisher) = tier_and_publisher(&origin);
    Ok(Bound {
        package: LoadedPackage::Connector {
            connector: connector.name.as_str().into(),
            trust_tier,
            publisher,
            manifest_digest: connector.manifest_digest.as_str().into(),
        },
        origin,
        tools,
        closures: vec![connector.connector_digest.as_str().to_owned()],
        connectors,
        declares,
    })
}

/// A runtime capability is runx's; an added version is the registry's, as
/// the project recorded it; any other is the project's own.
fn connector_origin(connector: &LoadedConnector, paths: &RunxPaths) -> Result<Origin, String> {
    let capability = ConnectorScope::runtime_capabilities()
        .map_err(|error| error.to_string())?
        .iter()
        .any(|capability| {
            capability.name == connector.name
                && capability.connector_digest == connector.connector_digest
        });
    if capability {
        return Ok(Origin::Registry {
            tier: TrustTier::FirstParty,
            publisher: Some("runx".to_owned()),
        });
    }
    match &connector.origin {
        crate::kernel::connectors::ConnectorOrigin::Installed { directory } => {
            recorded_origin(paths, &canonical(directory)?)
        }
        crate::kernel::connectors::ConnectorOrigin::Plugin { .. } => Ok(Origin::Project),
        crate::kernel::connectors::ConnectorOrigin::RuntimeCapability => Ok(Origin::Registry {
            tier: TrustTier::FirstParty,
            publisher: Some("runx".to_owned()),
        }),
    }
}

/// A package on disk is the registry's when the project recorded it so, and
/// otherwise the project's own, which it holds inside its workspace.
fn recorded_origin(paths: &RunxPaths, directory: &Path) -> Result<Origin, String> {
    let workspace = canonical(paths.workspace())?;
    match crate::registry::read_install_record(paths, directory)? {
        Some(record) => Ok(Origin::Registry {
            tier: record.trust_tier,
            publisher: publisher_of(&record.reference),
        }),
        None if directory.starts_with(&workspace) => Ok(Origin::Project),
        None => Err(format!(
            "{} is neither the project's own nor a package a registry added",
            directory.display()
        )),
    }
}

/// What the rules make of a package from `origin`.
fn ruling(origin: &Origin, rules: &[CatalogRule], what: &str) -> Ruling {
    match origin {
        Origin::Project if rules.iter().any(|rule| rule.project) => Ruling::Admitted,
        Origin::Project => Ruling::Refused(format!(
            "{what} is the project's own, and the turn's rules admit only the catalog"
        )),
        Origin::Registry { tier, .. } => {
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
            Ruling::AsksTrust {
                minimum: minimums
                    .into_iter()
                    .reduce(|lowest, minimum| {
                        if lowest.meets(minimum) {
                            minimum
                        } else {
                            lowest
                        }
                    })
                    .unwrap_or(TrustTier::Verified),
            }
        }
    }
}

/// Why what `bound` reaches disagrees with what the turn already binds.
fn revision_refusal(bound: &Bound, held: &BTreeMap<String, String>) -> Option<String> {
    bound.connectors.iter().find_map(|(name, digest)| {
        held.get(name).filter(|held| *held != digest).map(|held| {
            format!(
                "{} binds connector {name} at manifest {digest}, and this run already binds it at {held}; one run binds one revision of a name",
                describe(&bound.package)
            )
        })
    })
}

/// Ask the person to trust `bound`; record their trust for its closures.
fn ask_trust<A>(
    runtime: &super::Runtime<A>,
    graph_dir: &Path,
    step: &runx_parser::GraphStep,
    bound: &Bound,
    minimum: TrustTier,
    paths: &RunxPaths,
    host: &mut dyn crate::host::Host,
) -> Result<(), RuntimeError> {
    let options = &runtime.options;
    let what = describe(&bound.package);
    let tier = match &bound.origin {
        Origin::Registry { tier, .. } => *tier,
        Origin::Project => TrustTier::FirstParty,
    };
    let gate_id = format!("{}/trust", step.id);
    let request_id = options.run.address(&gate_id);
    let mut gate = ApprovalGate {
        id: gate_id.into(),
        reason: format!(
            "Trust {what}, a {} package, below the turn's {} rule.",
            tier.as_str(),
            minimum.as_str()
        )
        .into(),
        gate_type: Some("trust".to_owned()),
        summary: Some(JsonObject::from([(
            "step".to_owned(),
            JsonValue::String(step.id.clone()),
        )])),
        payload: None,
        payload_ref: None,
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
        &RunxPaths::resolve(&options.env, graph_dir).artifacts(),
        options.run.run_id(),
    )
    .map_err(|message| RuntimeError::effect_state("carrying a load", message))?;
    let resolution = crate::approval::request_approval(host, request_id.clone(), gate.clone())
        .map_err(|error| RuntimeError::effect_state("asking for trust", error.to_string()))?;
    match resolution {
        ApprovalResolution::Approved { .. } => {
            for closure in &bound.closures {
                crate::registry::record_trust(
                    paths,
                    &crate::registry::TrustRecord {
                        closure_digest: closure.clone(),
                        reference: what.clone(),
                        trust_tier: tier,
                    },
                )
                .map_err(|message| RuntimeError::effect_state("recording trust", message))?;
            }
            Ok(())
        }
        ApprovalResolution::Denied { reason, .. } => Err(RuntimeError::AuthorityDenied {
            verb: AuthorityVerb::Execute,
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
    connectors: &mut BTreeMap<String, String>,
    connector: &LoadedConnector,
    what: &str,
) -> Result<(), String> {
    let digest = connector.manifest_digest.as_str().to_owned();
    match connectors.insert(connector.name.as_str().to_owned(), digest.clone()) {
        Some(other) if other != digest => Err(format!(
            "{what} reaches two revisions of connector {} (manifest {other} and {digest})",
            connector.name
        )),
        _ => Ok(()),
    }
}

fn tier_and_publisher(origin: &Origin) -> (Option<TrustTier>, Option<NonEmptyString>) {
    match origin {
        Origin::Project => (None, None),
        Origin::Registry { tier, publisher } => (
            Some(*tier),
            publisher.as_deref().and_then(NonEmptyString::new),
        ),
    }
}

/// The publisher of `<publisher>/<name>[@version]`.
fn publisher_of(reference: &str) -> Option<String> {
    reference
        .split_once('/')
        .map(|(publisher, _)| publisher.to_owned())
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

    fn registry(tier: TrustTier) -> Origin {
        Origin::Registry {
            tier,
            publisher: Some("acme".to_owned()),
        }
    }

    #[test]
    fn a_rule_admits_by_origin_and_tier() {
        let rules = [catalog(TrustTier::Verified), PROJECT];
        assert_eq!(ruling(&Origin::Project, &rules, "it"), Ruling::Admitted);
        assert_eq!(
            ruling(&registry(TrustTier::FirstParty), &rules, "it"),
            Ruling::Admitted
        );
        assert_eq!(
            ruling(&registry(TrustTier::Verified), &rules, "it"),
            Ruling::Admitted
        );
        assert_eq!(
            ruling(&registry(TrustTier::Community), &rules, "it"),
            Ruling::AsksTrust {
                minimum: TrustTier::Verified
            }
        );
    }

    #[test]
    fn a_load_binding_another_revision_of_a_held_connector_is_refused() {
        let bound = Bound {
            package: LoadedPackage::Connector {
                connector: "github".into(),
                trust_tier: None,
                publisher: None,
                manifest_digest: "sha256:b".into(),
            },
            origin: Origin::Project,
            tools: Vec::new(),
            closures: Vec::new(),
            connectors: BTreeMap::from([("github".to_owned(), "sha256:b".to_owned())]),
            declares: JsonValue::Null,
        };
        let held = |digest: &str| BTreeMap::from([("github".to_owned(), digest.to_owned())]);
        let reason = revision_refusal(&bound, &held("sha256:a")).unwrap_or_default();
        assert!(
            reason.contains("one run binds one revision of a name"),
            "{reason}"
        );
        assert_eq!(revision_refusal(&bound, &held("sha256:b")), None);
        assert_eq!(revision_refusal(&bound, &BTreeMap::new()), None);
    }

    #[test]
    fn a_rule_of_one_origin_refuses_the_other() {
        let refused = |origin: &Origin, rule: CatalogRule| match ruling(origin, &[rule], "skill x")
        {
            Ruling::Refused(reason) => reason,
            other => format!("not refused: {other:?}"),
        };
        let reason = refused(&Origin::Project, catalog(TrustTier::Community));
        assert!(reason.contains("admit only the catalog"), "{reason}");
        let reason = refused(&registry(TrustTier::FirstParty), PROJECT);
        assert!(reason.contains("admit only the project's own"), "{reason}");
    }
}
