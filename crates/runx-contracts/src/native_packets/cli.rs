//! What the native CLI tools return.

use serde::{Deserialize, Serialize};

use crate::schema::{BoundedVec, NonEmptyString, RunxSchema};

/// The most subcommand arguments one help capture places before its flag.
pub const CLI_HELP_MAX_ARGS: usize = 32;

/// The form a CLI is asked for its help in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
pub enum CliHelpFlag {
    #[serde(rename = "--help")]
    Long,
    #[serde(rename = "-h")]
    Short,
    #[serde(rename = "help")]
    Subcommand,
}

impl CliHelpFlag {
    /// The argument as the CLI receives it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Long => "--help",
            Self::Short => "-h",
            Self::Subcommand => "help",
        }
    }
}

/// What `cli.capture_help` returns: one CLI's help output, redacted, and how
/// it exited.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.cli.help.v1")]
pub struct CliHelp {
    pub command: NonEmptyString,
    pub args: BoundedVec<NonEmptyString, 0, CLI_HELP_MAX_ARGS>,
    pub help_flag: CliHelpFlag,
    /// The absolute directory the CLI ran in.
    pub cwd: NonEmptyString,
    pub stdout: String,
    pub stderr: String,
    /// Null when a signal ended the CLI.
    pub exit_code: Option<i64>,
}
