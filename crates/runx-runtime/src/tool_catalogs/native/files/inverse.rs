//! The inverse of a run's edits (agent-sessions §4.5): `fs.inverse` reads
//! the edit calls a run made, by `call_id`, and returns the bundle that
//! restores what each file held before them, bound to what it holds after
//! them. `revert` applies it with `fs.apply_bundle`, all or none, so a file
//! changed since is refused, naming it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use runx_contracts::schema::{NonEmptyString, NonEmptyVec};
use runx_contracts::{
    Digest, FileBoundDelete, FileBundleDelete, FileBundleWrite, FileInverseBundle, JsonObject,
    JsonValue, RunCallOutcome, RunEventFact, TurnTarget,
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

/// One edit call, as the run recorded it.
struct Edit {
    index: u32,
    call_id: String,
    output: JsonObject,
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
    let edits = recorded_edits(invocation.env, &workspace, inputs)?;
    let store = crate::kernel::store::KernelStore::for_project(&paths)
        .map_err(|error| refused(error.to_string()))?;
    // Per file: what it held before the first edit, and what it holds after
    // the last.
    let mut root: Option<PathBuf> = None;
    let mut before = BTreeMap::<String, Option<Vec<u8>>>::new();
    let mut after = BTreeMap::<String, Option<String>>::new();
    for edit in &edits {
        let bundle = match edit.output.get("journal") {
            Some(JsonValue::Number(runx_contracts::JsonNumber::U64(bundle))) => Some(*bundle),
            Some(JsonValue::Number(runx_contracts::JsonNumber::I64(bundle))) => {
                u64::try_from(*bundle).ok()
            }
            _ => None,
        }
        .ok_or_else(|| refused(format!("call {} journaled no change", edit.call_id)))?;
        let images = crate::filesystem::applied_bundle(&store, &inputs.run_id, bundle)?;
        match &root {
            Some(root) if *root != images.root => {
                return Err(refused(format!(
                    "call {} changed {}, and another call {}; one revert restores one root",
                    edit.call_id,
                    images.root.display(),
                    root.display()
                )));
            }
            Some(_) => {}
            None => root = Some(images.root.clone()),
        }
        for (path, held) in images.files {
            before.entry(path).or_insert(held);
        }
        for (path, digest) in after_digests(&edit.output) {
            after.insert(path, digest);
        }
    }
    let root = root.ok_or_else(|| refused("the calls changed nothing".to_owned()))?;
    let repo_root = root
        .strip_prefix(&workspace)
        .map(|relative| match relative.to_string_lossy().into_owned() {
            relative if relative.is_empty() => ".".to_owned(),
            relative => relative,
        })
        .map_err(|_| refused(format!("{} lies outside the workspace", root.display())))?;
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
        match (held, now) {
            (Some(contents), now) => writes.push(FileBundleWrite {
                contents: String::from_utf8(contents)
                    .map_err(|_| refused(format!("{path} held content that is not text")))?,
                create: now.is_none(),
                expected_digest: now,
                path: named(path, "path")?,
            }),
            (None, Some(digest)) => deletes.push(FileBundleDelete::Bound(FileBoundDelete {
                path: named(path, "path")?,
                expected_digest: digest,
            })),
            (None, None) => {}
        }
    }
    let calls = edits
        .into_iter()
        .map(|edit| named(edit.call_id, "call id"))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(FileInverseBundle {
        repo_root: named(repo_root, "root")?,
        writes,
        deletes,
        calls: NonEmptyVec::new(calls)
            .ok_or_else(|| refused("the calls changed nothing".to_owned()))?,
    })
}

