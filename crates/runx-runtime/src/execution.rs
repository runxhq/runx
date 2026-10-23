//! Execution cluster.
//!
//! - `runner`: the `Runtime` graph engine and step orchestrator.
//! - `graph`: graph loading and step lookup helpers.
//! - `harness`: harness fixture replay and assertion engine.
//! - `orchestrator`: canonical entrypoint for local skill, graph, and harness
//!   execution.
//! - `skill_front`: the skill front; compiles a skill run into an execution and seals it through the act engine.

pub(crate) mod graph;
pub(crate) mod graph_index;
pub mod harness;
pub(crate) mod operator_context;
pub mod orchestrator;
pub(crate) mod outcome;
pub(crate) mod output_projection;
pub(crate) mod prepared_skill;
pub mod run_record;
pub mod runner;
pub(crate) mod skill_context;
pub mod skill_front;
