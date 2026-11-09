//! Single integration-test binary for runx-contracts.
//!
//! Each module below is one integration test file, compiled and linked once
//! as a single binary instead of one binary per file. `autotests = false` in
//! Cargo.toml keeps Cargo from also building each file as its own binary.
//! See .scafld/specs/active/test-surface-build-consolidation.md.

mod doctor_fixtures;
mod fixture_support;
mod host_protocol_fixtures;
mod operational_policy;
mod published_schemas;
mod purchase;
mod reference;
mod run_spine_fixtures;
mod schema_validation;
mod schema_wire_conformance;
mod x402;
