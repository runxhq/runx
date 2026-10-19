//! Native bounded filesystem reads.

use std::path::Path;

use runx_contracts::{JsonNumber, JsonValue};

use super::capability::decode_typed_output;
use super::{NativeInvocation, invalid_input, required_string, resolve_repo_root_for};
use crate::RuntimeError;
use crate::capability::LocalEffect;
use crate::filesystem::{
    Expected, Journal, TextBundle, TextFileWrite, apply_text_bundle, text_bundle_effect,
};
use crate::services::WorkspaceBounds;

mod capability;
#[cfg(test)]
mod edit_tests;
mod inverse;
#[cfg(test)]
mod read_tests;
#[cfg(test)]
mod test_support;
mod walk;

pub(super) use capability::CAPABILITIES;
use capability::{
    BundleDelete, FileApplyBundleInput, FileApplyBundleOutput, FileEditInput, FileEditOutput,
    FileReadBundleInput, FileReadBundleOutput, FileReadInput, FileReadOutput, FileWriteInput,
    FileWriteOutput, MissingFile,
};

const READ_TOOL: &str = "fs.read";
const READ_BUNDLE_TOOL: &str = "fs.read_bundle";
const WRITE_TOOL: &str = "fs.write";
const EDIT_TOOL: &str = "fs.edit";
const APPLY_BUNDLE_TOOL: &str = "fs.apply_bundle";
const MAX_FILE_READ_BYTES: usize = 8 * 1024 * 1024;
const MAX_BUNDLE_FILES: usize = 16;
const MAX_FILE_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

fn read(invocation: &NativeInvocation<'_, FileReadInput>) -> Result<FileReadOutput, RuntimeError> {
    let inputs = invocation.inputs;
    let root = root(READ_TOOL, &inputs.repo_root, &inputs.path_scope, invocation)?;
    if inputs.start_line == 0 {
        return Err(invalid_input(READ_TOOL, "start_line counts from 1"));
    }
    if inputs.line_count == 0 {
        return Err(invalid_input(READ_TOOL, "line_count is at least 1"));
    }
    read_one(
        READ_TOOL,
        &root,
        &inputs.path,
        LineRange {
            start_line: inputs.start_line,
            line_count: inputs.line_count,
        },
        max_bytes(READ_TOOL, inputs.max_bytes)?,
        invocation.bounds,
    )
}

/// The lines a read returns: from `start_line`, counting from 1, at most `line_count` of them.
#[derive(Clone, Copy, Debug)]
struct LineRange {
    start_line: u64,
    line_count: u64,
}

impl LineRange {
    /// Every line: a bundle's files and an edit's file are read whole.
    const WHOLE: Self = Self {
        start_line: 1,
        line_count: u64::MAX,
    };
}

/// The lines of `text` a read returns within `max_bytes`, whether the file
/// goes on past them, and how many lines it has. A read stops at the last
/// whole line within its bound; a first line longer than the bound comes
/// back cut at a character boundary, never as nothing.
fn select_lines(text: &str, range: LineRange, max_bytes: usize) -> (String, bool, u64) {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let total = lines.len() as u64;
    let first = usize::try_from(range.start_line.saturating_sub(1)).unwrap_or(usize::MAX);
    let mut contents = String::new();
    let mut taken = 0_u64;
    for line in lines.iter().skip(first) {
        if taken == range.line_count {
            break;
        }
        if contents.len() + line.len() > max_bytes {
            if taken == 0 {
                let mut end = max_bytes;
                while !line.is_char_boundary(end) {
                    end -= 1;
                }
                contents.push_str(&line[..end]);
            }
            return (contents, true, total);
        }
        contents.push_str(line);
        taken += 1;
    }
    let next = (first as u64).saturating_add(taken);
    (contents, next < total, total)
}