/// The completed edit calls of run `inputs.run_id` that `inputs.call_ids`
/// name, in the order the run made them, each with its recorded output.
fn recorded_edits(
    env: &BTreeMap<String, String>,
    workspace: &Path,
    inputs: &FileInverseInput,
) -> Result<Vec<Edit>, RuntimeError> {
    let refused = |message: String| super::super::invalid_input(INVERSE_TOOL, message);
    let wanted = inputs
        .call_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    // A call is recorded as each of its outcomes; its last is how it ended.
    let mut latest = BTreeMap::<(String, u32), (Option<String>, String, RunCallOutcome)>::new();
    let mut cursor = None;
    loop {
        let page = crate::execution::run_record::read_run(
            env,
            workspace,
            &inputs.run_id,
            cursor.as_deref(),
        )
        .map_err(|error| refused(error.to_string()))?;
        for record in page.events {
            if record.stream.as_str() != inputs.run_id {
                continue;
            }
            let RunEventFact::Call {
                turn,
                index,
                target: TurnTarget::Tool { tool },
                call_id,
                outcome,
                ..
            } = record.event
            else {
                continue;
            };
            latest.insert(
                (turn.as_str().to_owned(), index),
                (
                    call_id.map(|id| id.as_str().to_owned()),
                    tool.as_str().to_owned(),
                    *outcome,
                ),
            );
        }
        if !page.more {
            break;
        }
        cursor = Some(page.cursor.as_str().to_owned());
    }
    let artifacts = RunxPaths::resolve(env, workspace).artifacts();
    let mut edits = Vec::new();
    let mut seen = BTreeSet::new();
    for ((_, index), (call_id, tool, outcome)) in latest {
        let Some(call_id) = call_id.filter(|id| wanted.contains(id.as_str())) else {
            continue;
        };
        if !seen.insert(call_id.clone()) {
            return Err(refused(format!(
                "run {} made more than one call {call_id}",
                inputs.run_id
            )));
        }
        let Some((_, packet)) = EDITS.iter().find(|(edit, _)| *edit == tool) else {
            return Err(refused(format!(
                "call {call_id} is a {tool} call, not an edit"
            )));
        };
        let RunCallOutcome::Completed {
            succeeded: true,
            outputs,
            ..
        } = outcome
        else {
            return Err(refused(format!("call {call_id} did not complete")));
        };
        let artifact = outputs
            .iter()
            .find(|output| {
                output
                    .output
                    .as_ref()
                    .is_some_and(|name| name.as_str() == *packet)
            })
            .ok_or_else(|| refused(format!("call {call_id} recorded no {packet}")))?;
        let reference = JsonValue::from_serialize(&artifact.artifact)
            .map_err(|error| refused(error.to_string()))?;
        let output = match crate::services::read_json_artifact(&artifacts, &reference)
            .map_err(|message| refused(format!("call {call_id}: {message}")))?
        {
            JsonValue::Object(object) => match object.get("data") {
                Some(JsonValue::Object(data)) => data.clone(),
                _ => object,
            },
            _ => return Err(refused(format!("call {call_id} recorded no object"))),
        };
        edits.push(Edit {
            index,
            call_id,
            output,
        });
    }
    if let Some(missing) = wanted
        .iter()
        .find(|id| !edits.iter().any(|edit| edit.call_id == **id))
    {
        return Err(refused(format!(
            "run {} made no call {missing}",
            inputs.run_id
        )));
    }
    edits.sort_by_key(|edit| edit.index);
    Ok(edits)
}

/// What each file an edit's output names holds after it: its
/// `content_digest`, or none for a file it deleted.
fn after_digests(output: &JsonObject) -> Vec<(String, Option<String>)> {
    let text = |value: &JsonValue, field: &str| {
        value
            .as_object()
            .and_then(|object| object.get(field))
            .and_then(JsonValue::as_str)
            .map(str::to_owned)
    };
    let value = JsonValue::Object(output.clone());
    let mut files = Vec::new();
    if let Some(path) = text(&value, "path") {
        files.push((path, text(&value, "content_digest")));
    }
    for write in output
        .get("writes")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(path) = text(write, "path") {
            files.push((path, text(write, "content_digest")));
        }
    }
    for delete in output
        .get("deletes")
        .and_then(JsonValue::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(path) = text(delete, "path") {
            files.push((path, None));
        }
    }
    files
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
        description: "The call ids of its fs.write, fs.edit and fs.apply_bundle calls to undo, together.",
    },
];

pub(super) static INVERSE: TypedNativeCapability<FileInverseInput, FileInverseBundle> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: INVERSE_TOOL,
            source: "runx-runtime/fs",
            summary: "The bundle that undoes a run's edit calls: what each file held before them, bound to what it holds after them, for fs.apply_bundle to apply whole.",
            scopes: &["fs.read"],
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
