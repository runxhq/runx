//! Receipts cluster.
//!
//! - `act`: the act as a first-class object, opened with intent and closed with outcome.
//! - `seal`: step and graph receipt sealing helpers.
//! - `store`: the local on-disk receipt store and index.
//! - `tree`: receipt-tree resolution and proof validation.
//! - `paths`: workspace and receipt-store path resolution.
//! - `signing`, `local_key`: the identity receipts are sealed under and the keys a verifier trusts.

pub(crate) mod act;
pub mod archive;
pub mod inspection;
pub mod local_key;
pub mod paths;
pub mod seal;
pub mod signing;
pub mod store;
pub mod tree;

pub(crate) use act::SealedCall;
pub use local_key::{LOCAL_RECEIPT_KEY_SCHEMA, LocalReceiptKeyFault};
pub(crate) use seal::{
    GraphClosure, RuntimeReceiptProofContextProvider, StepReceiptWithDisposition, StepSeal,
    StepSealClosure, graph_receipt_with_closure, seal_step,
    step_receipt_with_declared_claim_and_policy,
};
pub use seal::{
    RunScope, RuntimeReceiptSignaturePolicy, SealAuthority, graph_receipt, step_receipt,
};
pub use signing::{
    Ed25519CounterSealVerifier, Ed25519ReceiptSigner, Ed25519ReceiptVerifier, FIXTURE_RECEIPT_SEED,
    RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_SIGN_KID_ENV, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
    RUNX_RECEIPT_VERIFY_ISSUER_TYPE_ENV, RUNX_RECEIPT_VERIFY_KID_ENV, ReceiptTrust,
    ReceiptTrustOrigin, ReceiptTrustSource, RuntimeReceiptSignatureConfig, RuntimeReceiptSigner,
    RuntimeReceiptSigningError, TrustedReceiptKey, local_receipt_kid, receipt_signer,
    receipt_trust, receipt_verifier,
};
