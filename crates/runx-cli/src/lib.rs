pub mod add;
pub mod assistant;
pub mod cli_args;
pub mod cli_error;
pub(crate) mod cli_io;
pub mod command_spec;
pub mod config;
pub mod connect;
pub mod connector;
pub mod credential;
pub mod dev;
pub mod doctor;
pub mod document_input;
pub mod export;
pub mod history;
pub mod kernel;
pub mod list;
pub mod login;
mod managed_agent;
pub mod mcp;
mod official_skills;
pub mod parser;
pub mod policy;
mod project;
pub mod publish;
pub mod registry;
pub mod resume;
pub mod router;
pub mod runtime;
pub mod skill;
pub mod tool;
pub mod turn;
pub mod verify;

pub use project::{run_native_init, run_native_new_with_workspace};

#[cfg(test)]
mod release_identity_tests {
    #[test]
    fn native_runtime_release_identity_tracks_cli_version() {
        assert_eq!(
            runx_runtime::EXECUTION_RUNTIME_RELEASE,
            env!("CARGO_PKG_VERSION")
        );
    }
}
