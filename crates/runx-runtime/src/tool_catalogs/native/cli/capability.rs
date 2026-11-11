use runx_contracts::{CliHelp, CliHelpFlag, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CaptureHelpInput {
    pub(super) repo_root: String,
    pub(super) command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) args: Vec<String>,
    pub(super) help_flag: CliHelpFlag,
    pub(super) cwd: String,
}

impl CapabilityInput for CaptureHelpInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            (
                "help_flag".to_owned(),
                JsonValue::String("--help".to_owned()),
            ),
            ("cwd".to_owned(), JsonValue::String(".".to_owned())),
        ])
    }
}

const FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative project root.",
    },
    CapabilityField {
        name: "command",
        description: "Exact executable name or path; no shell expansion is performed.",
    },
    CapabilityField {
        name: "args",
        description: "Optional subcommand arguments placed before the help flag.",
    },
    CapabilityField {
        name: "help_flag",
        description: "Help form: --help, -h, or help.",
    },
    CapabilityField {
        name: "cwd",
        description: "Working directory relative to the project root.",
    },
];

// It runs the CLI it's given, so it's a read a host process makes.
static CAPTURE_HELP: TypedNativeCapability<CaptureHelpInput, CliHelp> =
    TypedNativeCapability::new_with_execution_boundary(
        CapabilityDefinition {
            id: "cli.capture_help",
            source: "runx-runtime/cli",
            summary: "Capture bounded CLI help through exact argv in a contained directory.",
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "cli_help",
                packet: "runx.cli.help.v1",
            },
            fields: FIELDS,
        },
        super::capture_help,
        crate::process_invocation::NATIVE_COMMAND_EXECUTION_BOUNDARY,
    );

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&CAPTURE_HELP];
