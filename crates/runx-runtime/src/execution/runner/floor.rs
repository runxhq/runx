//! The approval floor. In a run whose authority sets `agent_bounds`, every
//! step that changes the workspace or runs a process waits, at any depth, on
//! an approval of gate type `tool` carrying exactly what it would do, unless a
//! standing approval the person granted covers it. It is decided when the call
//! or step is admitted, before it is claimed, so nothing waiting on the person
//! is ever "may have run". A connector operation is governed by its own
//! declared approval, whatever implements it, so the floor never holds one.

use std::path::Path;

use runx_contracts::{
    ApprovalGate, AuthorityVerb, JsonObject, JsonValue, Reference, ReferenceType,
    ResolutionRequest, StandingApproval,
};
use runx_parser::{GraphStep, StepKind, ToolTarget};

use super::Runtime;
use crate::RuntimeError;
use crate::approval::{ApprovalError, ApprovalResolution};
use crate::capability::LocalEffect;
use crate::host::Host;

/// What admitted a step the floor would hold.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Admitted {
    /// The step changes nothing locally, or the run is an operator's.
    Free,
    /// The person approved the exact effect.
    Person,
    /// A standing approval the person granted covers it.
    Standing(StandingApproval),
}

impl Admitted {
    /// The reference the step's receipt carries for whose confirmation let it
    /// run.
    pub(super) fn reference(&self) -> Result<Option<Reference>, RuntimeError> {
        let mut reference = match self {
            Self::Free => return Ok(None),
            Self::Person => Reference::runx(ReferenceType::ScopeAdmission, "approval:person"),
            Self::Standing(entry) => Reference::runx(
                ReferenceType::ScopeAdmission,
                &format!(
                    "standing:{}",
                    runx_contracts::canonical_digest(entry).map_err(|error| {
                        RuntimeError::EngineInvariant {
                            context: "naming a standing approval",
                            message: error.to_string(),
                        }
                    })?
                ),
            ),
        };
        reference.label = Some(
            match self {
                Self::Person => "approved by a person",
                _ => "a standing approval",
            }
            .into(),
        );
        Ok(Some(reference))
    }

    /// The standing approval that admitted the step, if one did.
    pub(super) fn standing(&self) -> Option<&StandingApproval> {
        match self {
            Self::Standing(entry) => Some(entry),
            Self::Free | Self::Person => None,
        }
    }
}

/// Decide the floor for `step` with `inputs`: free, covered by a standing
/// approval, or approved by the person through `host`. A step still waiting
/// on the person is pending on its gate; one the person declined is refused.
pub(super) fn hold<A>(
    runtime: &Runtime<A>,
    graph_dir: &Path,
    step: &GraphStep,
    inputs: &JsonObject,
    host: &mut dyn Host,
) -> Result<Admitted, RuntimeError> {
    let options = &runtime.options;
    let Some(authority) = options
        .run_authority
        .as_ref()
        .filter(|authority| authority.agent_bounds)
    else {
        return Ok(Admitted::Free);
    };
    let Some(effect) = local_effect(runtime, graph_dir, step, inputs)? else {
        return Ok(Admitted::Free);
    };
    if let Some(entry) = covering(&effect, &authority.standing_approvals) {
        return Ok(Admitted::Standing(entry.clone()));
    }
    let gate_id = format!("{}/tool", step.id);
    let request_id = options.run.address(&gate_id);
    let mut gate = ApprovalGate {
        id: gate_id.into(),
        reason: reason(&effect).into(),
        gate_type: Some("tool".to_owned()),
        summary: Some(JsonObject::from([(
            "step".to_owned(),
            JsonValue::String(step.id.clone()),
        )])),
        payload: None,
        payload_artifact: None,
    };
    let stale = changed(&effect);
    crate::approval::carry_payload(
        &mut gate,
        effect.payload,
        &crate::paths::RunxPaths::resolve(&options.env, graph_dir).artifacts(),
        options.run.run_id(),
    )
    .map_err(|message| RuntimeError::effect_state("carrying a local effect", message))?;
    let resolution = match crate::approval::request_approval(host, request_id.clone(), gate.clone())
    {
        Ok(resolution) => resolution,
        // The effect is not the one the person was asked about: a file it
        // changes changed since. What they approved never runs; the call is
        // refused, naming what changed, and its agent reads it again.
        Err(ApprovalError::Runtime(RuntimeError::ContinuationRejected { .. })) => {
            return Err(RuntimeError::AuthorityDenied {
                verb: AuthorityVerb::Execute,
                step_id: step.id.clone(),
                reason: stale,
            });
        }
        Err(error) => {
            return Err(RuntimeError::effect_state(
                "asking for a local effect",
                error.to_string(),
            ));
        }
    };
    match resolution {
        ApprovalResolution::Approved { .. } => Ok(Admitted::Person),
        ApprovalResolution::Denied { reason, .. } => Err(RuntimeError::AuthorityDenied {
            verb: AuthorityVerb::Execute,
            step_id: step.id.clone(),
            reason: reason.unwrap_or_else(|| format!("the person declined {}", gate.reason)),
        }),
        ApprovalResolution::Pending { .. } => Err(RuntimeError::ResolutionPending {
            step_id: step.id.clone(),
            reason: format!("{} waits on the person", step.id),
            request: Box::new(ResolutionRequest::Approval {
                id: request_id.into(),
                gate: Box::new(gate),
            }),
            child: None,
        }),
    }
}

