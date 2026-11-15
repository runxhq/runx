//! Authority-free deterministic JavaScript engine for runx.

/// Exact shared host/worker wire contract; this is an alias, not a second
/// protocol definition.
pub use runx_contracts::javascript_worker as protocol;

mod engine;
mod limits;
mod server;

pub use server::serve;
