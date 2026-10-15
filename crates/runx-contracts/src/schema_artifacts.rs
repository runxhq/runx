//! Published JSON Schema artifact manifest.
//!
//! This is the authoritative Rust-side list consumed by the schema generation
//! gate. Keep filenames aligned with `oss/schemas/*.json`.
use serde_json as schema_json;

use crate::schema::RunxSchema;
use crate::{
    ActAssignment, ActResultEnvelope, AgentActInvocation, AgentContextEnvelope,
    ApprovalDecisionPacket, ApprovalGate, Artifact, AuthorityDerivation, AuthoritySubsetProof,
    AuthorityTerm, CancelPaidInvocationRequest, CancelPaidInvocationResult, ConnectorInstance,
    ConnectorManifest, ConversationObservation, ConversationPage, CredentialDeliveryObservation,
    DataOperationResult, Decision, DecisionObservation, DoctorReport, ExecutePaidInvocationRequest,
    ExecutePaidInvocationResult, ExternalJobContinuation, ExternalJobSchedule,
    ExternalJobScheduleIntent, ExternalJobStageRequest, ExternalJobStageResult, Fixture,
    GetPaidInvocationRequest, GetPaidInvocationResult, GitBlobDigest, HandoffSignal, HandoffState,
    LedgerEntry, LocalArtifact, LocalArtifactPage, OfferRevisionRef, OperationalPolicy,
    OperationalProposal, OrchestratorExecutionContext, OrchestratorHandoffContext, Output,
    PacketIndex, PaidInvocation, PaidSkillListing, ProcessList, ProcessOutput,
    ProviderOperationPacket, Question, QuotePaidInvocationRequest, QuotePaidInvocationResult,
    Readiness, Receipt, Redaction, Reference, ReferenceLink, RegistryBinding, ResolutionRequest,
    ResolutionResponse, ReviewReceiptOutput, RunxListReport, RunxProcess,
    RunxX402InvocationExtensionInfo, Signal, SkillApplyResult,
    SkillArchitectureDecision, SkillArchitecturePlan, SkillChangeBundle, SkillChangeDraft,
    SkillValidationResult, SourcePacket, SuppressionRecord, ToolManifest, TurnCallResult,
    TurnStatus, WorkOutcome, X402PaymentPayload, X402PaymentRequired, X402PaymentRequirements,
    X402ResourceInfo, X402SettleResponse,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaArtifact {
    pub file_name: &'static str,
    pub schema: schema_json::Value,
    /// True when the generated Rust schema is the complete public packet
    /// contract and may be embedded by the native runtime.
    pub native_packet: bool,
}

#[must_use]
// Function rationale: the artifact manifest is deliberately one
// ordered list so the published schema set is auditable against `oss/schemas`.
pub fn generated_schema_artifacts() -> Vec<SchemaArtifact> {
    vec![
        schema_artifact::<Output>("output.schema.json"),
        schema_artifact::<crate::SkillOutput>("skill-output.schema.json"),
        schema_artifact::<crate::ProjectArtifactRef>("project-artifact-ref.schema.json"),
        public_packet_artifact::<WorkOutcome>("work-outcome.schema.json"),
        public_packet_artifact::<DecisionObservation>("decision-observation.schema.json"),
        schema_artifact::<ConnectorManifest>("connector.schema.json"),
        schema_artifact::<crate::connector::ConnectorBinding>("connector-binding.schema.json"),
        schema_artifact::<ConnectorInstance>("connector-instance.schema.json"),
        public_packet_artifact::<Readiness>("readiness.schema.json"),
        schema_artifact::<AuthorityDerivation>("authority-derivation.schema.json"),
        schema_artifact::<AgentContextEnvelope>("agent-context-envelope.schema.json"),
        schema_artifact::<AgentActInvocation>("agent-act-invocation.schema.json"),
        schema_artifact::<TurnCallResult>("turn-call.schema.json"),
        schema_artifact::<TurnStatus>("turn-status.schema.json"),
        schema_artifact::<crate::RunRead>("run-read.schema.json"),
        schema_artifact::<crate::ResumeDocument>("resume.schema.json"),
        schema_artifact::<crate::RunEventRecord>("run-event.schema.json"),
        schema_artifact::<Question>("question.schema.json"),
        schema_artifact::<ApprovalGate>("approval-gate.schema.json"),
        schema_artifact::<ResolutionRequest>("resolution-request.schema.json"),
        schema_artifact::<ResolutionResponse>("resolution-response.schema.json"),
        schema_artifact::<ActResultEnvelope>("act-result.schema.json"),
        schema_artifact::<CredentialDeliveryObservation>(
            "credential-delivery-observation.schema.json",
        ),
        schema_artifact::<DoctorReport>("doctor.schema.json"),
        schema_artifact::<RunxListReport>("list.schema.json"),
        schema_artifact::<SkillArchitectureDecision>("skill-architecture-decision.schema.json"),
        public_packet_artifact::<SkillArchitecturePlan>("skill-architecture-plan.schema.json"),
        schema_artifact::<SkillChangeDraft>("skill-change-draft.schema.json"),
        public_packet_artifact::<SkillChangeBundle>("skill-change-bundle.schema.json"),
        public_packet_artifact::<SkillValidationResult>("skill-validation-result.schema.json"),
        public_packet_artifact::<SkillApplyResult>("skill-apply-result.schema.json"),
        schema_artifact::<Fixture>("fixture.schema.json"),
        schema_artifact::<ToolManifest>("tool-manifest.schema.json"),
        schema_artifact::<PacketIndex>("packet-index.schema.json"),
        public_packet_artifact::<LocalArtifact>("local-artifact.schema.json"),
        public_packet_artifact::<LocalArtifactPage>("local-artifact-page.schema.json"),
        public_packet_artifact::<GitBlobDigest>("git-blob-digest.schema.json"),
        public_packet_artifact::<DataOperationResult>("data-operation-result.schema.json"),
        public_packet_artifact::<ApprovalDecisionPacket>("approval-decision.schema.json"),
        public_packet_artifact::<RunxProcess>("process.schema.json"),
        public_packet_artifact::<ProcessOutput>("process-output.schema.json"),
        public_packet_artifact::<ProcessList>("process-list.schema.json"),
        schema_artifact::<ActAssignment>("act-assignment.schema.json"),
        schema_artifact::<Reference>("reference.schema.json"),
        schema_artifact::<ReferenceLink>("reference-link.schema.json"),
        public_packet_artifact::<AuthorityTerm>("authority-term.schema.json"),
        schema_artifact::<AuthoritySubsetProof>("authority-subset-proof.schema.json"),
        public_packet_artifact::<Signal>("signal.schema.json"),
        public_packet_artifact::<SourcePacket>("source-packet.schema.json"),
        public_packet_artifact::<ConversationPage>("conversation-page.schema.json"),
        public_packet_artifact::<ConversationObservation>("conversation-observation.schema.json"),
        public_packet_artifact::<Decision>("decision.schema.json"),
        public_packet_artifact::<Receipt>("receipt.schema.json"),
        schema_artifact::<Artifact>("artifact.schema.json"),
        schema_artifact::<Redaction>("redaction.schema.json"),
        schema_artifact::<LedgerEntry>("ledger-entry.schema.json"),
        schema_artifact::<HandoffSignal>("handoff-signal.schema.json"),
        schema_artifact::<HandoffState>("handoff-state.schema.json"),
        schema_artifact::<SuppressionRecord>("suppression-record.schema.json"),
        public_packet_artifact::<OperationalPolicy>("operational-policy.schema.json"),
        schema_artifact::<OperationalProposal>("operational-proposal.schema.json"),
        public_packet_artifact::<OrchestratorExecutionContext>(
            "orchestrator-execution-context.schema.json",
        ),
        public_packet_artifact::<OrchestratorHandoffContext>(
            "orchestrator-handoff-context.schema.json",
        ),
        schema_artifact::<RegistryBinding>("registry-binding.schema.json"),
        public_packet_artifact_with_external_contract::<ProviderOperationPacket>(
            "provider-operation.schema.json",
        ),
        public_packet_artifact::<ExternalJobContinuation>("external-job-continuation.schema.json"),
        public_packet_artifact::<ExternalJobSchedule>("external-job-schedule.schema.json"),
        public_packet_artifact::<ExternalJobScheduleIntent>(
            "external-job-schedule-intent.schema.json",
        ),
        public_packet_artifact::<ExternalJobStageRequest>("external-job-stage-request.schema.json"),
        public_packet_artifact::<ExternalJobStageResult>("external-job-stage-result.schema.json"),
        schema_artifact::<ReviewReceiptOutput>("review-receipt-output.schema.json"),
        public_packet_artifact::<PaidInvocation>("paid-invocation.schema.json"),
        public_packet_artifact::<PaidSkillListing>("paid-skill-listing.schema.json"),
        public_packet_artifact::<OfferRevisionRef>("offer-revision-ref.schema.json"),
        public_packet_artifact::<QuotePaidInvocationRequest>(
            "quote-paid-invocation-request.schema.json",
        ),
        public_packet_artifact::<QuotePaidInvocationResult>(
            "quote-paid-invocation-result.schema.json",
        ),
        public_packet_artifact::<ExecutePaidInvocationRequest>(
            "execute-paid-invocation-request.schema.json",
        ),
        public_packet_artifact::<ExecutePaidInvocationResult>(
            "execute-paid-invocation-result.schema.json",
        ),
        public_packet_artifact::<GetPaidInvocationRequest>(
            "get-paid-invocation-request.schema.json",
        ),
        public_packet_artifact::<GetPaidInvocationResult>("get-paid-invocation-result.schema.json"),
        public_packet_artifact::<CancelPaidInvocationRequest>(
            "cancel-paid-invocation-request.schema.json",
        ),
        public_packet_artifact::<CancelPaidInvocationResult>(
            "cancel-paid-invocation-result.schema.json",
        ),
        schema_artifact::<X402ResourceInfo>("x402-v2-resource-info.schema.json"),
        schema_artifact::<X402PaymentRequirements>("x402-v2-payment-requirements.schema.json"),
        schema_artifact::<X402PaymentRequired>("x402-v2-payment-required.schema.json"),
        schema_artifact::<X402PaymentPayload>("x402-v2-payment-payload.schema.json"),
        schema_artifact::<X402SettleResponse>("x402-v2-settle-response.schema.json"),
        schema_artifact::<RunxX402InvocationExtensionInfo>(
            "runx-x402-invocation-extension-v1.schema.json",
        ),
    ]
}

pub fn schema_artifact<T: RunxSchema>(file_name: &'static str) -> SchemaArtifact {
    SchemaArtifact {
        file_name,
        schema: T::json_schema(),
        native_packet: false,
    }
}

/// Mark a reusable, named boundary contract for packet distribution even when
/// its immediate producer is native rather than a skill manifest. Runner-local
/// input and output shapes must stay in `run.yaml` and use [`schema_artifact`]
/// instead.
pub fn public_packet_artifact<T: RunxSchema>(file_name: &'static str) -> SchemaArtifact {
    let mut artifact = schema_artifact::<T>(file_name);
    artifact.native_packet = true;
    if let schema_json::Value::Object(schema) = &mut artifact.schema {
        schema.insert("x-runx-packet".to_owned(), schema_json::Value::Bool(true));
    } else {
        let schema = std::mem::replace(&mut artifact.schema, schema_json::Value::Null);
        artifact.schema = schema_json::json!({
            "allOf": [schema],
            "x-runx-packet": true,
        });
    }
    artifact
}

/// Mark a public packet whose checked-in manual schema is deliberately
/// stronger than the generated Rust projection. It remains distributable but
/// cannot be embedded as though the generated projection were authoritative.
fn public_packet_artifact_with_external_contract<T: RunxSchema>(
    file_name: &'static str,
) -> SchemaArtifact {
    let mut artifact = public_packet_artifact::<T>(file_name);
    artifact.native_packet = false;
    artifact
}