/// What `step` with `inputs` would change or run, when it is a local effect:
/// a core tool that changes something, describing itself; a catalog tool runx
/// runs as a process; a graph's process step.
fn local_effect<A>(
    runtime: &Runtime<A>,
    graph_dir: &Path,
    step: &GraphStep,
    inputs: &JsonObject,
) -> Result<Option<LocalEffect>, RuntimeError> {
    let options = &runtime.options;
    let bounds = crate::services::WorkspaceBounds::of(options.run_authority.as_ref());
    match &step.kind {
        StepKind::Tool(ToolTarget::Catalog(tool))
            if crate::tool_catalogs::native::is_core_tool(tool) =>
        {
            crate::tool_catalogs::native::local_effect(
                tool,
                inputs.clone(),
                &options.env,
                graph_dir,
                &options.effects,
                bounds,
            )
        }
        StepKind::Tool(ToolTarget::Catalog(tool)) => {
            let inspected = crate::tool_catalogs::dispatch::inspect_catalog_tool(
                tool,
                &options.env,
                graph_dir,
                &options.effects,
            )?;
            Ok(inspected
                .runtime
                .filter(|_| inspected.execution == "process")
                .map(|runtime| {
                    process_effect(
                        Some(tool),
                        &runtime.command,
                        &runtime.args,
                        runtime.cwd.as_deref(),
                        inputs,
                    )
                }))
        }
        StepKind::Tool(ToolTarget::Process(process)) => Ok(Some(process_effect(
            None,
            &process.command,
            &process.args,
            process.cwd.as_deref(),
            inputs,
        ))),
        _ => Ok(None),
    }
}

/// A process runx runs for a skill: its argv, where, and the input it reads.
fn process_effect(
    tool: Option<&str>,
    command: &str,
    args: &[String],
    cwd: Option<&str>,
    inputs: &JsonObject,
) -> LocalEffect {
    let argv = std::iter::once(command.to_owned())
        .chain(args.iter().cloned())
        .collect::<Vec<_>>();
    let mut payload = JsonObject::from([
        ("kind".to_owned(), JsonValue::String("process".to_owned())),
        (
            "argv".to_owned(),
            JsonValue::Array(argv.iter().cloned().map(JsonValue::String).collect()),
        ),
        ("input".to_owned(), JsonValue::Object(inputs.clone())),
    ]);
    if let Some(tool) = tool {
        payload.insert("tool".to_owned(), JsonValue::String(tool.to_owned()));
    }
    if let Some(cwd) = cwd {
        payload.insert("cwd".to_owned(), JsonValue::String(cwd.to_owned()));
    }
    LocalEffect {
        payload: JsonValue::Object(payload),
        paths: Vec::new(),
        argv: Some(argv),
    }
}

