//! Single integration-test binary for runx-core.
//!
//! Each module below is one integration test file, compiled and linked once
//! as a single binary instead of one binary per file. `autotests = false` in
//! Cargo.toml keeps Cargo from also building each file as its own binary.
//! See .scafld/specs/active/test-surface-build-consolidation.md.

mod kernel_eval;
mod policy_fixtures;
mod state_machine_fixtures;
mod state_machine_proptest;
