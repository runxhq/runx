//! CLI-backed Rust SDK for runx clients and host protocol helpers.
//!
//! The SDK calls the `runx` CLI with `--json`; it never executes a skill
//! itself.

pub mod act;
pub mod client;
pub mod command;
pub mod error;
pub mod host;

pub use client::{
    ContinuePayload, RunSkillOptions, RunxClient, RunxClientOptions, RunxJsonReport,
    SkillSearchResult,
};
pub use error::{RunxError, RunxResult};
