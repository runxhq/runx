//! Single integration-test binary for runx-contracts.
//!
//! Each module below is one integration test file, compiled and linked once
//! as a single binary instead of one binary per file. `autotests = false` in
//! Cargo.toml keeps Cargo from also building each file as its own binary.
//! See .scafld/specs/active/test-surface-build-consolidation.md.

mod act_assignment_fixtures;
mod credential_delivery_fixtures;
mod doctor_fixtures;
mod example_external_fixture;
mod execution_fixtures;
mod fixture_support;
mod harness_spine_fixtures;
mod host_protocol_fixtures;
mod operational_policy;
mod operational_proposal_fixtures;
mod paid_invocation;
mod reference;
mod schema_validation;
mod schema_wire_conformance;
mod x402;
