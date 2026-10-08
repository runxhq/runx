//! Pure Rust receipt verification for runx.
//!
//! This crate owns the receipt layer: a receipt is the sealed
//! proof of a harness node, with acts and decisions proven through that seal.

mod canonical;
mod identity;
mod tree;
mod verify;

pub use canonical::{
    canonical_receipt_body_digest, canonical_receipt_body_json, canonical_receipt_digest,
    canonical_receipt_identity_json, canonical_receipt_json, content_addressed_receipt_id,
};
pub use identity::{SignedDisplayIdentity, signed_display_identity};
pub use runx_contracts::{
    RECEIPT_CANONICALIZATION, RECEIPT_SCHEMA, Receipt, ReceiptIssuer, ReceiptIssuerType,
    ReceiptSchema, ReceiptSignature, Seal, SignatureAlgorithm,
};
pub use tree::{
    ReceiptProofContextProvider, ReceiptResolveResult, ReceiptResolver, ReceiptTreeConfig,
    ResolvedReceipt, receipt_edge_references, validate_receipt_tree, validate_receipt_tree_proof,
    validate_receipt_tree_proof_with_resolver, validate_receipt_tree_with_resolver,
    verify_receipt_tree, verify_receipt_tree_proof, verify_receipt_tree_proof_with_resolver,
    verify_receipt_tree_with_resolver,
};
pub use verify::{
    ReceiptError, ReceiptFinding, ReceiptFindingCode, ReceiptProofContext,
    ReceiptProofFindingSummary, ReceiptProofStatus, ReceiptProofStatusKind, ReceiptVerification,
    ReceiptVerifyCheck, ReceiptVerifyFinding, ReceiptVerifyLineageCheck,
    ReceiptVerifySignatureCheck, ReceiptVerifySignatureMode, ReceiptVerifyVerdict,
    SignatureVerificationFailure, SignatureVerifier, VERIFY_VERDICT_SCHEMA,
    compute_verification_summary, receipt_id_is_content_addressed, receipt_proof_status,
    validate_receipt, validate_receipt_proof, verify_receipt, verify_receipt_document_verdict,
    verify_receipt_proof, verify_receipt_verdict,
};

#[cfg(test)]
mod tests {
    #[test]
    fn package_name_matches() {
        assert_eq!(env!("CARGO_PKG_NAME"), "runx-receipts");
    }
}
