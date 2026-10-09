use runx_contracts::act::Act;
use runx_contracts::act::assignment::ActAssignment;
use runx_contracts::act::result::ActResultEnvelope;
use runx_contracts::agent_context::AgentContextEnvelope;
use runx_contracts::artifact::Artifact;
use runx_contracts::authority::{Authority, AuthoritySubsetProof, AuthorityTerm};
use runx_contracts::credential_delivery::CredentialDeliveryObservation;
use runx_contracts::decision::Decision;
use runx_contracts::dev::DevReport;
use runx_contracts::doctor::DoctorReport;
use runx_contracts::fixture::Fixture;
use runx_contracts::handoff::{HandoffSignal, HandoffState};
use runx_contracts::host_protocol::{
    AgentActInvocation, ApprovalGate, Question, ResolutionRequest, ResolutionResponse,
    TurnCallResult, TurnStatus,
};
use runx_contracts::ledger::LedgerEntry;
use runx_contracts::list::RunxListReport;
use runx_contracts::operational_policy::OperationalPolicy;
use runx_contracts::operational_proposal::OperationalProposal;
use runx_contracts::orchestrator_handoff::{
    OrchestratorExecutionContext, OrchestratorHandoffContext,
};
use runx_contracts::output::Output;
use runx_contracts::packet_index::PacketIndex;
use runx_contracts::receipt::Receipt;
use runx_contracts::redaction::Redaction;
use runx_contracts::reference::{Reference, ReferenceLink};
use runx_contracts::registry_binding::RegistryBinding;
use runx_contracts::review::ReviewReceiptOutput;
use runx_contracts::run_summary::RunSummary;
use runx_contracts::schema::RunxSchema;
use runx_contracts::signal::Signal;
use runx_contracts::source_packet::SourcePacket;
use runx_contracts::suppression::SuppressionRecord;
use runx_contracts::tools::ToolManifest;
use runx_contracts::verification::Verification;

use serde_json::Value;

use super::corpora::*;

