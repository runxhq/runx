use std::collections::BTreeMap;

use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, ProcessList, ProcessOutput, RunxProcess, TerminalSize,
};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessStartInput {
    pub(super) repo_root: String,
    pub(super) command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) args: Vec<String>,
    pub(super) cwd: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) tty: Option<TerminalSize>,
    pub(super) max_seconds: u64,
}

impl CapabilityInput for ProcessStartInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            ("cwd".to_owned(), JsonValue::String(".".to_owned())),
            (
                "max_seconds".to_owned(),
                JsonValue::Number(JsonNumber::U64(3_600)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessReadInput {
    pub(super) process_id: String,
    pub(super) after: u64,
    pub(super) max_bytes: u64,
}

impl CapabilityInput for ProcessReadInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("after".to_owned(), JsonValue::Number(JsonNumber::U64(0))),
            (
                "max_bytes".to_owned(),
                JsonValue::Number(JsonNumber::U64(16 * 1024)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessWriteInput {
    pub(super) process_id: String,
    pub(super) input: String,
}

impl CapabilityInput for ProcessWriteInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessResizeInput {
    pub(super) process_id: String,
    pub(super) rows: u16,
    pub(super) cols: u16,
}

impl CapabilityInput for ProcessResizeInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessIdInput {
    pub(super) process_id: String,
}

impl CapabilityInput for ProcessIdInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ProcessListInput {}

impl CapabilityInput for ProcessListInput {}

impl CapabilityOutput for RunxProcess {}
impl CapabilityOutput for ProcessOutput {}
impl CapabilityOutput for ProcessList {}

const PROCESS_ID: CapabilityField = CapabilityField {
    name: "process_id",
    description: "The process_id process.start returned.",
};

const START_FIELDS: &[CapabilityField] = &[
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
        description: "Zero to 128 exact string arguments.",
    },
    CapabilityField {
        name: "cwd",
        description: "Working directory relative to the project root.",
    },
    CapabilityField {
        name: "env",
        description: "Bounded non-secret environment values for the process.",
    },
    CapabilityField {
        name: "tty",
        description: "Run it in a terminal of these rows and cols (Unix hosts), for an interactive program.",
    },
    CapabilityField {
        name: "max_seconds",
        description: "How long it may run before its supervisor stops it: 1 to 86400, 3600 by default.",
    },
];

const READ_FIELDS: &[CapabilityField] = &[
    PROCESS_ID,
    CapabilityField {
        name: "after",
        description: "The cursor a previous read returned as next; 0 for the start of what the log keeps.",
    },
    CapabilityField {
        name: "max_bytes",
        description: "About how much output to return, at most 262144.",
    },
];

const WRITE_FIELDS: &[CapabilityField] = &[
    PROCESS_ID,
    CapabilityField {
        name: "input",
        description: "Text for its stdin or terminal, up to 65536 bytes; control keys such as \\u0003 (Ctrl-C) included.",
    },
];

const RESIZE_FIELDS: &[CapabilityField] = &[
    PROCESS_ID,
    CapabilityField {
        name: "rows",
        description: "The terminal's new height, 1 to 1000.",
    },
    CapabilityField {
        name: "cols",
        description: "The terminal's new width, 1 to 1000.",
    },
];

const PROCESS: CapabilityArtifacts = CapabilityArtifacts::Wrapped {
    output: "process",
    packet: "runx.process.v1",
};

static START: TypedNativeCapability<ProcessStartInput, RunxProcess> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "process.start",
        owner: "runx-runtime/process",
        summary: "Start a long-running process under runx supervision: it outlives the call, and later calls read, write and stop it.",
        scopes: &["process.exec"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: PROCESS,
        fields: START_FIELDS,
    },
    super::start,
)
.describing(super::start_effect);

static READ: TypedNativeCapability<ProcessReadInput, ProcessOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "process.read",
        owner: "runx-runtime/process",
        summary: "Read a process's output after a cursor, stdout and stderr tagged, and whether it still runs.",
        scopes: &["process.exec"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "process_output",
            packet: "runx.process.output.v1",
        },
        fields: READ_FIELDS,
    },
    super::read,
);

static WRITE: TypedNativeCapability<ProcessWriteInput, RunxProcess> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "process.write",
        owner: "runx-runtime/process",
        summary: "Write to a running process's stdin or terminal.",
        scopes: &["process.exec"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: PROCESS,
        fields: WRITE_FIELDS,
    },
    super::write,
)
.describing(super::no_effect);

static RESIZE: TypedNativeCapability<ProcessResizeInput, RunxProcess> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "process.resize",
        owner: "runx-runtime/process",
        summary: "Resize a running process's terminal.",
        scopes: &["process.exec"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: PROCESS,
        fields: RESIZE_FIELDS,
    },
    super::resize,
)
.describing(super::no_effect);

static STOP: TypedNativeCapability<ProcessIdInput, RunxProcess> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "process.stop",
        owner: "runx-runtime/process",
        summary: "Stop a process: terminate it, then kill it, and return it exited.",
        scopes: &["process.exec"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: PROCESS,
        fields: &[PROCESS_ID],
    },
    super::stop,
)
.describing(super::no_effect);

static LIST: TypedNativeCapability<ProcessListInput, ProcessList> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "process.list",
        owner: "runx-runtime/process",
        summary: "List the processes this run's actor started in the project, most recently changed first.",
        scopes: &["process.exec"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "process_list",
            packet: "runx.process.list.v1",
        },
        fields: &[],
    },
    super::list,
);

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&START, &READ, &WRITE, &RESIZE, &STOP, &LIST];
