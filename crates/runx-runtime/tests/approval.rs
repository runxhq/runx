#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::VecDeque;

use runx_contracts::{
    ApprovalGate, ExecutionEvent, JsonObject, JsonValue, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_runtime::{
    ApprovalError, Host, LocalApprovalGateResolver, RuntimeError, request_approval,
};

#[test]
fn approval_response_approved() -> Result<(), Box<dyn std::error::Error>> {
    let mut host = RecordingHost::with_responses([Some(response(
        ResolutionResponseActor::Human,
        JsonValue::Bool(true),
    ))]);

    let resolution = request_approval(&mut host, "req_approval", gate())?;

    assert_eq!(resolution.approved(), Some(true));
    assert_eq!(resolution.actor(), Some(&ResolutionResponseActor::Human));
    assert_eq!(resolution.reason(), None);
    assert_eq!(host.requests.len(), 1);
    assert_approval_request(host.requests.first(), "req_approval")?;
    assert_resolution_events(&host.events, Some(true))?;
    Ok(())
}

#[test]
fn approval_response_denied() -> Result<(), Box<dyn std::error::Error>> {
    let mut host = RecordingHost::with_responses([Some(response(
        ResolutionResponseActor::Human,
        JsonValue::Bool(false),
    ))]);

    let resolution = request_approval(&mut host, "req_approval", gate())?;

    assert_eq!(resolution.approved(), Some(false));
    assert_eq!(resolution.actor(), Some(&ResolutionResponseActor::Human));
    assert_eq!(resolution.reason(), None);
    assert_resolution_events(&host.events, Some(false))?;
    Ok(())
}

#[test]
fn approval_response_pending_without_host_resolution() -> Result<(), Box<dyn std::error::Error>> {
    let mut host = RecordingHost::with_responses([None]);

    let resolution = request_approval(&mut host, "req_approval", gate())?;

    assert_eq!(resolution.approved(), None);
    assert_eq!(resolution.actor(), None);
    assert_eq!(host.requests.len(), 1);
    assert_resolution_events(&host.events, None)?;
    Ok(())
}

#[test]
fn approval_rejects_string_boolean_payload() -> Result<(), Box<dyn std::error::Error>> {
    let mut host = RecordingHost::with_responses([Some(response(
        ResolutionResponseActor::Human,
        JsonValue::String("true".to_owned()),
    ))]);

    let result = request_approval(&mut host, "req_approval", gate());

    match result {
        Err(ApprovalError::NonBooleanPayload {
            actor,
            payload_type,
        }) => {
            assert_eq!(actor, ResolutionResponseActor::Human);
            assert_eq!(payload_type, "string");
        }
        Ok(other) => {
            return Err(std::io::Error::other(format!(
                "expected non-boolean payload error, got {other:?}"
            ))
            .into());
        }
        Err(other) => {
            return Err(std::io::Error::other(format!("unexpected error: {other}")).into());
        }
    }
    assert_resolution_events(&host.events, None)?;
    Ok(())
}

#[test]
fn approval_rejects_agent_actor() -> Result<(), Box<dyn std::error::Error>> {
    let parsed: ResolutionResponse = serde_json::from_str(r#"{"actor":"agent","payload":true}"#)?;
    let mut host = RecordingHost::with_responses([Some(parsed)]);

    assert!(matches!(
        request_approval(&mut host, "req_approval", gate()),
        Err(ApprovalError::HumanApprovalRequired {
            actor: ResolutionResponseActor::Agent
        })
    ));
    Ok(())
}

#[test]
fn approval_rejects_agent_reasoned_decision() -> Result<(), Box<dyn std::error::Error>> {
    let parsed: ResolutionResponse = serde_json::from_str(
        r#"{"actor":"agent","payload":{"approved":true,"reason":"verdict 5/5 met the rubric"}}"#,
    )?;
    let mut host = RecordingHost::with_responses([Some(parsed)]);

    assert!(matches!(
        request_approval(&mut host, "req_approval", gate()),
        Err(ApprovalError::HumanApprovalRequired {
            actor: ResolutionResponseActor::Agent
        })
    ));
    assert_resolution_events(&host.events, None)?;
    Ok(())
}

#[test]
fn approval_object_payload_without_bool_approved_rejected_fail_closed()
-> Result<(), Box<dyn std::error::Error>> {
    let parsed: ResolutionResponse = serde_json::from_str(
        r#"{"actor":"human","payload":{"approved":"true","reason":"stringly bool"}}"#,
    )?;
    let mut host = RecordingHost::with_responses([Some(parsed)]);

    let result = request_approval(&mut host, "req_approval", gate());

    match result {
        Err(ApprovalError::NonBooleanPayload {
            actor,
            payload_type,
        }) => {
            assert_eq!(actor, ResolutionResponseActor::Human);
            assert_eq!(payload_type, "object");
        }
        other => {
            return Err(std::io::Error::other(format!(
                "expected fail-closed non-boolean payload error, got {other:?}"
            ))
            .into());
        }
    }
    Ok(())
}

#[test]
fn approval_dedupes_resolved_gate_by_canonical_idempotency_key()
-> Result<(), Box<dyn std::error::Error>> {
    let mut resolver = LocalApprovalGateResolver::new();
    let mut host = RecordingHost::with_responses([
        Some(response(
            ResolutionResponseActor::Human,
            JsonValue::Bool(true),
        )),
        Some(response(
            ResolutionResponseActor::Human,
            JsonValue::Bool(false),
        )),
    ]);

    let first = resolver.request_approval(&mut host, "req_approval", gate())?;
    let second = resolver.request_approval(&mut host, "req_duplicate", gate())?;

    assert_eq!(first.approved(), Some(true));
    assert_eq!(second.approved(), Some(true));
    assert_eq!(first.idempotency_key(), second.idempotency_key());
    assert_eq!(host.requests.len(), 1);
    assert_eq!(host.events.len(), 2);
    Ok(())
}

#[test]
fn approval_optional_fields_omit_null_via_host_protocol_serde()
-> Result<(), Box<dyn std::error::Error>> {
    let request = ResolutionRequest::Approval {
        id: "req_approval".into(),
        gate: Box::new(ApprovalGate {
            id: "workspace-write".into(),
            reason: "Allow workspace write".into(),
            gate_type: None,
            summary: None,
            payload: None,
            payload_ref: None,
        }),
    };

    let actual = serde_json::to_string(&request)?;

    assert_eq!(
        actual,
        r#"{"kind":"approval","id":"req_approval","gate":{"id":"workspace-write","reason":"Allow workspace write"}}"#
    );
    Ok(())
}

#[test]
fn raw_gate_type_alternate_shape_rejected_by_host_protocol_serde() {
    let result = serde_json::from_str::<ResolutionRequest>(
        r#"{"kind":"approval","id":"req_approval","gate":{"id":"workspace-write","reason":"Allow workspace write","gate_type":"filesystem_write"}}"#,
    );

    assert!(result.is_err());
}

#[derive(Default)]
struct RecordingHost {
    events: Vec<ExecutionEvent>,
    requests: Vec<ResolutionRequest>,
    responses: VecDeque<Option<ResolutionResponse>>,
}

impl RecordingHost {
    fn with_responses<const N: usize>(responses: [Option<ResolutionResponse>; N]) -> Self {
        Self {
            responses: VecDeque::from(responses),
            ..Self::default()
        }
    }
}

impl Host for RecordingHost {
    fn report(&mut self, event: ExecutionEvent) -> Result<(), RuntimeError> {
        self.events.push(event);
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        self.requests.push(request);
        Ok(self.responses.pop_front().flatten())
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

fn gate() -> ApprovalGate {
    ApprovalGate {
        id: "workspace-write".into(),
        reason: "Allow workspace write".into(),
        gate_type: Some("filesystem_write".to_owned()),
        summary: Some(summary()),
        payload: None,
        payload_ref: None,
    }
}

fn summary() -> JsonObject {
    [(
        "path".to_owned(),
        JsonValue::String("docs/guide.md".to_owned()),
    )]
    .into()
}

fn response(actor: ResolutionResponseActor, payload: JsonValue) -> ResolutionResponse {
    ResolutionResponse { actor, payload }
}

fn assert_approval_request(
    request: Option<&ResolutionRequest>,
    expected_id: &str,
) -> Result<(), std::io::Error> {
    let Some(ResolutionRequest::Approval { id, gate }) = request else {
        return Err(std::io::Error::other("missing approval request"));
    };
    assert_eq!(id, expected_id);
    assert_eq!(gate.id, "workspace-write");
    assert_eq!(gate.gate_type.as_deref(), Some("filesystem_write"));
    Ok(())
}

fn assert_resolution_events(
    events: &[ExecutionEvent],
    approved: Option<bool>,
) -> Result<(), std::io::Error> {
    let Some(ExecutionEvent::ResolutionRequested { data, .. }) = events.first() else {
        return Err(std::io::Error::other("missing resolution requested event"));
    };
    assert_event_key(
        data,
        "gate_id",
        JsonValue::String("workspace-write".to_owned()),
    )?;
    match approved {
        Some(value) => assert_resolved_event(events.get(1), value),
        None => {
            assert_eq!(events.len(), 1);
            Ok(())
        }
    }
}

fn assert_resolved_event(
    event: Option<&ExecutionEvent>,
    approved: bool,
) -> Result<(), std::io::Error> {
    let Some(ExecutionEvent::ResolutionResolved { data, .. }) = event else {
        return Err(std::io::Error::other("missing resolution resolved event"));
    };
    assert_event_key(data, "approved", JsonValue::Bool(approved))
}

fn assert_event_key(
    data: &Option<JsonValue>,
    key: &str,
    expected: JsonValue,
) -> Result<(), std::io::Error> {
    let Some(JsonValue::Object(object)) = data else {
        return Err(std::io::Error::other("event data must be an object"));
    };
    assert_eq!(object.get(key), Some(&expected));
    Ok(())
}
