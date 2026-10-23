use std::collections::BTreeMap;

use runx_contracts::{CommandExecution, CommandPlan, JsonNumber, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandInput {
    pub(super) repo_root: String,
    pub(super) command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) args: Vec<String>,
    pub(super) cwd: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) env: BTreeMap<String, String>,
    pub(super) timeout_ms: u64,
    pub(super) output_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) expected_command_digest: Option<String>,
}

impl CapabilityInput for CommandInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            ("cwd".to_owned(), JsonValue::String(".".to_owned())),
            (
                "timeout_ms".to_owned(),
                JsonValue::Number(JsonNumber::U64(60_000)),
            ),
            // The result carries the bounded output unless asked otherwise (agent-sessions §4.10).
            (
                "output_mode".to_owned(),
                JsonValue::String("text".to_owned()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandPlanOutput {
    pub(super) command_plan: CommandPlan,
}

impl CapabilityOutput for CommandPlanOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct CommandExecutionOutput {
    pub(super) command_execution: CommandExecution,
}

impl CapabilityOutput for CommandExecutionOutput {}

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
        description: "Zero to 128 exact string arguments.",
    },
    CapabilityField {
        name: "cwd",
        description: "Working directory relative to the project root.",
    },
    CapabilityField {
        name: "env",
        description: "Bounded non-secret environment values for the child process.",
    },
    CapabilityField {
        name: "timeout_ms",
        description: "Execution timeout from 1000 to 3600000 milliseconds.",
    },
    CapabilityField {
        name: "output_mode",
        description: "What the result carries of the output: text (stdout and stderr, as far as the capture limit holds them, the default), digest (its digest and size only, the output kept off the record), or json (stdout as one JSON object).",
    },
    CapabilityField {
        name: "expected_command_digest",
        description: "Optional prior command.plan digest that must match before execution.",
    },
];

static PLAN: TypedNativeCapability<CommandInput, CommandPlanOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "command.plan",
        source: "runx-runtime/command",
        summary: "Normalize and digest one exact argv command without executing it.",
        scopes: &[],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "command_plan",
            packet: "runx.command.plan.v1",
        },
        fields: FIELDS,
    },
    super::plan,
);

static EXECUTE: TypedNativeCapability<CommandInput, CommandExecutionOutput> =
    TypedNativeCapability::new_with_execution_boundary(
        CapabilityDefinition {
            id: "command.execute",
            source: "runx-runtime/command",
            summary: "Execute exact argv under runtime-owned process supervision.",
            scopes: &["process.exec"],
            effect: CapabilityEffect::Mutate,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "command_execution",
                packet: "runx.command.execution.v1",
            },
            fields: FIELDS,
        },
        super::execute,
        crate::process_invocation::NATIVE_COMMAND_EXECUTION_BOUNDARY,
    )
    .describing(super::execute_effect);

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&PLAN, &EXECUTE];
