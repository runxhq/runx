//! The approval floor: where a run's `effects` decides, at any depth, each
//! step that would change something, when the call or step is admitted and
//! before it is claimed, so nothing waiting on the person is ever "may have
//! run". A change is a step that changes the workspace or runs a process, or
//! a connector operation that writes.
//!
//! Under `effects: allow` nothing waits. Under `ask` a change waits on an
//! approval carrying exactly what it would do, of gate type `tool` for a
//! local effect and `write` for an operation's write, unless a standing
//! approval the person granted covers it. A write that carries an approval
//! of its own, its operation's or its step's, keeps that one, and a package's
//! own `store` state is exempt. Under `none` every change is refused, a write
//! with its own approval and a `store` write included.

use std::path::Path;

use runx_contracts::{
    ApprovalGate, AuthorityVerb, Effects, JsonObject, JsonValue, Reference, ReferenceType,
    ResolutionRequest, StandingApproval,
};
use runx_parser::{StepKind, ToolTarget, WorkflowStep};

use super::Runtime;
use crate::RuntimeError;
use crate::approval::{ApprovalError, ApprovalResolution};
use crate::capability::LocalEffect;
use crate::host::Host;
use crate::kernel::operation::OperationWrite;

/// What admitted a step the floor would hold.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Admitted {
    /// The step changes nothing, or the run's `effects` hold nothing.
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

/// What a step would change that the run's `effects` decides.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Change {
    /// A change to the workspace, or a process runx runs.
    Local(LocalEffect),
    /// A connector operation's write, with the call it makes.
    Write {
        write: OperationWrite,
        payload: JsonValue,
    },
}

impl Change {
    /// The gate a person answers for it.
    fn gate_type(&self) -> &'static str {
        match self {
            Self::Local(_) => "tool",
            Self::Write { .. } => "write",
        }
    }

    /// What the person is asked: the command it runs, the files it changes,
    /// or the operation it calls.
    fn reason(&self) -> String {
        match self {
            Self::Local(effect) => match &effect.argv {
                Some(argv) => format!("Run {}", argv.join(" ")),
                None => format!("Change {}", effect.paths.join(", ")),
            },
            Self::Write { write, .. } => {
                format!("Call {}/{}", write.connector, write.operation)
            }
        }
    }

    /// Why the run refuses it under `effects: none`.
    fn refused(&self) -> String {
        let what = match self {
            Self::Local(effect) => match &effect.argv {
                Some(argv) => format!("running {}", argv.join(" ")),
                None => format!("changing {}", effect.paths.join(", ")),
            },
            Self::Write { write, .. } => {
                format!("the {}/{} write", write.connector, write.operation)
            }
        };
        format!("the run's effects are none, so it changes nothing: {what} is refused")
    }

    /// Why a change the person was asked about no longer runs.
    fn changed(&self) -> String {
        match self {
            Self::Local(effect) => match &effect.argv {
                Some(argv) => format!(
                    "{} changed after the person was asked to approve it",
                    argv.join(" ")
                ),
                None => format!(
                    "{} changed after the person was asked to approve the change; read it again",
                    effect.paths.join(", ")
                ),
            },
            Self::Write { write, .. } => format!(
                "the {}/{} call changed after the person was asked to approve it",
                write.connector, write.operation
            ),
        }
    }

    fn verb(&self) -> AuthorityVerb {
        match self {
            Self::Local(_) => AuthorityVerb::Execute,
            Self::Write { .. } => AuthorityVerb::Write,
        }
    }
}

