//! The kernel: what decides, records and protects.
//!
//! - `store`: the protected state store. Append-only streams with expected
//!   versions, atomic multi-stream batches and a hash chain per stream. No
//!   data-store binding, tool or grant can reach it.
//! - `connectors`: the registry of official and plugin-local connectors.
//! - `instances`: what a connector requirement resolves to, with no fallback.
//! - `session`: tokens read from a tool the person is signed into.
//! - `setup`: confirming an instance from its authority source.
//! - `capability`: what an instance's principal can do at the provider.
//! - `admission`: the one evaluator that admits an operation call or projects
//!   a requirement's readiness.
//! - `custody`: effect custody for each act, across runs.
//! - `operation`: operation steps, from admission to the sealed packet.

pub mod admission;
pub mod authority;
pub(crate) mod capability;
pub mod connectors;
pub(crate) mod custody;
pub mod instances;
pub mod operation;
pub(crate) mod session;
pub mod setup;
pub mod store;
pub(crate) mod usage;