fn read_bundle(
    invocation: &NativeInvocation<'_, FileReadBundleInput>,
) -> Result<FileReadBundleOutput, RuntimeError> {
    let root = root(
        READ_BUNDLE_TOOL,
        &invocation.inputs.repo_root,
        &invocation.inputs.path_scope,
        invocation,
    )?;
    let paths = bundle_paths(&invocation.inputs.paths)?;
    let max_bytes = max_bytes(READ_BUNDLE_TOOL, invocation.inputs.max_bytes)?;
    let report_missing = report_missing(&invocation.inputs.on_missing)?;
    let (files, missing, total_bytes) =
        read_bundle_files(&root, paths, max_bytes, report_missing, invocation.bounds)?;
    Ok(FileReadBundleOutput {
        repo_root: root.to_string_lossy().into_owned(),
        file_count: files.len() as u64,
        total_bytes,
        files,
        missing,
    })
}

fn bundle_paths(paths: &[String]) -> Result<&[String], RuntimeError> {
    if paths.len() > MAX_BUNDLE_FILES {
        return Err(invalid_input(
            READ_BUNDLE_TOOL,
            format!("paths must contain at most {MAX_BUNDLE_FILES} entries"),
        ));
    }
    Ok(paths)
}

fn report_missing(value: &str) -> Result<bool, RuntimeError> {
    match value {
        "error" => Ok(false),
        "report" => Ok(true),
        _ => Err(invalid_input(
            READ_BUNDLE_TOOL,
            "on_missing must be error or report",
        )),
    }
}

fn read_bundle_files(
    root: &Path,
    paths: &[String],
    max_bytes: usize,
    report_missing: bool,
    bounds: WorkspaceBounds,
) -> Result<(Vec<FileReadOutput>, Vec<MissingFile>, u64), RuntimeError> {
    let mut seen = std::collections::BTreeSet::new();
    let mut total_bytes = 0u64;
    let mut files = Vec::with_capacity(paths.len());
    let mut missing = Vec::new();
    for path in paths {
        if !seen.insert(path) {
            return Err(invalid_input(
                READ_BUNDLE_TOOL,
                format!("paths contains a duplicate: {path}"),
            ));
        }
        let file = match read_one(
            READ_BUNDLE_TOOL,
            root,
            path,
            LineRange::WHOLE,
            max_bytes,
            bounds,
        ) {
            Ok(file) => file,
            Err(_) if report_missing => {
                missing.push(MissingFile {
                    path: path.to_owned(),
                    reason: "file was unavailable within the declared root and read limits"
                        .to_owned(),
                });
                continue;
            }
            Err(error) => return Err(error),
        };
        let bytes = file.bytes;
        total_bytes = total_bytes
            .checked_add(bytes)
            .ok_or_else(|| invalid_input(READ_BUNDLE_TOOL, "bundle byte count overflow"))?;
        if total_bytes > MAX_FILE_BUNDLE_BYTES {
            return Err(invalid_input(
                READ_BUNDLE_TOOL,
                format!("bundle exceeds {MAX_FILE_BUNDLE_BYTES} bytes"),
            ));
        }
        files.push(file);
    }
    Ok((files, missing, total_bytes))
}

/// Read `range` of one file within `max_bytes`. The file is read whole,
/// within the native bound, so its digest binds a later write and its line
/// count is known; a larger file is refused, never returned empty.
fn read_one(
    tool: &str,
    root: &Path,
    requested: &str,
    range: LineRange,
    max_bytes: usize,
    bounds: WorkspaceBounds,
) -> Result<FileReadOutput, RuntimeError> {
    let file = crate::services::WorkspaceFile::resolve(root, Path::new(requested), bounds)
        .map_err(|error| invalid_input(tool, error.to_string()))?;
    let file = file
        .read_text(MAX_FILE_READ_BYTES as u64)
        .map_err(|error| invalid_input(tool, error.to_string()))?;
    if file.truncated {
        return Err(invalid_input(
            tool,
            format!(
                "{} is {} bytes, more than a read takes ({MAX_FILE_READ_BYTES}); search it with fs.search",
                file.relative_path, file.bytes
            ),
        ));
    }
    let (contents, truncated, total_lines) = select_lines(&file.contents, range, max_bytes);
    Ok(FileReadOutput {
        path: file.relative_path,
        repo_root: root.to_string_lossy().into_owned(),
        contents,
        bytes: file.bytes,
        truncated,
        content_digest: file.digest,
        start_line: range.start_line,
        total_lines,
    })
}

