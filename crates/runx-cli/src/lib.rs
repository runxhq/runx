pub mod always_on;
mod answer;
mod at_login;
pub mod authority;
pub mod cli_args;
pub mod cli_error;
pub(crate) mod cli_io;
pub mod command_spec;
pub mod config;
pub mod connector;
pub mod doctor;
pub mod document_input;
mod drive;
pub mod export;
pub mod history;
pub mod list;
mod local_model;
pub mod login;
pub mod loops;
pub mod mcp;
pub mod notarize;
pub mod process;
pub mod registry;
pub mod resume;
pub mod router;
pub mod runtime;
pub mod secret;
pub mod skill;
pub mod tool;
pub mod trust;
pub mod verify;
pub mod workspace;

#[cfg(test)]
mod release_identity_tests {
    #[test]
    fn the_runtime_runx_version_is_the_cli_version() {
        assert_eq!(runx_runtime::RUNX_VERSION, env!("CARGO_PKG_VERSION"));
    }
}
