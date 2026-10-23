use thiserror::Error;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ReceiptError {
    #[error("receipt serialization failed: {message}")]
    Serialization { message: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiptFindingCode {
    EmptyEnvelopeField,
    DecisionSelectedActMissing,
    ActLegitimacyTermMissing,
    SealCriterionUnbound,
    ChildReceiptRefInvalid,
    ChildReceiptRefMalformed,
    ChildReceiptMissing,
    ChildReceiptAmbiguous,
    ChildReceiptResolverError,
    ChildReceiptCycle,
    OrphanChildReceipt,
    ChildReceiptDigestMismatch,
    ChildReceiptDepthLimit,
    ChildReceiptCountLimit,
    ChildReceiptShared,
    DuplicateChildReceipt,
    HashCommitmentInvalid,
    EffectGrantEvidenceMissing,
    SealDigestMismatch,
    ContentAddressMismatch,
    SignatureVerifierMissing,
    SignatureInvalid,
    SignatureKeyMissing,
    SignatureKeyMalformed,
    SignatureKeyDigestMismatch,
    SignatureUnsupportedIssuer,
    SignatureUnsupportedAlgorithm,
    SignatureMalformed,
}

impl ReceiptFindingCode {
    /// The code's one public name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EmptyEnvelopeField => "empty_envelope_field",
            Self::DecisionSelectedActMissing => "decision_selected_act_missing",
            Self::ActLegitimacyTermMissing => "act_legitimacy_term_missing",
            Self::SealCriterionUnbound => "seal_criterion_unbound",
            Self::ChildReceiptRefInvalid => "child_receipt_ref_invalid",
            Self::ChildReceiptRefMalformed => "child_receipt_ref_malformed",
            Self::ChildReceiptMissing => "child_receipt_missing",
            Self::ChildReceiptAmbiguous => "child_receipt_ambiguous",
            Self::ChildReceiptResolverError => "child_receipt_resolver_error",
            Self::ChildReceiptCycle => "child_receipt_cycle",
            Self::OrphanChildReceipt => "orphan_child_receipt",
            Self::ChildReceiptDigestMismatch => "child_receipt_digest_mismatch",
            Self::ChildReceiptDepthLimit => "child_receipt_depth_limit",
            Self::ChildReceiptCountLimit => "child_receipt_count_limit",
            Self::ChildReceiptShared => "child_receipt_shared",
            Self::DuplicateChildReceipt => "duplicate_child_receipt",
            Self::HashCommitmentInvalid => "hash_commitment_invalid",
            Self::EffectGrantEvidenceMissing => "effect_grant_evidence_missing",
            Self::SealDigestMismatch => "seal_digest_mismatch",
            Self::ContentAddressMismatch => "content_address_mismatch",
            Self::SignatureVerifierMissing => "signature_verifier_missing",
            Self::SignatureInvalid => "signature_invalid",
            Self::SignatureKeyMissing => "signature_key_missing",
            Self::SignatureKeyMalformed => "signature_key_malformed",
            Self::SignatureKeyDigestMismatch => "signature_key_digest_mismatch",
            Self::SignatureUnsupportedIssuer => "signature_unsupported_issuer",
            Self::SignatureUnsupportedAlgorithm => "signature_unsupported_algorithm",
            Self::SignatureMalformed => "signature_malformed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptFinding {
    pub code: ReceiptFindingCode,
    pub path: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceiptVerification {
    pub valid: bool,
    pub findings: Vec<ReceiptFinding>,
}

impl ReceiptVerification {
    #[must_use]
    pub fn valid() -> Self {
        Self {
            valid: true,
            findings: Vec::new(),
        }
    }

    #[must_use]
    pub fn from_findings(findings: Vec<ReceiptFinding>) -> Self {
        Self {
            valid: findings.is_empty(),
            findings,
        }
    }
}
