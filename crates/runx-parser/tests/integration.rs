//! Single integration-test binary for runx-parser.
//!
//! Each module below is one integration test file, compiled and linked once
//! as a single binary instead of one binary per file. `autotests = false` in
//! Cargo.toml keeps Cargo from also building each file as its own binary.
//! See .scafld/specs/active/test-surface-build-consolidation.md.

mod parser_catalog;
mod parser_connectors;
mod parser_fixtures;
mod parser_graph_context_edges;
mod parser_marketplace;
mod parser_packet;
mod parser_rejections;
mod parser_skill_package;
mod parser_step_kinds;
mod parser_turn_limits;
mod plugin_connectors;
