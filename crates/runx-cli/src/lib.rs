pub mod cli_args;
pub mod cli_error;
pub(crate) mod cli_io;
pub mod command_spec;
pub mod config;
pub mod connector;
pub mod doctor;
pub mod document_input;
pub mod export;
pub mod history;
pub mod list;
pub mod login;
mod managed_agent;
pub mod mcp;
pub mod notarize;
pub mod policy;
pub mod process;
mod project;
pub mod registry;
pub mod resume;
pub mod router;
pub mod runtime;
pub mod secret;
pub mod skill;
pub mod tool;
pub mod verify;

pub use project::run_native_new_with_workspace;

#[cfg(test)]
mod release_identity_tests {
    #[test]
    fn the_runtime_runx_version_is_the_cli_version() {
        assert_eq!(runx_runtime::RUNX_VERSION, env!("CARGO_PKG_VERSION"));
    }
}
