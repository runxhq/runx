//! Connector drivers: each performs the requests the kernel hands it, as the
//! connector manifest declares them, and reports what it saw.
//!
//! Admission, approval, custody, recovery, readback matching and the seal are
//! the kernel's ([`crate::kernel::operation`]); a driver decides none of them.
//! It receives an [`AdmittedCall`](crate::kernel::operation::AdmittedCall),
//! which only the kernel can build, so it performs nothing admission did not
//! prove. Every driver is generic: provider specifics live in manifests.

pub(crate) mod artifact;
pub(crate) mod http;
#[cfg(feature = "mcp")]
pub(crate) mod mcp;
pub(crate) mod process;
pub(crate) mod store;
pub(crate) mod web;
