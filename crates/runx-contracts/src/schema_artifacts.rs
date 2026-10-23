//! Published JSON Schema artifact manifest.
//!
//! This is the authoritative Rust-side list consumed by the schema generation
//! gate. Keep filenames aligned with `oss/schemas/*.json`.
use serde_json as schema_json;

use crate::schema::{NamedSchemas, RunxSchema};
use crate::{
    ActAssignment, ActResultEnvelope, AgentActInvocation, AgentContextEnvelope,
    ApprovalDecisionPacket, ApprovalGate, Artifact, Attestation, AuthorityDerivation,
    AuthoritySubsetProof, AuthorityTerm, CancelPurchaseRequest, CancelPurchaseResult, CliHelp,
    CommandExecution, CommandPlan, ConnectorInstance, ConnectorManifest, ConversationObservation,
    ConversationPage, CredentialDeliveryObservation, DataComparison, DataDigest,
    DataOperationResult, DataValidation, Decision, DecisionObservation, DoctorReport,
    ExecutePurchaseRequest, ExecutePurchaseResult, ExternalJobContinuation, ExternalJobSchedule,
    ExternalJobScheduleIntent, ExternalJobStageRequest, ExternalJobStageResult, FileApplyBundle,
    FileEdit, FileInverseBundle, FileList, FileRead, FileReadBundle, FileSearch, FileWrite,
    Fixture, GetPurchaseRequest, GetPurchaseResult, GitBlobDigest, GitBranch, GitDiff, GitLog,
    GitPatch, GitShow, GitStatus, HarnessContext, LedgerEntry, LocalArtifact, LocalArtifactPage,
    OfferVersionBinding, OperationalPolicy, OperationalProposal, OrchestratorExecutionContext,
    OrchestratorHandoffContext, Output, PacketLock, PaidSkillListing, PolicyLint, ProcessList,
    ProcessOutput, ProviderOperationPacket, Purchase, PurchaseClaim, PurchaseRead, Question,
    QuotePurchaseRequest, QuotePurchaseResult, Readiness, Receipt, ReceiptProof, ReceiptQuery,
    Reference, ReferenceLink, RegistryBinding, RegistryInspection, RegistrySearchPacket,
    ResolutionRequest, ResolutionResponse, RunList, RunxListReport, RunxProcess,
    RunxX402PurchaseExtensionInfo, Signal, SkillApplyResult, SkillArchitectureDecision,
    SkillArchitecturePlan, SkillAuthoringContext, SkillChangeBundle, SkillChangeDraft,
    SkillInspection, SkillValidation, SkillValidationResult, SourcePacket, ToolManifest,
    TurnCallResult, TurnLoad, TurnReadPage, TurnStatus, WorkOutcome, X402PaymentPayload,
    X402PaymentRequired, X402PaymentRequirements, X402ResourceInfo, X402SettleResponse,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SchemaArtifact {
    pub file_name: &'static str,
    /// The Rust type the schema is generated from, without its module path:
    /// the name generated bindings give the contract.
    pub type_name: &'static str,
    pub schema: schema_json::Value,
    /// Each named type the schema reaches, with its own schema.
    pub named_schemas: NamedSchemas,
    /// True when the generated Rust schema is the complete public packet
    /// contract and may be embedded by the native runtime.
    pub native_packet: bool,
}

impl SchemaArtifact {
    /// The exact bytes published as `schemas/<file_name>`. A core packet's
    /// digest is the digest of these bytes, so a sealed packet check names
    /// a schema anyone can fetch and hash.
    pub fn document(&self) -> Result<String, schema_json::Error> {
        schema_json::to_string_pretty(&self.schema).map(|document| format!("{document}\n"))
    }

    /// The core packet id a native packet artifact is published under.
    #[must_use]
    pub fn packet_id(&self) -> Option<crate::PacketId> {
        if !self.native_packet {
            return None;
        }
        self.schema
            .get("x-runx-schema")
            .and_then(schema_json::Value::as_str)
            .and_then(crate::PacketId::new)
    }
}

#[must_use]
// Function rationale: the artifact manifest is deliberately one
// ordered list so the published schema set is auditable against `oss/schemas`.
pub fn generated_schema_artifacts() -> Vec<SchemaArtifact> {
    vec![
        schema_artifact::<Output>("output.schema.json"),
        schema_artifact::<crate::SkillOutput>("skill-output.schema.json"),
        schema_artifact::<crate::ProjectArtifact>("project-artifact.schema.json"),
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
        schema_artifact::<TurnLoad>("turn-load.schema.json"),
        schema_artifact::<TurnReadPage>("turn-read.schema.json"),
        schema_artifact::<SkillInspection>("skill-inspection.schema.json"),
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
        public_packet_artifact::<SkillArchitectureDecision>(
            "skill-architecture-decision.schema.json",
        ),
        public_packet_artifact::<SkillArchitecturePlan>("skill-architecture-plan.schema.json"),
        public_packet_artifact::<SkillChangeDraft>("skill-change-draft.schema.json"),
        public_packet_artifact::<SkillChangeBundle>("skill-change-bundle.schema.json"),
        public_packet_artifact::<SkillValidationResult>("skill-validation-result.schema.json"),
        public_packet_artifact::<SkillApplyResult>("skill-apply-result.schema.json"),
        schema_artifact::<Fixture>("fixture.schema.json"),
        schema_artifact::<ToolManifest>("tool-manifest.schema.json"),
        schema_artifact::<PacketLock>("packet-lock.schema.json"),
        public_packet_artifact::<LocalArtifact>("local-artifact.schema.json"),
        public_packet_artifact::<LocalArtifactPage>("local-artifact-page.schema.json"),
        public_packet_artifact::<GitBlobDigest>("git-blob-digest.schema.json"),
        public_packet_artifact::<GitBranch>("git-branch.schema.json"),
        public_packet_artifact::<GitStatus>("git-status.schema.json"),
        public_packet_artifact::<GitDiff>("git-diff.schema.json"),
        public_packet_artifact::<GitLog>("git-log.schema.json"),
        public_packet_artifact::<GitShow>("git-show.schema.json"),
        public_packet_artifact::<GitPatch>("git-patch.schema.json"),
        public_packet_artifact::<DataOperationResult>("data-operation-result.schema.json"),
        public_packet_artifact::<DataValidation>("data-validation.schema.json"),
        public_packet_artifact::<DataDigest>("data-digest.schema.json"),
        public_packet_artifact::<DataComparison>("data-comparison.schema.json"),
        public_packet_artifact::<HarnessContext>("harness-context.schema.json"),
        public_packet_artifact::<FileRead>("file-read.schema.json"),
        public_packet_artifact::<FileReadBundle>("file-read-bundle.schema.json"),
        public_packet_artifact::<FileWrite>("file-write.schema.json"),
        public_packet_artifact::<FileEdit>("file-edit.schema.json"),
        public_packet_artifact::<FileApplyBundle>("file-apply-bundle.schema.json"),
        public_packet_artifact::<FileList>("file-list.schema.json"),
        public_packet_artifact::<FileSearch>("file-search.schema.json"),
        public_packet_artifact::<FileInverseBundle>("file-inverse-bundle.schema.json"),
        public_packet_artifact::<CommandPlan>("command-plan.schema.json"),
        public_packet_artifact::<CommandExecution>("command-execution.schema.json"),
        public_packet_artifact::<CliHelp>("cli-help.schema.json"),
        public_packet_artifact::<ReceiptQuery>("receipt-query.schema.json"),
        public_packet_artifact::<ReceiptProof>("receipt-proof.schema.json"),
        public_packet_artifact::<Attestation>("attestation.schema.json"),
        public_packet_artifact::<RunList>("run-list.schema.json"),
        public_packet_artifact::<PolicyLint>("policy-lint.schema.json"),
        public_packet_artifact::<SkillValidation>("skill-validation.schema.json"),
        public_packet_artifact::<SkillAuthoringContext>("skill-authoring-context.schema.json"),
        public_packet_artifact::<ApprovalDecisionPacket>("approval-decision.schema.json"),
        public_packet_artifact::<RunxProcess>("process.schema.json"),
        public_packet_artifact::<ProcessOutput>("process-output.schema.json"),
        public_packet_artifact::<ProcessList>("process-list.schema.json"),
        public_packet_artifact::<RegistrySearchPacket>("registry-search.schema.json"),
        public_packet_artifact::<RegistryInspection>("registry-inspection.schema.json"),
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
        schema_artifact::<LedgerEntry>("ledger-entry.schema.json"),
        public_packet_artifact::<OperationalPolicy>("operational-policy.schema.json"),
        schema_artifact::<OperationalProposal>("operational-proposal.schema.json"),
        public_packet_artifact::<OrchestratorExecutionContext>(
            "orchestrator-execution-context.schema.json",
        ),
        public_packet_artifact::<OrchestratorHandoffContext>(
            "orchestrator-handoff-context.schema.json",
        ),
        schema_artifact::<RegistryBinding>("registry-binding.schema.json"),
        public_packet_artifact::<ProviderOperationPacket>("provider-operation.schema.json"),
        public_packet_artifact::<ExternalJobContinuation>("external-job-continuation.schema.json"),
        public_packet_artifact::<ExternalJobSchedule>("external-job-schedule.schema.json"),
        public_packet_artifact::<ExternalJobScheduleIntent>(
            "external-job-schedule-intent.schema.json",
        ),
        public_packet_artifact::<ExternalJobStageRequest>("external-job-stage-request.schema.json"),
        public_packet_artifact::<ExternalJobStageResult>("external-job-stage-result.schema.json"),
        public_packet_artifact::<Purchase>("purchase.schema.json"),
        public_packet_artifact::<PaidSkillListing>("paid-skill-listing.schema.json"),
        public_packet_artifact::<OfferVersionBinding>("offer-version-binding.schema.json"),
        public_packet_artifact::<QuotePurchaseRequest>("quote-purchase-request.schema.json"),
        public_packet_artifact::<QuotePurchaseResult>("quote-purchase-result.schema.json"),
        public_packet_artifact::<ExecutePurchaseRequest>("execute-purchase-request.schema.json"),
        public_packet_artifact::<ExecutePurchaseResult>("execute-purchase-result.schema.json"),
        public_packet_artifact::<GetPurchaseRequest>("get-purchase-request.schema.json"),
        public_packet_artifact::<GetPurchaseResult>("get-purchase-result.schema.json"),
        public_packet_artifact::<PurchaseRead>("purchase-read.schema.json"),
        public_packet_artifact::<PurchaseClaim>("purchase-claim.schema.json"),
        public_packet_artifact::<CancelPurchaseRequest>("cancel-purchase-request.schema.json"),
        public_packet_artifact::<CancelPurchaseResult>("cancel-purchase-result.schema.json"),
        schema_artifact::<X402ResourceInfo>("x402-v2-resource-info.schema.json"),
        schema_artifact::<X402PaymentRequirements>("x402-v2-payment-requirements.schema.json"),
        schema_artifact::<X402PaymentRequired>("x402-v2-payment-required.schema.json"),
        schema_artifact::<X402PaymentPayload>("x402-v2-payment-payload.schema.json"),
        schema_artifact::<X402SettleResponse>("x402-v2-settle-response.schema.json"),
        schema_artifact::<RunxX402PurchaseExtensionInfo>(
            "runx-x402-purchase-extension-v1.schema.json",
        ),
    ]
}

pub fn schema_artifact<T: RunxSchema>(file_name: &'static str) -> SchemaArtifact {
    let type_name = std::any::type_name::<T>();
    let mut named_schemas = NamedSchemas::new();
    T::named_schemas(&mut named_schemas);
    SchemaArtifact {
        file_name,
        type_name: type_name.rsplit("::").next().unwrap_or(type_name),
        schema: T::json_schema(),
        named_schemas,
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

#[cfg(test)]
mod tests {
    use super::generated_schema_artifacts;

    #[test]
    fn every_native_packet_has_a_core_id_and_its_canonical_url() {
        for artifact in generated_schema_artifacts() {
            if !artifact.native_packet {
                continue;
            }
            let id = artifact.packet_id();
            assert!(
                id.as_ref().is_some_and(crate::PacketId::is_core),
                "{} has no core packet id",
                artifact.file_name
            );
            assert_eq!(
                artifact
                    .schema
                    .get("$id")
                    .and_then(serde_json::Value::as_str),
                id.map(|id| id.schema_url()).as_deref(),
                "{} $id",
                artifact.file_name
            );
        }
    }
}
