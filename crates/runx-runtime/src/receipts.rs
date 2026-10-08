//! Receipts cluster.
//!
//! - `act`: the act as a first-class object, opened with intent and closed with outcome.
//! - `seal`: step and graph receipt sealing helpers.
//! - `store`: the local on-disk receipt store and index.
//! - `tree`: receipt-tree resolution and proof validation.
//! - `paths`: workspace and receipt-store path resolution.

pub(crate) mod act;
pub mod archive;
pub mod inspection;
pub(crate) mod issuer;
pub mod paths;
pub mod seal;
pub mod signing;
pub mod store;
pub mod tree;

pub(crate) use issuer::local_runtime_issuer;
pub(crate) use seal::{
    DomainActFrame, DomainActReceiptRequest, GraphClosure, RuntimeReceiptProofContextProvider,
    StepReceiptWithDisposition, StepSeal, StepSealClosure, domain_act_receipt,
    graph_receipt_with_closure, seal_step, step_receipt_with_declared_claim_and_policy,
};
pub use seal::{
    RunScope, RuntimeReceiptSignaturePolicy, SealAuthority, graph_receipt,
    graph_receipt_with_signature_policy, step_receipt, step_receipt_with_authority_grant_refs,
    step_receipt_with_signature_policy,
};
pub use signing::{
    Ed25519ReceiptSigner, Ed25519ReceiptVerifier, ProductionReceiptKey,
    RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64_ENV, RUNX_RECEIPT_SIGN_ISSUER_TYPE_ENV,
    RUNX_RECEIPT_SIGN_KID_ENV, RUNX_RECEIPT_VERIFY_ED25519_PUBLIC_KEY_BASE64_ENV,
    RUNX_RECEIPT_VERIFY_KID_ENV, ResolvedReceiptVerifier, RuntimeReceiptSignatureConfig,
    RuntimeReceiptSigner, RuntimeReceiptSigningError, RuntimeReceiptVerifierSource,
    receipt_verifier_from_env,
};
