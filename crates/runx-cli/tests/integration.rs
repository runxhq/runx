//! Single integration-test binary for runx-cli.
//!
//! Each module below is one integration test file, compiled and linked once
//! as a single binary instead of one binary per file. `support` is the shared
//! helper module (tests/support/), referenced by test modules as
//! `crate::support`. `autotests = false` in Cargo.toml keeps Cargo from also
//! building each file as its own binary.
//! Cross-command behavior belongs in `operator_journeys`; focused modules keep
//! only distinct parser, security, protocol, and failure-boundary invariants.
//! See .scafld/specs/active/test-surface-build-consolidation.md.

mod connect;
mod credential;
mod data;
mod doctor;
mod export;
mod harness;
mod kernel;
mod list;
mod local_credential;
mod locality;
mod login;
mod mcp_dogfood;
mod native_no_ts;
mod new_skill_authoring;
mod official_skill_admission;
mod operator_journeys;
mod parser;
mod policy;
mod registry;
mod router;
mod skill;
mod support;
mod tool;
mod verify;
