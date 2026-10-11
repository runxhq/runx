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
    ChildReceiptBreadthLimit,
    DuplicateChildReceipt,
    HashCommitmentInvalid,
    EffectGrantEvidenceMissing,
    SealDigestMismatch,
    ContentAddressMismatch,
    SignatureVerifierMissing,
    SignatureInvalid,
    SignatureKeyMissing,
    SignatureKeyMalformed,
    SignatureKeyHashMismatch,
    SignatureUnsupportedIssuer,
    SignatureUnsupportedAlgorithm,
    SignatureMalformed,
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
