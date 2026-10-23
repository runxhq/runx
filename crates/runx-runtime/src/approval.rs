use std::collections::{BTreeMap, BTreeSet};

use runx_contracts::{
    ApprovalGate, ExecutionEvent, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor, sha256_prefixed,
};
use thiserror::Error;

use crate::{Host, RuntimeError};

#[derive(Debug, Error)]
pub enum ApprovalError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(
        "approval response payload from {actor:?} must be a boolean or {{approved: bool, reason?: string}}, got {payload_type}"
    )]
    NonBooleanPayload {
        actor: ResolutionResponseActor,
        payload_type: &'static str,
    },
    #[error("approval response must come from a host-attested human, got {actor:?}")]
    HumanApprovalRequired { actor: ResolutionResponseActor },
    #[error("approval gate serialization failed while {context}: {source}")]
    Json {
        context: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApprovalResolution {
    Approved {
        actor: ResolutionResponseActor,
        idempotency_key: String,
        reason: Option<String>,
    },
    Denied {
        actor: ResolutionResponseActor,
        idempotency_key: String,
        reason: Option<String>,
    },
    Pending {
        idempotency_key: String,
    },
}

impl ApprovalResolution {
    #[must_use]
    pub fn approved(&self) -> Option<bool> {
        match self {
            Self::Approved { .. } => Some(true),
            Self::Denied { .. } => Some(false),
            Self::Pending { .. } => None,
        }
    }

    #[must_use]
    pub fn actor(&self) -> Option<&ResolutionResponseActor> {
        match self {
            Self::Approved { actor, .. } | Self::Denied { actor, .. } => Some(actor),
            Self::Pending { .. } => None,
        }
    }

    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Approved { reason, .. } | Self::Denied { reason, .. } => reason.as_deref(),
            Self::Pending { .. } => None,
        }
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &str {
        match self {
            Self::Approved {
                idempotency_key, ..
            }
            | Self::Denied {
                idempotency_key, ..
            }
            | Self::Pending { idempotency_key } => idempotency_key,
        }
    }
}

#[derive(Debug, Default)]
pub struct LocalApprovalGateResolver {
    requested: BTreeSet<String>,
    resolved: BTreeMap<String, CachedApproval>,
}

impl LocalApprovalGateResolver {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request_approval(
        &mut self,
        host: &mut dyn Host,
        id: impl Into<String>,
        gate: ApprovalGate,
    ) -> Result<ApprovalResolution, ApprovalError> {
        let id = id.into();
        let idempotency_key = approval_idempotency_key(&gate)?;
        if let Some(cached) = self.resolved.get(&idempotency_key) {
            return Ok(cached.resolution(idempotency_key));
        }

        self.report_requested(host, &id, &gate, &idempotency_key)?;
        let response = host.resolve(ResolutionRequest::Approval {
            id: id.clone().into(),
            gate: Box::new(gate.clone()),
        })?;
        let Some(response) = response else {
            return Ok(ApprovalResolution::Pending { idempotency_key });
        };

        self.resolve_response(host, &id, &gate, idempotency_key, response)
    }

    fn report_requested(
        &mut self,
        host: &mut dyn Host,
        id: &str,
        gate: &ApprovalGate,
        idempotency_key: &str,
    ) -> Result<(), ApprovalError> {
        if self.requested.insert(idempotency_key.to_owned()) {
            host.report(requested_event(id, gate, idempotency_key))?;
        }
        Ok(())
    }

    fn resolve_response(
        &mut self,
        host: &mut dyn Host,
        id: &str,
        gate: &ApprovalGate,
        idempotency_key: String,
        response: ResolutionResponse,
    ) -> Result<ApprovalResolution, ApprovalError> {
        let cached = CachedApproval::from_response(response)?;
        host.report(resolved_event(id, gate, &idempotency_key, &cached))?;
        let resolution = cached.resolution(idempotency_key.clone());
        self.resolved.insert(idempotency_key, cached);
        Ok(resolution)
    }
}

pub fn request_approval(
    host: &mut dyn Host,
    id: impl Into<String>,
    gate: ApprovalGate,
) -> Result<ApprovalResolution, ApprovalError> {
    LocalApprovalGateResolver::new().request_approval(host, id, gate)
}

pub fn approval_idempotency_key(gate: &ApprovalGate) -> Result<String, ApprovalError> {
    let canonical = serde_json::to_string(gate).map_err(|source| ApprovalError::Json {
        context: "serializing approval gate".to_owned(),
        source,
    })?;
    Ok(sha256_prefixed(canonical.as_bytes()))
}

/// The largest payload a gate carries inline; a larger one is a project
/// artifact.
const GATE_PAYLOAD_INLINE_BYTES: usize = 64 * 1024;