fn write(
    invocation: &NativeInvocation<'_, FileWriteInput>,
) -> Result<FileWriteOutput, RuntimeError> {
    let root = resolve_repo_root_for(
        WRITE_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let report = apply_text_bundle(
        WRITE_TOOL,
        &root,
        &write_bundle(invocation.inputs)?,
        invocation.bounds,
        Journal::of_run(invocation.env, invocation.skill_directory)?.as_ref(),
    )?;
    let write = first_write(WRITE_TOOL, &report)?;
    let normalized_path = required_string(WRITE_TOOL, write, "path")?;
    let bytes_written = write
        .get("bytes_written")
        .cloned()
        .ok_or_else(|| invalid_input(WRITE_TOOL, "filesystem transaction omitted byte count"))?;
    let sha256 = required_string(WRITE_TOOL, write, "sha256")?
        .strip_prefix("sha256:")
        .ok_or_else(|| {
            invalid_input(
                WRITE_TOOL,
                "filesystem transaction returned an invalid digest",
            )
        })?;
    let bytes_written = match bytes_written {
        JsonValue::Number(JsonNumber::U64(value)) => value,
        _ => {
            return Err(invalid_input(
                WRITE_TOOL,
                "filesystem transaction returned an invalid byte count",
            ));
        }
    };
    Ok(FileWriteOutput {
        path: normalized_path.to_owned(),
        repo_root: root.to_string_lossy().into_owned(),
        bytes_written,
        sha256: sha256.to_owned(),
        before_digest: write
            .get("before_digest")
            .and_then(JsonValue::as_str)
            .map(str::to_owned),
        diff: diff_of(WRITE_TOOL, write)?,
        journal: journal_of(&report),
    })
}

/// A reported change's diff, exactly as it was computed: empty when the
/// change left the file as it was.
fn diff_of(tool: &str, change: &runx_contracts::JsonObject) -> Result<String, RuntimeError> {
    change
        .get("diff")
        .and_then(JsonValue::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid_input(tool, "filesystem transaction omitted its diff"))
}

/// The write a one-file bundle report proves.
fn first_write<'a>(
    tool: &str,
    report: &'a runx_contracts::JsonObject,
) -> Result<&'a runx_contracts::JsonObject, RuntimeError> {
    report
        .get("writes")
        .and_then(JsonValue::as_array)
        .and_then(|writes| writes.first())
        .and_then(JsonValue::as_object)
        .ok_or_else(|| invalid_input(tool, "filesystem transaction omitted write proof"))
}

/// The run's journal entry a bundle report names.
fn journal_of(report: &runx_contracts::JsonObject) -> Option<u64> {
    match report.get("journal") {
        Some(JsonValue::Number(JsonNumber::U64(journal))) => Some(*journal),
        _ => None,
    }
}

/// What a change names it was written against: the digest it replaces, or
/// that the file is new.
fn expectation(
    tool: &str,
    path: &str,
    expected_digest: Option<&String>,
    create: bool,
) -> Result<Option<Expected>, RuntimeError> {
    match (expected_digest, create) {
        (Some(_), true) => Err(invalid_input(
            tool,
            format!("{path} names an expected_digest or create, not both"),
        )),
        (Some(digest), false) => Ok(Some(Expected::Digest(digest.clone()))),
        (None, true) => Ok(Some(Expected::Absent)),
        (None, false) => Ok(None),
    }
}

