//! `run.list`, `run.read` and `call.read`: the workspace's runs, read from
//! their records, the tool forms of `runx history` and `runx history
//! <run-id>`, and one call a run made, read whole.

use runx_contracts::schema::BoundedVec;
use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, RUN_LIST_MAX_RUNS, RunList, RunListSchema, RunRead,
};
use serde::{Deserialize, Serialize};

use super::NativeInvocation;
use super::capability::{NativeCapability, TypedNativeCapability};
use crate::RuntimeError;
use crate::execution::run_record::{RunFilter, list_runs, read_call, read_run};
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

/// One call a run made, by its address there, read whole: written as an
/// artifact of `runx.call_read.v1`, labelled with everything it holds, and
/// read in bounded pages through `artifact.open` and `artifact.read`. Without
/// `run_id` it reads the run the call is made in, from its root.
#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
struct CallReadInput {
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    call: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    after: Option<u32>,
}

impl CapabilityInput for CallReadInput {}

const CALL_FIELDS: &[CapabilityField] = &[
    field(
        "run_id",
        "A run of this workspace; without it, the run the call is made in, from its root.",
    ),
    field(
        "call",
        "The call's address in its run, <step>/<index>, as run.read's events name it.",
    ),
    field(
        "after",
        "For a script call, the position its calls are read from: the next an earlier read gave.",
    ),
];

static CALL: TypedNativeCapability<CallReadInput, runx_contracts::Artifact> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "call.read",
            source: "runx-runtime/runs",
            summary: "Read one call a run made, whole, as an artifact read in pages: its input, how it ended, the turn that made it, and each call its script made.",
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "call",
                packet: runx_contracts::ARTIFACT_SCHEMA,
            },
            fields: CALL_FIELDS,
        },
        call,
    );

pub(super) const CAPABILITIES: &[&dyn NativeCapability] = &[&LIST, &READ, &CALL];

/// What a reader does with a call address `call.read` can't read.
const ADDRESS_NEXT: &str =
    "pass call as <step>/<index>, the address run.read's events give each call";

fn call(
    invocation: &NativeInvocation<'_, CallReadInput>,
) -> Result<runx_contracts::Artifact, RuntimeError> {
    let failed = |message: String| RuntimeError::failed("call.read".to_owned(), message);
    let run_id = match (&invocation.inputs.run_id, invocation.call) {
        (Some(run_id), _) => run_id.as_str(),
        (None, Some(call)) => crate::execution::run_record::root_run_id(&call.run_id),
        (None, None) => {
            return Err(failed(
                "run_id is required outside a run, which has no run of its own to read".to_owned(),
            ));
        }
    };
    let address = invocation.inputs.call.as_str();
    let (step_id, index) = address
        .rsplit_once('/')
        .and_then(|(step, index)| Some((step, index.parse::<u32>().ok()?)))
        .filter(|(step, _)| !step.is_empty())
        .ok_or_else(|| {
            RuntimeError::Failed(Box::new(
                crate::Failure::new("call.read", format!("call {address} is no <step>/<index>"))
                    .next(ADDRESS_NEXT),
            ))
        })?;
    let (read, labels) = read_call(
        invocation.env,
        invocation.skill_directory,
        run_id,
        step_id,
        index,
        invocation.inputs.after.unwrap_or(0),
    )
    .map_err(|error| match error {
        crate::execution::run_record::RunRecordError::UnknownCall { .. } => RuntimeError::Failed(
            Box::new(crate::Failure::new("call.read", error.to_string()).next(ADDRESS_NEXT)),
        ),
        error => failed(error.to_string()),
    })?;
    let value = JsonValue::from_serialize(&read)
        .map_err(|error| failed(format!("writing the call: {error}")))?;
    // Kept under the run reading it, when it's one, or the run it reads.
    let holder = invocation.call.map_or(run_id, |call| call.run_id.as_str());
    let artifacts =
        crate::paths::RunxPaths::resolve(invocation.env, invocation.skill_directory).artifacts();
    crate::services::persist_json_artifact(
        &artifacts,
        holder,
        "calls",
        &match invocation.inputs.after {
            Some(after) if after > 0 => format!("{run_id}-{step_id}-{index}-from-{after}"),
            _ => format!("{run_id}-{step_id}-{index}"),
        }
        .replace('/', "_"),
        &value,
        labels,
    )
    .map_err(failed)
}

fn read(invocation: &NativeInvocation<'_, RunReadInput>) -> Result<RunReadOutput, RuntimeError> {
    let failed = |message: String| RuntimeError::failed("run.read".to_owned(), message);
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
        return Err(RuntimeError::failed(
            "run.list".to_owned(),
            format!("limit must be between 1 and {MAX_LIMIT}"),
        ));
    }
    let failed = |message: String| RuntimeError::failed("run.list".to_owned(), message);
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
