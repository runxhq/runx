use runx_contracts::{ReceiptIssuer, ReceiptSignature};

use super::super::ReceiptFindingCode;

pub trait SignatureVerifier {
    fn verify(
        &self,
        issuer: &ReceiptIssuer,
        signature: &ReceiptSignature,
        body_digest: &str,
    ) -> Result<(), SignatureVerificationFailure>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureVerificationFailure {
    MissingKey,
    MalformedKey,
    KeyDigestMismatch,
    UnsupportedIssuer,
    UnsupportedAlgorithm,
    MalformedSignature,
    SignatureMismatch,
}

pub(super) fn signature_failure_code(error: &SignatureVerificationFailure) -> ReceiptFindingCode {
    match error {
        SignatureVerificationFailure::MissingKey => ReceiptFindingCode::SignatureKeyMissing,
        SignatureVerificationFailure::MalformedKey => ReceiptFindingCode::SignatureKeyMalformed,
        SignatureVerificationFailure::KeyDigestMismatch => {
            ReceiptFindingCode::SignatureKeyDigestMismatch
        }
        SignatureVerificationFailure::UnsupportedIssuer => {
            ReceiptFindingCode::SignatureUnsupportedIssuer
        }
        SignatureVerificationFailure::UnsupportedAlgorithm => {
            ReceiptFindingCode::SignatureUnsupportedAlgorithm
        }
        SignatureVerificationFailure::MalformedSignature => ReceiptFindingCode::SignatureMalformed,
        SignatureVerificationFailure::SignatureMismatch => ReceiptFindingCode::SignatureInvalid,
    }
}

pub(super) fn signature_failure_path(error: &SignatureVerificationFailure) -> &'static str {
    match error {
        SignatureVerificationFailure::MissingKey
        | SignatureVerificationFailure::MalformedKey
        | SignatureVerificationFailure::KeyDigestMismatch
        | SignatureVerificationFailure::UnsupportedIssuer => "issuer",
        SignatureVerificationFailure::UnsupportedAlgorithm => "signature.alg",
        SignatureVerificationFailure::MalformedSignature
        | SignatureVerificationFailure::SignatureMismatch => "signature.value",
    }
}

pub(super) fn signature_failure_message(error: &SignatureVerificationFailure) -> &'static str {
    match error {
        SignatureVerificationFailure::MissingKey => {
            "signature verifier could not resolve the issuer key"
        }
        SignatureVerificationFailure::MalformedKey => {
            "signature verifier resolved malformed issuer key material"
        }
        SignatureVerificationFailure::KeyDigestMismatch => {
            "issuer public key digest does not match the resolved verifier key"
        }
        SignatureVerificationFailure::UnsupportedIssuer => {
            "signature verifier does not support this issuer type"
        }
        SignatureVerificationFailure::UnsupportedAlgorithm => {
            "signature verifier does not support this algorithm"
        }
        SignatureVerificationFailure::MalformedSignature => "signature value is malformed",
        SignatureVerificationFailure::SignatureMismatch => {
            "signature does not verify against the receipt body commitment"
        }
    }
}
