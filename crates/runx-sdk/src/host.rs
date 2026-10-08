pub use runx_contracts::{
    AgentActInvocation, ApprovalDecision, ApprovalGate, ExecutionEvent, HostNeedsAgentState,
    HostRunApproval, HostRunApprovalDecision, HostRunKind, HostRunLineage, HostRunLineageKind,
    HostRunResult, HostRunState, HostRunVerification, HostRunVerificationStatus, HostTerminalState,
    Question, ResolutionRequest, ResolutionResponse, ResolutionResponseActor,
};

use crate::error::RunxResult;

#[must_use]
pub fn host_result_status(result: &HostRunResult) -> &'static str {
    match result {
        HostRunResult::NeedsAgent { .. } => "needs_agent",
        HostRunResult::Completed { .. } => "completed",
        HostRunResult::Failed { .. } => "failed",
        HostRunResult::Escalated { .. } => "escalated",
        HostRunResult::Denied { .. } => "denied",
    }
}

pub fn decode_host_result(json: &str) -> RunxResult<HostRunResult> {
    Ok(serde_json::from_str(json)?)
}

pub fn decode_host_state(json: &str) -> RunxResult<HostRunState> {
    Ok(serde_json::from_str(json)?)
}