/// The standing approval in `approvals` that covers `effect`: `local`
/// covers any; `edits` covers changes to files all under its paths;
/// `commands` covers a process whose argv begins with its tokens.
pub(super) fn covering<'a>(
    effect: &LocalEffect,
    approvals: &'a [StandingApproval],
) -> Option<&'a StandingApproval> {
    approvals.iter().find(|entry| {
        if entry.local {
            return true;
        }
        if let Some(edits) = &entry.edits {
            return effect.argv.is_none()
                && !effect.paths.is_empty()
                && effect.paths.iter().all(|path| {
                    edits
                        .paths
                        .iter()
                        .any(|prefix| Path::new(path).starts_with(prefix))
                });
        }
        if let Some(commands) = &entry.commands {
            return effect.argv.as_ref().is_some_and(|argv| {
                argv.len() >= commands.argv_prefix.len()
                    && argv
                        .iter()
                        .zip(&commands.argv_prefix)
                        .all(|(token, prefix)| token == prefix.as_str())
            });
        }
        false
    })
}

/// Whether `approvals` cover `request`, an open approval of the floor (gate
/// type `tool`), judged by what its payload says the step changes or runs.
pub(crate) fn covers(
    request: &ResolutionRequest,
    approvals: &[StandingApproval],
    artifacts: &Path,
) -> Result<bool, RuntimeError> {
    let ResolutionRequest::Approval { gate, .. } = request else {
        return Ok(false);
    };
    if gate.gate_type.as_deref() != Some("tool") {
        return Ok(false);
    }
    let payload = match (&gate.payload, &gate.payload_artifact) {
        (Some(payload), _) => payload.clone(),
        (None, Some(reference)) => crate::services::read_json_artifact(
            artifacts,
            &JsonValue::from_serialize(reference).map_err(|error| {
                RuntimeError::effect_state("reading a local effect", error.to_string())
            })?,
        )
        .map_err(|message| RuntimeError::effect_state("reading a local effect", message))?,
        (None, None) => return Ok(false),
    };
    let strings = |value: Option<&JsonValue>| -> Vec<String> {
        value
            .and_then(JsonValue::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(JsonValue::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let payload = payload.as_object().cloned().unwrap_or_default();
    let effect = match payload.get("kind").and_then(JsonValue::as_str) {
        Some("edit") => LocalEffect {
            payload: JsonValue::Null,
            paths: payload
                .get("files")
                .and_then(JsonValue::as_array)
                .map(|files| {
                    files
                        .iter()
                        .filter_map(|file| file.as_object()?.get("path")?.as_str())
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
            argv: None,
        },
        Some("command" | "process") => LocalEffect {
            payload: JsonValue::Null,
            paths: Vec::new(),
            argv: Some(strings(payload.get("argv"))),
        },
        _ => return Ok(false),
    };
    Ok(covering(&effect, approvals).is_some())
}

/// What the person is asked: the command it runs, or the files it changes.
fn reason(effect: &LocalEffect) -> String {
    match &effect.argv {
        Some(argv) => format!("Run {}", argv.join(" ")),
        None => format!("Change {}", effect.paths.join(", ")),
    }
}

/// Why an effect the person was asked about no longer runs.
fn changed(effect: &LocalEffect) -> String {
    match &effect.argv {
        Some(argv) => format!(
            "{} changed after the person was asked to approve it",
            argv.join(" ")
        ),
        None => format!(
            "{} changed after the person was asked to approve the change; read it again",
            effect.paths.join(", ")
        ),
    }
}

#[cfg(test)]
mod tests;
