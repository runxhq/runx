//! The inverse of a run's edits (agent-sessions §4.5): `fs.inverse` reads
//! the edit calls a run made, by `call_id`, and returns the bundle that
//! restores what each file held before them, bound to what it holds after
//! them. `revert` applies it with `fs.apply_bundle`, all or none, so a file
//! changed since is refused, naming it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use runx_contracts::schema::{NonEmptyString, NonEmptyVec};
use runx_contracts::{
    CallTarget, Digest, FileBoundDelete, FileBundleWrite, FileInverseBundle, JsonValue,
    RunCallOutcome, RunEventFact,
};
use serde::{Deserialize, Serialize};

use super::super::NativeInvocation;
use super::super::capability::TypedNativeCapability;
use crate::paths::RunxPaths;
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput, RuntimeError,
};

const INVERSE_TOOL: &str = "fs.inverse";

/// The edit tools and the packet each returns.
const EDITS: &[(&str, &str)] = &[
    ("fs.write", "file_write"),
    ("fs.edit", "file_edit"),
    ("fs.apply_bundle", "file_bundle_apply"),
];

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::tool_catalogs::native) struct FileInverseInput {
    pub(super) run_id: String,
    pub(super) call_ids: Vec<String>,
}

impl CapabilityInput for FileInverseInput {}

impl CapabilityOutput for FileInverseBundle {}

/// One change a named call made, as the journal records it: the call that
/// names it, and the change.
struct Edit {
    call_id: String,
    change: crate::filesystem::AppliedChange,
}

fn inverse(
    invocation: &NativeInvocation<'_, FileInverseInput>,
) -> Result<FileInverseBundle, RuntimeError> {
    let inputs = invocation.inputs;
    let refused = |message: String| super::super::invalid_input(INVERSE_TOOL, message);
    if inputs.call_ids.is_empty() {
        return Err(refused("name at least one call to undo".to_owned()));
    }
    let paths = RunxPaths::resolve(invocation.env, invocation.skill_directory);
    let workspace = canonical(paths.workspace());
    let store = crate::kernel::store::KernelStore::for_project(&paths)
        .map_err(|error| refused(error.to_string()))?;
    let edits = recorded_edits(invocation.env, &workspace, &store, inputs)?;
    // Per file: what it held before the first change, and what it holds
    // after the last.
    let mut root: Option<PathBuf> = None;
    let mut before = BTreeMap::<String, Option<String>>::new();
    let mut after = BTreeMap::<String, Option<String>>::new();
    for edit in &edits {
        match &root {
            Some(root) if *root != edit.change.root => {
                return Err(refused(format!(
                    "call {} changed {}, and another call {}; one revert restores one root",
                    edit.call_id,
                    edit.change.root.display(),
                    root.display()
                )));
            }
            Some(_) => {}
            None => root = Some(edit.change.root.clone()),
        }
        for file in &edit.change.files {
            before
                .entry(file.path.clone())
                .or_insert_with(|| file.before.clone());
            after.insert(file.path.clone(), file.after.clone());
        }
    }
    let root = root.ok_or_else(|| refused("the calls changed nothing".to_owned()))?;
    // Each file named from the workspace root, as every file tool names it.
    let base = root
        .strip_prefix(&workspace)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| refused(format!("{} lies outside the workspace", root.display())))?;
    let from_workspace = |path: String| {
        if base.is_empty() {
            path
        } else {
            format!("{base}/{path}")
        }
    };
    let named = |value: String, what: &str| {
        NonEmptyString::new(value)
            .ok_or_else(|| refused(format!("an edit recorded an empty {what}")))
    };
    let mut writes = Vec::new();
    let mut deletes = Vec::new();
    for (path, held) in before {
        let now = after
            .get(&path)
            .cloned()
            .flatten()
            .map(|digest| {
                Digest::new(digest)
                    .ok_or_else(|| refused(format!("{path} recorded no sha256 digest")))
            })
            .transpose()?;
        let held = held
            .map(|blob| store.blob(&blob))
            .transpose()
            .map_err(|error| refused(format!("{path}: {error}")))?;
        match (held, now) {
            (Some(contents), now) => writes.push(FileBundleWrite {
                contents: String::from_utf8(contents)
                    .map_err(|_| refused(format!("{path} held content that is not text")))?,
                create: now.is_none(),
                expected_digest: now,
                path: named(from_workspace(path), "path")?,
            }),
            (None, Some(digest)) => deletes.push(FileBoundDelete {
                path: named(from_workspace(path), "path")?,
                expected_digest: digest,
            }),
            (None, None) => {}
        }
    }
    // Each call named once, in the order its first change was made.
    let mut calls = Vec::new();
    for edit in edits {
        if !calls.contains(&edit.call_id) {
            calls.push(edit.call_id);
        }
    }
    let calls = calls
        .into_iter()
        .map(|call_id| named(call_id, "call id"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FileInverseBundle {
        writes,
        deletes,
        calls: NonEmptyVec::new(calls)
            .ok_or_else(|| refused("the calls changed nothing".to_owned()))?,
    })
}

