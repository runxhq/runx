//! `run.list` and `run.read`: the workspace's runs, read from their records,
//! the tool forms of `runx history` and `runx history <run-id>`.

use runx_contracts::schema::BoundedVec;
use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, RUN_LIST_MAX_RUNS, RunList, RunListSchema, RunRead,
};
use serde::{Deserialize, Serialize};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability};
use crate::RuntimeError;
use crate::execution::run_record::{RunFilter, list_runs, read_run};
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

const MAX_LIMIT: u64 = RUN_LIST_MAX_RUNS as u64;

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct RunListInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    since: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    until: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    period: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    as_of: Option<String>,
    limit: u64,
}

impl CapabilityInput for RunListInput {
    fn defaults() -> JsonObject {
        JsonObject::from([("limit".to_owned(), JsonValue::Number(JsonNumber::U64(50)))])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct RunListOutput {
    run_list: RunList,
}

impl CapabilityOutput for RunListOutput {}

const FIELDS: &[CapabilityField] = &[
    field("query", "Case-insensitive run id or skill search."),
    field("skill", "Skill filter."),
    field(
        "state",
        "running, paused or sealed, or a sealed run's outcome.",
    ),
    field(
        "since",
        "RFC3339 inclusive lower bound on when a run started.",
    ),
    field(
        "until",
        "RFC3339 inclusive upper bound on when a run started.",
    ),
    field(
        "period",
        "Relative lookback such as 7d, used only when since is absent.",
    ),
    field(
        "as_of",
        "Optional RFC3339 anchor for a reproducible lookback.",
    ),
    field("limit", "Maximum runs from 1 to 1000."),
];

const fn field(name: &'static str, description: &'static str) -> CapabilityField {
    CapabilityField { name, description }
}

static LIST: TypedNativeCapability<RunListInput, RunListOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "run.list",
        source: "runx-runtime/runs",
        summary: "List the workspace's runs, most recent first, with what each waits on or sealed.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "run_list",
            packet: "runx.run.list.v1",
        },
        fields: FIELDS,
    },
    list,
);

/// One run's record, as `runx history <run-id>` reads it: its summary, what it
/// waits on, and its events and its child runs' after `after`. Without
/// `run_id` it reads the run the call is made in, from its root, whose id a
/// step never names (`$run.id` only keys an operation), so a method reads back
/// what its own turns and child runs did.
#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct RunReadInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<String>,
}

impl CapabilityInput for RunReadInput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct RunReadOutput {
    run_read: RunRead,
}

impl CapabilityOutput for RunReadOutput {}

const READ_FIELDS: &[CapabilityField] = &[
    field(
        "run_id",
        "A run of this workspace; without it, the run the call is made in, from its root.",
    ),
    field("after", "The cursor an earlier read returned."),
];

static READ: TypedNativeCapability<RunReadInput, RunReadOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "run.read",
        source: "runx-runtime/runs",
        summary: "Read one run's record: its summary, what it waits on, and its events and its child runs' in commit order.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Named {
            output: "run_read",
            packet: "runx.run_read.v1",
        },
        fields: READ_FIELDS,
    },
    read,
);

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&LIST, &READ];

fn read(invocation: &NativeInvocation<'_, RunReadInput>) -> Result<RunReadOutput, RuntimeError> {
    let failed = |message: String| RuntimeError::SkillFailed {
        skill_name: "run.read".to_owned(),
        message,
    };
    let run_id = match (&invocation.inputs.run_id, invocation.call) {
        (Some(run_id), _) => run_id.as_str(),
        (None, Some(call)) => crate::execution::run_record::root_run_id(&call.run_id),
        (None, None) => {
            return Err(failed(
                "run_id is required outside a run, which has no run of its own to read".to_owned(),
            ));
        }
    };
    let run_read = read_run(
        invocation.env,
        invocation.skill_directory,
        run_id,
        invocation.inputs.after.as_deref(),
    )
    .map_err(|error| failed(error.to_string()))?;
    Ok(RunReadOutput { run_read })
}

fn list(invocation: &NativeInvocation<'_, RunListInput>) -> Result<RunListOutput, RuntimeError> {
    let inputs = invocation.inputs;
    if !(1..=MAX_LIMIT).contains(&inputs.limit) {
        return Err(RuntimeError::SkillFailed {
            skill_name: "run.list".to_owned(),
            message: format!("limit must be between 1 and {MAX_LIMIT}"),
        });
    }
    let failed = |message: String| RuntimeError::SkillFailed {
        skill_name: "run.list".to_owned(),
        message,
    };
    let since = match (&inputs.since, &inputs.period) {
        (Some(since), _) => Some(since.clone()),
        (None, Some(period)) => {
            Some(crate::time::lookback_since(period, inputs.as_of.as_deref()).map_err(failed)?)
        }
        (None, None) => None,
    };
    let listing = list_runs(
        invocation.env,
        invocation.skill_directory,
        &RunFilter {
            query: inputs.query.clone(),
            skill: inputs.skill.clone(),
            state: inputs.state.clone(),
            since,
            until: inputs.until.clone(),
            limit: usize::try_from(inputs.limit).ok(),
        },
    )?;
    let runs = BoundedVec::new(listing.runs)
        .ok_or_else(|| failed(format!("listed more than {MAX_LIMIT} runs")))?;
    let unreadable = BoundedVec::new(listing.unreadable)
        .ok_or_else(|| failed(format!("listed more than {MAX_LIMIT} unreadable runs")))?;
    Ok(RunListOutput {
        run_list: RunList {
            schema: RunListSchema::V1,
            runs,
            unreadable,
        },
    })
}
