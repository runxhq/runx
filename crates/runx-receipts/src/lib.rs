//! Pure Rust receipt verification for runx.
//!
//! This crate owns the receipt layer: a receipt is the sealed
//! proof of a run node, with acts and decisions proven through that seal.

mod canonical;
mod identity;
mod notary;
mod tree;
mod verify;
mod wire;

pub use canonical::{
    canonical_receipt_body_digest, canonical_receipt_body_json, canonical_receipt_digest,
    canonical_receipt_json, content_addressed_receipt_id,
};
pub use identity::{SignedDisplayIdentity, signed_display_identity};
pub use notary::{
    CounterSealVerifier, NOTARY_VERIFY_VERDICT_SCHEMA, NotaryCounterSealReport, NotaryFinding,
    NotaryVerifyVerdict, ed25519_public_key_from_spki_pem, verify_notary_document,
};
pub use runx_contracts::{
    RECEIPT_CANONICALIZATION, RECEIPT_SCHEMA, Receipt, ReceiptIssuer, ReceiptIssuerType,
    ReceiptSchema, ReceiptSignature, Seal, SignatureAlgorithm,
};
pub use tree::{
    ReceiptLinks, ReceiptProofContextProvider, ReceiptResolveResult, ReceiptResolver,
    ReceiptTreeConfig, ReceiptTreeGroup, ResolvedReceipt, group_receipt_trees,
    receipt_edge_references, verify_receipt_tree_proof_with_resolver,
};
pub use verify::{
    ReceiptError, ReceiptFinding, ReceiptFindingCode, ReceiptProofContext, ReceiptVerification,
    ReceiptVerifyCheck, ReceiptVerifyFinding, ReceiptVerifyLineageCheck,
    ReceiptVerifySignatureCheck, ReceiptVerifyVerdict, SignatureVerificationFailure,
    SignatureVerifier, VERIFY_VERDICT_SCHEMA, receipt_id_is_content_addressed, verify_receipt,
    verify_receipt_document_verdict, verify_receipt_proof, verify_receipt_verdict,
};
pub use wire::{ReceiptDecodeError, decode_receipt, decode_receipt_value};

// The shared fixture key names this crate as its tests and examples do.
#[cfg(test)]
extern crate self as runx_receipts;

#[cfg(test)]
mod tests {
    #[test]
    fn package_name_matches() {
        assert_eq!(env!("CARGO_PKG_NAME"), "runx-receipts");
    }
}
