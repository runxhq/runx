//! The pure Rust kernel for runx decisions.
//!
//! This crate owns the pure Rust decision domains. Keep IO, adapter, runtime,
//! and CLI presentation concerns outside this boundary.

pub mod kernel_eval;
pub mod policy;
pub mod serde_conventions;
pub mod state_machine;