/// The changes the calls `inputs` names made, in the order they were made.
/// A call named by its `call_id` is one of the run's own: an edit names its
/// own change, and any other call each change made under its address, by
/// the calls its script made or by its child run's steps and calls.
fn recorded_edits(
    env: &BTreeMap<String, String>,
    workspace: &Path,
    store: &crate::kernel::store::KernelStore,
    inputs: &FileInverseInput,
) -> Result<Vec<Edit>, RuntimeError> {
    let refused = |message: String| super::super::invalid_input(INVERSE_TOOL, message);
    let run_id = inputs.run_id.as_str();
    let wanted = inputs
        .call_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    // A call is recorded as each of its outcomes; its last is how it ended.
    // A step that succeeded is recorded with its output, which names the
    // change it journaled when it made one.
    let mut latest = BTreeMap::<(String, u32, u32), Recorded>::new();
    let mut steps = Vec::<(String, JsonValue)>::new();
    let mut cursor = None;
    loop {
        let page =
            crate::execution::run_record::read_run(env, workspace, run_id, cursor.as_deref())
                .map_err(|error| refused(error.to_string()))?;
        for record in page.events {
            if let RunEventFact::Step {
                succeeded: true,
                output,
                ..
            } = &record.event
            {
                steps.push((
                    record.stream.as_str().to_owned(),
                    JsonValue::Object(output.clone()),
                ));
                continue;
            }
            let RunEventFact::Call {
                step_id,
                attempt,
                index,
                target,
                call_id,
                outcome,
                ..
            } = record.event
            else {
                continue;
            };
            let stream = record.stream.as_str().to_owned();
            latest.insert(
                (stream.clone(), attempt, index),
                Recorded {
                    stream,
                    step_id: step_id.as_str().to_owned(),
                    index,
                    call_id: call_id.map(|id| id.as_str().to_owned()),
                    target,
                    outcome: *outcome,
                },
            );
        }
        if !page.more {
            break;
        }
        cursor = Some(page.cursor.as_str().to_owned());
    }
    // Each id names one of the run's own calls: ids are a turn's, so two
    // turns may each have made one.
    let mut named = BTreeMap::<&str, &Recorded>::new();
    for call in latest.values() {
        let Some(id) = call.call_id.as_deref().filter(|id| wanted.contains(id)) else {
            continue;
        };
        if !crate::execution::run_record::is_calls_stream_of(&call.stream, run_id) {
            continue;
        }
        if named.insert(id, call).is_some() {
            return Err(refused(format!(
                "run {run_id} made more than one call {id}"
            )));
        }
    }
    if let Some(missing) = wanted.iter().find(|id| !named.contains_key(**id)) {
        return Err(refused(format!("run {run_id} made no call {missing}")));
    }
    let artifacts = RunxPaths::resolve(env, workspace).artifacts();
    // The change each of the run's own edit calls journaled, by its id in
    // the run's journal.
    let journaled = |call_id: &str, recorded: &Recorded| -> Result<u64, RuntimeError> {
        let packet = edit_packet(&recorded.target).ok_or_else(|| {
            refused(format!(
                "call {call_id} is a {} call, not an edit",
                label(&recorded.target)
            ))
        })?;
        let RunCallOutcome::Completed {
            succeeded: true,
            outputs,
            ..
        } = &recorded.outcome
        else {
            return Err(refused(format!("call {call_id} did not complete")));
        };
        let artifact = outputs
            .iter()
            .find(|output| output.output.as_str() == packet)
            .ok_or_else(|| refused(format!("call {call_id} recorded no {packet}")))?;
        let output = match crate::services::read_json_artifact(&artifacts, &artifact.artifact)
            .map_err(|message| refused(format!("call {call_id}: {message}")))?
        {
            JsonValue::Object(object) => match object.get("data") {
                Some(JsonValue::Object(data)) => data.clone(),
                _ => object,
            },
            _ => return Err(refused(format!("call {call_id} recorded no object"))),
        };
        match output.get("journal") {
            Some(JsonValue::Number(runx_contracts::JsonNumber::U64(bundle))) => Some(*bundle),
            Some(JsonValue::Number(runx_contracts::JsonNumber::I64(bundle))) => {
                u64::try_from(*bundle).ok()
            }
            _ => None,
        }
        .ok_or_else(|| refused(format!("call {call_id} journaled no change")))
    };
    // Every run of a tree journals its changes under its root's key, the
    // run id each inherits, so a change is its journal's bundle there.
    let journal = crate::execution::run_record::root_run_id(run_id);
    let mut changes = crate::filesystem::applied_changes(store, journal)?
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
    // Which call names each change, by the change's place in the journal's
    // order; a change made under two named calls is undone once.
    let mut naming = BTreeMap::<usize, String>::new();
    for (call_id, call) in named {
        // The changes the run's journal holds for this call: its own, for an
        // edit; for any other, each edit call under it, its script's and its
        // child run's turns' alike, and each edit its child run's steps made.
        let mut own = BTreeSet::new();
        let address = format!(
            "{run_id}/{}",
            crate::execution::run_record::call_step_id(&call.step_id, call.index)
        );
        if edit_packet(&call.target).is_some() {
            own.insert(journaled(call_id, call)?);
        } else {
            for made in latest.values().filter(|recorded| {
                recorded
                    .stream
                    .strip_prefix(&address)
                    .is_some_and(|rest| rest.starts_with('@') || rest.starts_with('/'))
                    && edit_packet(&recorded.target).is_some()
                    && matches!(
                        recorded.outcome,
                        RunCallOutcome::Completed {
                            succeeded: true,
                            ..
                        }
                    )
            }) {
                own.insert(journaled(call_id, made)?);
            }
            // Its child run's steps, and its child's children's, each the
            // change its output names.
            for (_, output) in steps.iter().filter(|(stream, _)| {
                stream
                    .strip_prefix(&address)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
            }) {
                own.extend(journals(output));
            }
        }
        let mut made = false;
        for (place, change) in changes.iter().enumerate() {
            let Some(change) = change else { continue };
            if change.run == journal && own.contains(&change.bundle) {
                made = true;
                naming.entry(place).or_insert_with(|| call_id.to_owned());
            }
        }
        if !made {
            return Err(refused(format!("call {call_id} made no edit")));
        }
    }
    Ok(naming
        .into_iter()
        .filter_map(|(place, call_id)| changes[place].take().map(|change| Edit { call_id, change }))
        .collect())
}

