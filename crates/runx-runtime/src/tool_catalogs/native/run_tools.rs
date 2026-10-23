//! `run.list`: the workspace's runs, read from their records.

use runx_contracts::schema::BoundedVec;
use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, RUN_LIST_MAX_RUNS, RunList, RunListSchema,
};
use serde::{Deserialize, Serialize};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability};
use crate::RuntimeError;
use crate::execution::run_record::{RunFilter, list_runs};
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
        scopes: &["run.read"],
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

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&LIST];

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
    Ok(RunListOutput {
        run_list: RunList {
            schema: RunListSchema::V1,
            runs,
        },
    })
}
