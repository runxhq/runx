use runx_contracts::{Receipt, SignatureAlgorithm};

use crate::{canonical_receipt_body_digest, content_addressed_receipt_id};

use super::{ReceiptFinding, ReceiptFindingCode, ReceiptVerification, verify_receipt};

mod signature;

pub use signature::{SignatureVerificationFailure, SignatureVerifier};
use signature::{signature_failure_code, signature_failure_message, signature_failure_path};

pub fn validate_receipt_proof(
    receipt: &Receipt,
    context: &ReceiptProofContext<'_>,
) -> Result<(), ReceiptVerification> {
    let verification = verify_receipt_proof(receipt, context);
    if verification.valid {
        Ok(())
    } else {
        Err(verification)
    }
}

/// Read-time proof verification. Computes signature and digest validity on
/// top of the structural checks (which include the inline `selected_act_id`
/// integrity property against `acts[]`).
#[must_use]
pub fn verify_receipt_proof(
    receipt: &Receipt,
    context: &ReceiptProofContext<'_>,
) -> ReceiptVerification {
    let mut findings = verify_receipt(receipt).findings;
    let mut verifier = ProofVerifier {
        context,
        findings: Vec::new(),
    };
    verifier.check_body_proof(receipt);
    findings.extend(verifier.findings);
    ReceiptVerification::from_findings(findings)
}

#[derive(Default)]
pub struct ReceiptProofContext<'a> {
    pub signature_verifier: Option<&'a dyn SignatureVerifier>,
}

struct ProofVerifier<'a> {
    context: &'a ReceiptProofContext<'a>,
    findings: Vec<ReceiptFinding>,
}

/// Whether `receipt.id` equals its content address `hash(canonical_body)` under
/// `runx.receipt.c14n.v1`. Proof verification requires it: an id is what
/// every other receipt and store names a receipt by.
#[must_use]
pub fn receipt_id_is_content_addressed(receipt: &Receipt) -> bool {
    content_addressed_receipt_id(receipt).is_ok_and(|content_id| receipt.id == content_id)
}

impl ProofVerifier<'_> {
    fn check_body_proof(&mut self, receipt: &Receipt) {
        let Ok(body_digest) = canonical_receipt_body_digest(receipt) else {
            self.push(
                ReceiptFindingCode::SealDigestMismatch,
                "digest",
                "receipt body digest could not be recomputed",
            );
            return;
        };
        self.check_body_digest(receipt, &body_digest);
        self.check_content_address(receipt);
        self.check_signature(receipt, &body_digest);
    }

    fn check_content_address(&mut self, receipt: &Receipt) {
        if !receipt_id_is_content_addressed(receipt) {
            self.push(
                ReceiptFindingCode::ContentAddressMismatch,
                "id",
                "receipt id must be the content address of its canonical body",
            );
        }
    }

    fn check_body_digest(&mut self, receipt: &Receipt, body_digest: &str) {
        if receipt.digest != body_digest {
            self.push(
                ReceiptFindingCode::SealDigestMismatch,
                "digest",
                "receipt digest must match the canonical receipt body commitment",
            );
        }
    }

    fn check_signature(&mut self, receipt: &Receipt, body_digest: &str) {
        match self.context.signature_verifier {
            Some(verifier) => {
                if receipt.signature.alg != SignatureAlgorithm::Ed25519 {
                    self.push(
                        ReceiptFindingCode::SignatureUnsupportedAlgorithm,
                        "signature.alg",
                        "unsupported receipt signature algorithm",
                    );
                    return;
                }
                if let Err(error) =
                    verifier.verify(&receipt.issuer, &receipt.signature, body_digest)
                {
                    self.push(
                        signature_failure_code(&error),
                        signature_failure_path(&error),
                        signature_failure_message(&error),
                    );
                }
            }
            None => self.push(
                ReceiptFindingCode::SignatureVerifierMissing,
                "signature",
                "strict receipt proof verification requires an injected signature verifier",
            ),
        }
    }

    fn push(
        &mut self,
        code: ReceiptFindingCode,
        path: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.findings.push(ReceiptFinding {
            code,
            path: path.into(),
            message: message.into(),
        });
    }
}