/// The one-file bundle `fs.write` applies.
fn write_bundle(inputs: &FileWriteInput) -> Result<TextBundle, RuntimeError> {
    Ok(TextBundle {
        writes: vec![TextFileWrite {
            path: inputs.path.clone(),
            contents: inputs.contents.clone(),
        }],
        deletes: Vec::new(),
        expected: expectation(
            WRITE_TOOL,
            &inputs.path,
            inputs.expected_digest.as_ref(),
            inputs.create,
        )?
        .map(|expected| (inputs.path.clone(), expected))
        .into_iter()
        .collect(),
    })
}

/// What `fs.write` would change.
fn write_effect(
    invocation: &NativeInvocation<'_, FileWriteInput>,
) -> Result<Option<LocalEffect>, RuntimeError> {
    let root = resolve_repo_root_for(
        WRITE_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    text_bundle_effect(
        WRITE_TOOL,
        &root,
        &write_bundle(invocation.inputs)?,
        invocation.bounds,
    )
}

/// What `fs.edit` makes of its file: the file as it was read, and the text
/// its edits produce, applied in order. Each `old` must occur once, or at
/// least once with `replace_all`; anything else is refused with why.
struct Edited {
    root: std::path::PathBuf,
    path: String,
    before_digest: String,
    contents: String,
    replacements: u64,
}

fn edited(invocation: &NativeInvocation<'_, FileEditInput>) -> Result<Edited, RuntimeError> {
    let inputs = invocation.inputs;
    let root = resolve_repo_root_for(
        EDIT_TOOL,
        &inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let file = read_one(
        EDIT_TOOL,
        &root,
        &inputs.path,
        LineRange::WHOLE,
        MAX_FILE_READ_BYTES,
        invocation.bounds,
    )?;
    if inputs.edits.is_empty() {
        return Err(invalid_input(EDIT_TOOL, "edits names at least one edit"));
    }
    let mut contents = file.contents;
    let mut replacements = 0_u64;
    for (index, edit) in inputs.edits.iter().enumerate() {
        if edit.old.is_empty() {
            return Err(invalid_input(
                EDIT_TOOL,
                format!("edit {index} names no old text"),
            ));
        }
        if edit.old == edit.new {
            return Err(invalid_input(
                EDIT_TOOL,
                format!("edit {index} changes nothing"),
            ));
        }
        let found = contents.matches(edit.old.as_str()).count();
        contents = match (found, edit.replace_all) {
            (0, _) => {
                return Err(invalid_input(
                    EDIT_TOOL,
                    format!("edit {index}: its old text is not in {}", file.path),
                ));
            }
            (1, _) | (_, true) => contents.replace(edit.old.as_str(), &edit.new),
            (found, false) => {
                return Err(invalid_input(
                    EDIT_TOOL,
                    format!(
                        "edit {index}: its old text occurs {found} times in {}; make it unique or set replace_all",
                        file.path
                    ),
                ));
            }
        };
        replacements += found as u64;
    }
    Ok(Edited {
        root,
        path: file.path,
        before_digest: file.content_digest,
        contents,
        replacements,
    })
}

/// The one-file bundle an edit applies, bound to the content it was read as.
fn edit_bundle(edited: &Edited) -> TextBundle {
    TextBundle {
        writes: vec![TextFileWrite {
            path: edited.path.clone(),
            contents: edited.contents.clone(),
        }],
        deletes: Vec::new(),
        expected: std::collections::BTreeMap::from([(
            edited.path.clone(),
            Expected::Digest(edited.before_digest.clone()),
        )]),
    }
}

fn edit(invocation: &NativeInvocation<'_, FileEditInput>) -> Result<FileEditOutput, RuntimeError> {
    let edited = edited(invocation)?;
    let report = apply_text_bundle(
        EDIT_TOOL,
        &edited.root,
        &edit_bundle(&edited),
        invocation.bounds,
        Journal::of_run(invocation.env, invocation.skill_directory)?.as_ref(),
    )?;
    Ok(FileEditOutput {
        path: edited.path,
        repo_root: edited.root.to_string_lossy().into_owned(),
        replacements: edited.replacements,
        before_digest: edited.before_digest,
        content_digest: runx_contracts::sha256_prefixed(edited.contents.as_bytes()),
        bytes_written: edited.contents.len() as u64,
        diff: diff_of(EDIT_TOOL, first_write(EDIT_TOOL, &report)?)?,
        journal: journal_of(&report),
    })
}

/// What `fs.edit` would change.
fn edit_effect(
    invocation: &NativeInvocation<'_, FileEditInput>,
) -> Result<Option<LocalEffect>, RuntimeError> {
    let edited = edited(invocation)?;
    text_bundle_effect(
        EDIT_TOOL,
        &edited.root,
        &edit_bundle(&edited),
        invocation.bounds,
    )
}

/// The bundle `fs.apply_bundle` applies.
fn files_bundle(inputs: &FileApplyBundleInput) -> Result<TextBundle, RuntimeError> {
    let mut expected = std::collections::BTreeMap::new();
    for write in &inputs.writes {
        if let Some(expectation) = expectation(
            APPLY_BUNDLE_TOOL,
            &write.path,
            write.expected_digest.as_ref(),
            write.create,
        )? {
            expected.insert(write.path.clone(), expectation);
        }
    }
    let mut deletes = Vec::with_capacity(inputs.deletes.len());
    for delete in &inputs.deletes {
        match delete {
            BundleDelete::Path(path) => deletes.push(path.clone()),
            BundleDelete::Bound(bound) => {
                expected.insert(
                    bound.path.clone(),
                    Expected::Digest(bound.expected_digest.clone()),
                );
                deletes.push(bound.path.clone());
            }
        }
    }
    Ok(TextBundle {
        writes: inputs
            .writes
            .iter()
            .map(|write| TextFileWrite {
                path: write.path.clone(),
                contents: write.contents.clone(),
            })
            .collect(),
        deletes,
        expected,
    })
}

fn apply_files(
    invocation: &NativeInvocation<'_, FileApplyBundleInput>,
) -> Result<FileApplyBundleOutput, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        APPLY_BUNDLE_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    let report = apply_text_bundle(
        APPLY_BUNDLE_TOOL,
        &repo_root,
        &files_bundle(invocation.inputs)?,
        invocation.bounds,
        Journal::of_run(invocation.env, invocation.skill_directory)?.as_ref(),
    )?;
    decode_typed_output(APPLY_BUNDLE_TOOL, JsonValue::Object(report))
}

/// What `fs.apply_bundle` would change.
fn files_effect(
    invocation: &NativeInvocation<'_, FileApplyBundleInput>,
) -> Result<Option<LocalEffect>, RuntimeError> {
    let repo_root = resolve_repo_root_for(
        APPLY_BUNDLE_TOOL,
        &invocation.inputs.repo_root,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )?;
    text_bundle_effect(
        APPLY_BUNDLE_TOOL,
        &repo_root,
        &files_bundle(invocation.inputs)?,
        invocation.bounds,
    )
}

pub(super) fn root<I: ?Sized>(
    tool: &str,
    repo_root: &str,
    path_scope: &str,
    invocation: &NativeInvocation<'_, I>,
) -> Result<std::path::PathBuf, RuntimeError> {
    crate::services::resolve_scoped_root(
        repo_root,
        path_scope,
        invocation.env,
        invocation.skill_directory,
        invocation.bounds,
    )
    .map_err(|error| invalid_input(tool, error.to_string()))
}

fn max_bytes(tool: &str, value: u64) -> Result<usize, RuntimeError> {
    let value =
        usize::try_from(value).map_err(|_| invalid_input(tool, "max_bytes is too large"))?;
    if value == 0 || value > MAX_FILE_READ_BYTES {
        return Err(invalid_input(
            tool,
            format!("max_bytes must be 1-{MAX_FILE_READ_BYTES}"),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests;
