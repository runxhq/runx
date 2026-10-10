//! Single integration-test binary for runx-runtime.
//!
//! Each module below is one integration test file. They are compiled and
//! linked once as a single binary instead of one binary per file; see
//! .scafld/specs/active/test-surface-build-consolidation.md. `autotests = false`
//! in Cargo.toml keeps Cargo from building each file as its own binary.

mod abnormal_seal;
mod agent_turns;
mod approval;
mod call_acts;
mod config;
mod credential_delivery;
mod credential_delivery_core;
mod dev;
mod doctor;
mod effect_finality;
mod governance_witness;
mod graph_context_materialization;
mod harness_fixtures;
mod hello_graph;
mod init;
mod javascript_worker;
mod javascript_worker_hostile;
mod javascript_worker_support;
mod license_boundary;
mod managed_chat_completions;
mod mcp_server;
mod parity;
mod policy_approval;
mod process_invocation_contract;
mod receipt_archive;
mod receipt_paths;
mod receipt_refs;
mod receipt_signing;
mod receipt_store;
mod receipt_tree;
mod registry;
mod registry_client;
mod registry_install;
mod registry_paid_listing;
mod skill_author_runtime_fixtures;
mod skill_issue_intake;
mod skill_manual_context;
mod skill_run;
mod support;
mod tool_catalogs;
mod validated_skill_package;
mod volume_independent_twitter;
mod volume_independent_twitter_progress;