/// The changes a step's output names as journaled: each packet in it that
/// holds a `journal`, as a file edit's does.
fn journals(output: &JsonValue) -> Vec<u64> {
    let JsonValue::Object(fields) = output else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for value in fields.values() {
        let JsonValue::Object(packet) = value else {
            continue;
        };
        let data = match packet.get("data") {
            Some(JsonValue::Object(data)) => data,
            _ => packet,
        };
        match data.get("journal") {
            Some(JsonValue::Number(runx_contracts::JsonNumber::U64(bundle))) => found.push(*bundle),
            Some(JsonValue::Number(runx_contracts::JsonNumber::I64(bundle))) => {
                found.extend(u64::try_from(*bundle).ok());
            }
            _ => {}
        }
    }
    found
}

/// One call as the run recorded it, by its latest outcome.
struct Recorded {
    stream: String,
    step_id: String,
    index: u32,
    call_id: Option<String>,
    target: CallTarget,
    outcome: RunCallOutcome,
}

/// The packet an edit's call records, when `target` is an edit.
fn edit_packet(target: &CallTarget) -> Option<&'static str> {
    let CallTarget::Tool { tool } = target else {
        return None;
    };
    EDITS
        .iter()
        .find(|(edit, _)| *edit == tool.as_str())
        .map(|(_, packet)| *packet)
}

/// A call's target, as a refusal names it.
fn label(target: &CallTarget) -> String {
    match target {
        CallTarget::Tool { tool } => tool.as_str().to_owned(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|value| {
                value
                    .get("kind")
                    .and_then(|kind| kind.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "call".to_owned()),
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

const INVERSE_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "run_id",
        description: "The run whose edits to undo.",
    },
    CapabilityField {
        name: "call_ids",
        description: "The call ids to undo, together: an fs.write, fs.edit or fs.apply_bundle call undoes its own edit, and any other call the edits its script or its child run made.",
    },
];

pub(super) static INVERSE: TypedNativeCapability<FileInverseInput, FileInverseBundle> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: INVERSE_TOOL,
            source: "runx-runtime/fs",
            summary: "The bundle that undoes a run's edit calls: what each file held before them, bound to what it holds after them, for fs.apply_bundle to apply whole.",
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "inverse_bundle",
                packet: "runx.fs.inverse_bundle.v1",
            },
            fields: INVERSE_FIELDS,
        },
        inverse,
    );
