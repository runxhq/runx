//! Single integration-test binary for runx-receipts.
//!
//! Each module below is one integration test file, compiled and linked once
//! as a single binary instead of one binary per file. `autotests = false` in
//! Cargo.toml keeps Cargo from also building each file as its own binary.
//! See .scafld/specs/active/test-surface-build-consolidation.md.

/// The public fixture identity the modules below seal receipts under.
mod fixture_key;

mod conformance;
mod receipt_contracts;
mod receipt_tree_fixtures;
mod receipt_verify_corpus;
