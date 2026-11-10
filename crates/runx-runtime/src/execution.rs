//! Execution cluster.
//!
//! - `engine`: the act engine, the `Runtime` and its step orchestration.
//! - `workflow`: workflow loading and step lookup helpers.
//! - `test`: test fixture replay and assertion engine.
//! - `orchestrator`: canonical entrypoint for local skill, workflow, and test
//!   execution.
//! - `skill_front`: the skill front; compiles a skill run into an execution and seals it through the act engine.

pub mod engine;
pub(crate) mod operator_context;
pub mod orchestrator;
pub(crate) mod outcome;
pub(crate) mod output_projection;
pub(crate) mod prepared_skill;
pub(crate) mod reuse;
pub mod run_record;
pub(crate) mod skill_context;
pub mod skill_front;
pub mod test;
pub(crate) mod workflow;
pub(crate) mod workflow_index;