pub(super) struct Covered {
    pub(super) file_name: &'static str,
    pub(super) emitted: Value,
    pub(super) corpus: Vec<(&'static str, Value)>,
}

pub(super) fn covered() -> Vec<Covered> {
    vec![
        Covered {
            file_name: "reference.schema.json",
            emitted: Reference::json_schema(),
            corpus: reference_corpus(),
        },
        Covered {
            file_name: "reference-link.schema.json",
            emitted: ReferenceLink::json_schema(),
            corpus: reference_link_corpus(),
        },
        Covered {
            file_name: "doctor.schema.json",
            emitted: DoctorReport::json_schema(),
            corpus: doctor_corpus(),
        },
        Covered {
            file_name: "redaction.schema.json",
            emitted: Redaction::json_schema(),
            corpus: redaction_corpus(),
        },
        Covered {
            file_name: "artifact.schema.json",
            emitted: Artifact::json_schema(),
            corpus: artifact_corpus(),
        },
        Covered {
            file_name: "verification.schema.json",
            emitted: Verification::json_schema(),
            corpus: verification_corpus(),
        },
        Covered {
            file_name: "signal.schema.json",
            emitted: Signal::json_schema(),
            corpus: signal_corpus(),
        },
        Covered {
            file_name: "source-packet.schema.json",
            emitted: SourcePacket::json_schema(),
            corpus: source_packet_corpus(),
        },
        Covered {
            file_name: "decision.schema.json",
            emitted: Decision::json_schema(),
            corpus: decision_corpus(),
        },
        Covered {
            file_name: "credential-delivery-observation.schema.json",
            emitted: CredentialDeliveryObservation::json_schema(),
            corpus: credential_delivery_observation_corpus(),
        },
        Covered {
            file_name: "question.schema.json",
            emitted: Question::json_schema(),
            corpus: question_corpus(),
        },
        Covered {
            file_name: "approval-gate.schema.json",
            emitted: ApprovalGate::json_schema(),
            corpus: approval_gate_corpus(),
        },
        Covered {
            file_name: "resolution-response.schema.json",
            emitted: ResolutionResponse::json_schema(),
            corpus: resolution_response_corpus(),
        },
        Covered {
            file_name: "resolution-request.schema.json",
            emitted: ResolutionRequest::json_schema(),
            corpus: resolution_request_corpus(),
        },
        Covered {
            file_name: "act-assignment.schema.json",
            emitted: ActAssignment::json_schema(),
            corpus: act_assignment_corpus(),
        },
        Covered {
            file_name: "authority-subset-proof.schema.json",
            emitted: AuthoritySubsetProof::json_schema(),
            corpus: authority_subset_proof_corpus(),
        },
        Covered {
            file_name: "authority.schema.json",
            emitted: Authority::json_schema(),
            corpus: authority_corpus(),
        },
        Covered {
            file_name: "authority-term.schema.json",
            emitted: AuthorityTerm::json_schema(),
            corpus: authority_term_corpus(),
        },
        Covered {
            file_name: "operational-policy.schema.json",
            emitted: OperationalPolicy::json_schema(),
            corpus: operational_policy_corpus(),
        },
        Covered {
            file_name: "operational-proposal.schema.json",
            emitted: OperationalProposal::json_schema(),
            corpus: operational_proposal_corpus(),
        },
        Covered {
            file_name: "orchestrator-execution-context.schema.json",
            emitted: OrchestratorExecutionContext::json_schema(),
            corpus: orchestrator_execution_context_corpus(),
        },
        Covered {
            file_name: "orchestrator-handoff-context.schema.json",
            emitted: OrchestratorHandoffContext::json_schema(),
            corpus: orchestrator_handoff_context_corpus(),
        },
        Covered {
            file_name: "act.schema.json",
            emitted: Act::json_schema(),
            corpus: act_corpus(),
        },
        Covered {
            file_name: "receipt.schema.json",
            emitted: Receipt::json_schema(),
            corpus: receipt_corpus(),
        },
        Covered {
            file_name: "handoff-signal.schema.json",
            emitted: HandoffSignal::json_schema(),
            corpus: handoff_signal_corpus(),
        },
        Covered {
            file_name: "handoff-state.schema.json",
            emitted: HandoffState::json_schema(),
            corpus: handoff_state_corpus(),
        },
        Covered {
            file_name: "suppression-record.schema.json",
            emitted: SuppressionRecord::json_schema(),
            corpus: suppression_record_corpus(),
        },
        Covered {
            file_name: "packet-index.schema.json",
            emitted: PacketIndex::json_schema(),
            corpus: packet_index_corpus(),
        },
        Covered {
            file_name: "registry-binding.schema.json",
            emitted: RegistryBinding::json_schema(),
            corpus: registry_binding_corpus(),
        },
        Covered {
            file_name: "review-receipt-output.schema.json",
            emitted: ReviewReceiptOutput::json_schema(),
            corpus: review_receipt_output_corpus(),
        },
        Covered {
            file_name: "agent-context-envelope.schema.json",
            emitted: AgentContextEnvelope::json_schema(),
            corpus: agent_context_envelope_corpus(),
        },
        Covered {
            file_name: "agent-act-invocation.schema.json",
            emitted: AgentActInvocation::json_schema(),
            corpus: agent_act_invocation_corpus(),
        },
        Covered {
            file_name: "turn-call.schema.json",
            emitted: TurnCallResult::json_schema(),
            corpus: turn_call_corpus(),
        },
        Covered {
            file_name: "turn-status.schema.json",
            emitted: TurnStatus::json_schema(),
            corpus: turn_status_corpus(),
        },
        Covered {
            file_name: "act-result.schema.json",
            emitted: ActResultEnvelope::json_schema(),
            corpus: act_result_corpus(),
        },
        Covered {
            file_name: "dev.schema.json",
            emitted: DevReport::json_schema(),
            corpus: dev_report_corpus(),
        },
        Covered {
            file_name: "fixture.schema.json",
            emitted: Fixture::json_schema(),
            corpus: fixture_corpus(),
        },
        Covered {
            file_name: "tool-manifest.schema.json",
            emitted: ToolManifest::json_schema(),
            corpus: tool_manifest_corpus(),
        },
        Covered {
            file_name: "list.schema.json",
            emitted: RunxListReport::json_schema(),
            corpus: list_corpus(),
        },
        Covered {
            file_name: "run-summary.schema.json",
            emitted: RunSummary::json_schema(),
            corpus: run_summary_corpus(),
        },
        Covered {
            file_name: "ledger-entry.schema.json",
            emitted: LedgerEntry::json_schema(),
            corpus: ledger_entry_corpus(),
        },
        Covered {
            file_name: "output.schema.json",
            emitted: Output::json_schema(),
            corpus: output_corpus(),
        },
    ]
}