/// Decide the floor for `step` with `inputs` under the run's `effects`: free,
/// covered by a standing approval, approved by the person through `host`, or
/// refused. A step still waiting on the person is pending on its gate; one
/// the person declined, or one `effects: none` refuses, is refused.
pub(super) fn hold<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    step: &WorkflowStep,
    inputs: &JsonObject,
    host: &mut dyn Host,
) -> Result<Admitted, RuntimeError> {
    let options = &runtime.options;
    let authority = options.run_authority.as_ref();
    let effects = crate::kernel::authority::step_effects(authority, step);
    if effects == Effects::Allow {
        return Ok(Admitted::Free);
    }
    let Some(change) = change(runtime, workflow_dir, step, inputs, effects)? else {
        return Ok(Admitted::Free);
    };
    if effects == Effects::None {
        return Err(RuntimeError::AuthorityDenied {
            verb: change.verb(),
            step_id: step.id.clone(),
            reason: change.refused(),
        });
    }
    let standing = authority.map_or(&[][..], |authority| &authority.standing_approvals);
    if let Some(entry) = covering(&change, standing) {
        return Ok(Admitted::Standing(entry.clone()));
    }
    let gate_type = change.gate_type();
    let gate_id = format!("{}/{gate_type}", step.id);
    let request_id = options.run.address(&gate_id);
    let mut gate = ApprovalGate {
        id: gate_id.into(),
        reason: change.reason().into(),
        gate_type: Some(gate_type.to_owned()),
        summary: Some(JsonObject::from([(
            "step".to_owned(),
            JsonValue::String(step.id.clone()),
        )])),
        payload: None,
        payload_artifact: None,
    };
    let stale = change.changed();
    let payload = match &change {
        Change::Local(effect) => effect.payload.clone(),
        Change::Write { payload, .. } => payload.clone(),
    };
    crate::approval::carry_payload(
        &mut gate,
        payload,
        &crate::paths::RunxPaths::resolve(&options.env, workflow_dir).artifacts(),
        options.run.run_id(),
    )
    .map_err(|message| RuntimeError::effect_state("carrying a change", message))?;
    let resolution = match crate::approval::request_approval(host, request_id.clone(), gate.clone())
    {
        Ok(resolution) => resolution,
        // The change is not the one the person was asked about: a file it
        // changes changed since. What they approved never runs; the call is
        // refused, naming what changed, and its agent reads it again.
        Err(ApprovalError::Runtime(RuntimeError::ContinuationRejected { .. })) => {
            return Err(RuntimeError::AuthorityDenied {
                verb: change.verb(),
                step_id: step.id.clone(),
                reason: stale,
            });
        }
        Err(error) => {
            return Err(RuntimeError::effect_state(
                "asking for a change",
                error.to_string(),
            ));
        }
    };
    match resolution {
        ApprovalResolution::Approved { .. } => Ok(Admitted::Person),
        ApprovalResolution::Denied { reason, .. } => Err(RuntimeError::AuthorityDenied {
            verb: change.verb(),
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

/// What `step` with `inputs` would change that `effects` decides: a local
/// effect, or an operation's write. Under `ask`, a write that carries an
/// approval of its own is decided by that approval, and a package's own
/// `store` state is exempt; a turn's own call never writes to `store`, so a
/// `store` write here is a package's deterministic step.
fn change<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    step: &WorkflowStep,
    inputs: &JsonObject,
    effects: Effects,
) -> Result<Option<Change>, RuntimeError> {
    let StepKind::Operation(target) = &step.kind else {
        return Ok(local_effect(runtime, workflow_dir, step, inputs)?.map(Change::Local));
    };
    let Some(write) = crate::kernel::operation::operation_write(
        target,
        inputs,
        &runtime.options.env,
        workflow_dir,
    ) else {
        return Ok(None);
    };
    if effects == Effects::Ask && (write.approved || write.store) {
        return Ok(None);
    }
    let payload = JsonValue::Object(JsonObject::from([
        ("kind".to_owned(), JsonValue::String("operation".to_owned())),
        (
            "connector".to_owned(),
            JsonValue::String(write.connector.clone()),
        ),
        (
            "operation".to_owned(),
            JsonValue::String(write.operation.clone()),
        ),
        ("call".to_owned(), JsonValue::Object(inputs.clone())),
    ]));
    Ok(Some(Change::Write { write, payload }))
}

/// What `step` with `inputs` would change or run, when it is a local effect:
/// a core tool that changes something, describing itself; a catalog tool runx
/// runs as a process; a graph's process step.
fn local_effect<A>(
    runtime: &Runtime<A>,
    workflow_dir: &Path,
    step: &WorkflowStep,
    inputs: &JsonObject,
) -> Result<Option<LocalEffect>, RuntimeError> {
    let options = &runtime.options;
    let reach = crate::kernel::authority::step_reach(options.run_authority.as_ref(), step);
    match &step.kind {
        StepKind::Tool(ToolTarget::Catalog(tool))
            if crate::tool_catalogs::native::is_core_tool(tool) =>
        {
            crate::tool_catalogs::native::local_effect(
                tool,
                inputs.clone(),
                &options.env,
                workflow_dir,
                &options.effects,
                reach,
            )
        }
        StepKind::Tool(ToolTarget::Catalog(tool)) => {
            let inspected = crate::tool_catalogs::dispatch::inspect_catalog_tool(
                tool,
                &options.env,
                workflow_dir,
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

/// The standing approval in `approvals` that covers `change`: `local` covers
/// any local effect; `edits` covers changes to files all under its paths;
/// `commands` covers a process whose argv begins with its tokens;
/// `operations` covers a write by the operation's name. None covers a write's
/// own approval, which never reaches the floor.
pub(super) fn covering<'a>(
    change: &Change,
    approvals: &'a [StandingApproval],
) -> Option<&'a StandingApproval> {
    approvals.iter().find(|entry| match change {
        Change::Write { write, .. } => entry
            .operations
            .as_ref()
            .is_some_and(|operations| operations.cover(&write.connector, &write.operation)),
        Change::Local(effect) => {
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
        }
    })
}

/// Whether `approvals` cover `request`, an open approval of the floor (gate
/// type `tool` or `write`), judged by what its payload says the step changes,
/// runs or calls.
pub(crate) fn covers(
    request: &ResolutionRequest,
    approvals: &[StandingApproval],
    artifacts: &Path,
) -> Result<bool, RuntimeError> {
    let ResolutionRequest::Approval { gate, .. } = request else {
        return Ok(false);
    };
    if !matches!(gate.gate_type.as_deref(), Some("tool" | "write")) {
        return Ok(false);
    }
    let payload = match (&gate.payload, &gate.payload_artifact) {
        (Some(payload), _) => payload.clone(),
        (None, Some(reference)) => crate::services::read_json_artifact(
            artifacts,
            &JsonValue::from_serialize(reference).map_err(|error| {
                RuntimeError::effect_state("reading a change", error.to_string())
            })?,
        )
        .map_err(|message| RuntimeError::effect_state("reading a change", message))?,
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
    let text = |payload: &JsonObject, key: &str| {
        payload
            .get(key)
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    };
    let payload = payload.as_object().cloned().unwrap_or_default();
    let change = match (
        gate.gate_type.as_deref(),
        payload.get("kind").and_then(JsonValue::as_str),
    ) {
        (Some("tool"), Some("edit")) => Change::Local(LocalEffect {
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
        }),
        (Some("tool"), Some("command" | "process")) => Change::Local(LocalEffect {
            payload: JsonValue::Null,
            paths: Vec::new(),
            argv: Some(strings(payload.get("argv"))),
        }),
        (Some("write"), Some("operation")) => {
            let (Some(connector), Some(operation)) =
                (text(&payload, "connector"), text(&payload, "operation"))
            else {
                return Ok(false);
            };
            Change::Write {
                write: OperationWrite {
                    connector,
                    operation,
                    approved: false,
                    store: false,
                },
                payload: JsonValue::Null,
            }
        }
        _ => return Ok(false),
    };
    Ok(covering(&change, approvals).is_some())
}

#[cfg(test)]
mod tests;