/// Carry `value`, what executes once `gate` is approved, on the gate: its
/// stable digest as `summary.payload_digest`, and the value itself inline,
/// or over 64 KiB as a project artifact of run `run_id` whose bytes are its
/// canonical JSON, so a host recomputes the digest before it offers Approve.
pub(crate) fn carry_payload(
    gate: &mut ApprovalGate,
    value: JsonValue,
    artifacts: &std::path::Path,
    run_id: &str,
) -> Result<(), String> {
    let bytes = runx_contracts::to_canonical_bytes(&value).map_err(|error| error.to_string())?;
    gate.summary.get_or_insert_with(JsonObject::new).insert(
        "payload_digest".to_owned(),
        JsonValue::String(sha256_prefixed(&bytes)),
    );
    if bytes.len() <= GATE_PAYLOAD_INLINE_BYTES {
        gate.payload = Some(value);
        return Ok(());
    }
    let reference = crate::services::persist_canonical_json_artifact(
        artifacts,
        run_id,
        "approvals",
        gate.id.as_str(),
        &value,
    )?;
    gate.payload_artifact = Some(
        serde_json::to_value(reference)
            .and_then(serde_json::from_value)
            .map_err(|error| error.to_string())?,
    );
    Ok(())
}

#[derive(Clone, Debug)]
struct CachedApproval {
    actor: ResolutionResponseActor,
    approved: bool,
    reason: Option<String>,
}

impl CachedApproval {
    // The single choke point for an approval decision. The boolean stays the
    // decision: a bare `true`/`false` or
    // `{ "approved": bool, "reason"?: string }` from the host-attested human
    // lane is accepted. Agent provenance and every other shape (including an
    // object whose `approved` is not strictly a boolean) fail closed.
    fn from_response(response: ResolutionResponse) -> Result<Self, ApprovalError> {
        let payload_type = payload_type(&response.payload);
        let actor = response.actor;
        if actor != ResolutionResponseActor::Human {
            return Err(ApprovalError::HumanApprovalRequired { actor });
        }
        let reject = || ApprovalError::NonBooleanPayload {
            actor: actor.clone(),
            payload_type,
        };
        match response.payload {
            JsonValue::Bool(approved) => Ok(Self {
                actor,
                approved,
                reason: None,
            }),
            JsonValue::Object(object) => {
                let Some(JsonValue::Bool(approved)) = object.get("approved") else {
                    return Err(reject());
                };
                let reason = match object.get("reason") {
                    None | Some(JsonValue::Null) => None,
                    Some(JsonValue::String(reason)) if !reason.trim().is_empty() => {
                        Some(reason.clone())
                    }
                    Some(_) => return Err(reject()),
                };
                Ok(Self {
                    actor,
                    approved: *approved,
                    reason,
                })
            }
            _ => Err(reject()),
        }
    }

    fn resolution(&self, idempotency_key: String) -> ApprovalResolution {
        if self.approved {
            ApprovalResolution::Approved {
                actor: self.actor.clone(),
                idempotency_key,
                reason: self.reason.clone(),
            }
        } else {
            ApprovalResolution::Denied {
                actor: self.actor.clone(),
                idempotency_key,
                reason: self.reason.clone(),
            }
        }
    }
}

fn requested_event(id: &str, gate: &ApprovalGate, idempotency_key: &str) -> ExecutionEvent {
    ExecutionEvent::ResolutionRequested {
        message: format!("approval {} requested", gate.id),
        data: Some(JsonValue::Object(event_data(id, gate, idempotency_key))),
    }
}

fn resolved_event(
    id: &str,
    gate: &ApprovalGate,
    idempotency_key: &str,
    approval: &CachedApproval,
) -> ExecutionEvent {
    let mut data = event_data(id, gate, idempotency_key);
    data.insert(
        "actor".to_owned(),
        JsonValue::String(actor_name(&approval.actor)),
    );
    data.insert("approved".to_owned(), JsonValue::Bool(approval.approved));
    if let Some(reason) = &approval.reason {
        data.insert("reason".to_owned(), JsonValue::String(reason.clone()));
    }
    let decision = if approval.approved {
        "approved"
    } else {
        "denied"
    };
    ExecutionEvent::ResolutionResolved {
        message: format!("approval {} {decision}", gate.id),
        data: Some(JsonValue::Object(data)),
    }
}

fn event_data(id: &str, gate: &ApprovalGate, idempotency_key: &str) -> JsonObject {
    let mut data = JsonObject::new();
    data.insert("request_id".to_owned(), JsonValue::String(id.to_owned()));
    data.insert(
        "gate_id".to_owned(),
        JsonValue::String(gate.id.as_str().to_owned()),
    );
    if let Some(gate_type) = &gate.gate_type {
        data.insert("gate_type".to_owned(), JsonValue::String(gate_type.clone()));
    }
    data.insert(
        "idempotency_key".to_owned(),
        JsonValue::String(idempotency_key.to_owned()),
    );
    data
}

fn actor_name(actor: &ResolutionResponseActor) -> String {
    match actor {
        ResolutionResponseActor::Human => "human".to_owned(),
        ResolutionResponseActor::Agent => "agent".to_owned(),
    }
}

fn payload_type(payload: &JsonValue) -> &'static str {
    match payload {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    }
}
