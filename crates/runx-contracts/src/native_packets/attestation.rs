//! What the native attestation tool returns.

use serde::{Deserialize, Serialize};

use crate::schema::{BoundedString, BoundedVec, False, NonEmptyString, RunxSchema};
use crate::{Digest, JsonObject};

/// The longest action, in bytes, an attestation states.
pub const ATTESTATION_MAX_ACTION_BYTES: usize = 2_000;
/// The longest claim, in bytes, an attestation binds.
pub const ATTESTATION_MAX_CLAIM_BYTES: usize = 4_000;
/// The longest principal, in bytes, an attestation names.
pub const ATTESTATION_MAX_PRINCIPAL_BYTES: usize = 512;
/// The most evidence items one attestation carries.
pub const ATTESTATION_MAX_EVIDENCE: usize = 100;
/// The longest evidence locator, in bytes.
pub const ATTESTATION_MAX_LOCATOR_BYTES: usize = 512;
/// The longest statement, in bytes, of what one item proves.
pub const ATTESTATION_MAX_PROVES_BYTES: usize = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum AttestationSchema {
    #[serde(rename = "runx.attestation.v1")]
    V1,
}

/// Whether the attestation can be sealed, or what it waits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttestationDecision {
    ReadyToSeal,
    NeedsMoreEvidence,
    /// The action, principal or claim is missing.
    NeedsAgent,
}

/// What `receipt.attest` returns: one claim about an action outside runx,
/// bound to opaque evidence and digested for the parent receipt to seal. A
/// field the tool could not admit is null and a gap says why; a
/// `ready_to_seal` attestation has no gaps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.attestation.v1")]
pub struct Attestation {
    pub schema: AttestationSchema,
    pub decision: AttestationDecision,
    pub action: Option<BoundedString<ATTESTATION_MAX_ACTION_BYTES>>,
    pub claim: Option<BoundedString<ATTESTATION_MAX_CLAIM_BYTES>>,
    pub principal: Option<BoundedString<ATTESTATION_MAX_PRINCIPAL_BYTES>>,
    pub evidence: BoundedVec<AttestationEvidence, 0, ATTESTATION_MAX_EVIDENCE>,
    /// Who or what may rely on the claim.
    pub scope: JsonObject,
    /// The digest of the action, claim, principal, evidence and scope.
    pub attestation_digest: Digest,
    pub gaps: Vec<AttestationGap>,
    pub proof_boundary: AttestationProofBoundary,
}

/// One opaque evidence locator, its digest, and what it is asserted to prove.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AttestationEvidence {
    pub locator: Option<BoundedString<ATTESTATION_MAX_LOCATOR_BYTES>>,
    pub digest: Option<Digest>,
    pub proves: Option<BoundedString<ATTESTATION_MAX_PROVES_BYTES>>,
}

/// Why an attestation is not ready to seal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AttestationGap {
    pub code: NonEmptyString,
    pub message: NonEmptyString,
}

/// What an attestation does not prove: runx called no provider, appended to
/// no external ledger, and verified nothing outside itself. The parent
/// receipt's seal is the only signature evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct AttestationProofBoundary {
    pub external_action_verified: False,
    pub provider_status: ProviderNotCalled,
    pub external_ledger_status: ExternalLedgerNotRequested,
    pub runtime_seal_status: RuntimeSealPending,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ProviderNotCalled {
    #[default]
    #[serde(rename = "not_called")]
    NotCalled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum ExternalLedgerNotRequested {
    #[default]
    #[serde(rename = "not_requested")]
    NotRequested,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum RuntimeSealPending {
    #[default]
    #[serde(rename = "pending_parent_receipt")]
    PendingParentReceipt,
}
