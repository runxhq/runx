//! Native bounded filesystem reads.

use std::path::Path;

use runx_contracts::{JsonNumber, JsonValue};

use super::capability::decode_typed_output;
use super::{NativeInvocation, invalid_input, required_string, resolve_repo_root_for};
use crate::RuntimeError;
use crate::filesystem::{TextBundle, TextFileWrite, apply_text_bundle};
use crate::services::WorkspaceBounds;

mod capability;
mod walk;

pub(super) use capability::CAPABILITIES;
use capability::{
    FileApplyBundleInput, FileApplyBundleOutput, FileReadBundleInput, FileReadBundleOutput,
    FileReadInput, FileReadOutput, FileWriteInput, FileWriteOutput, MissingFile,
};

const READ_TOOL: &str = "fs.read";
const READ_BUNDLE_TOOL: &str = "fs.read_bundle";
const WRITE_TOOL: &str = "fs.write";
const APPLY_BUNDLE_TOOL: &str = "fs.apply_bundle";
const MAX_FILE_READ_BYTES: usize = 8 * 1024 * 1024;
const MAX_BUNDLE_FILES: usize = 16;
const MAX_FILE_BUNDLE_BYTES: u64 = 32 * 1024 * 1024;

fn read(invocation: &NativeInvocation<'_, FileReadInput>) -> Result<FileReadOutput, RuntimeError> {
    let root = root(
        READ_TOOL,
        &invocation.inputs.repo_root,
        &invocation.inputs.path_scope,
        invocation,
    )?;
    let file = read_one(
        READ_TOOL,
        &root,
        &invocation.inputs.path,
        max_bytes(READ_TOOL, invocation.inputs.max_bytes)?,
        invocation.bounds,
    )?;
    Ok(file)
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
        let file = match read_one(READ_BUNDLE_TOOL, root, path, max_bytes, bounds) {
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

fn read_one(
    tool: &str,
    root: &Path,
    requested: &str,
    max_bytes: usize,
    bounds: WorkspaceBounds,
) -> Result<FileReadOutput, RuntimeError> {
    let file = crate::services::WorkspaceFile::resolve(root, Path::new(requested), bounds)
        .map_err(|error| invalid_input(tool, error.to_string()))?;
    let file = file
        .read_text(max_bytes as u64)
        .map_err(|error| invalid_input(tool, error.to_string()))?;
    Ok(FileReadOutput {
        path: file.relative_path,
        repo_root: root.to_string_lossy().into_owned(),
        contents: file.contents,
        bytes: file.bytes,
        truncated: file.truncated,
        content_digest: file.digest,
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
        &TextBundle {
            writes: vec![TextFileWrite {
                path: invocation.inputs.path.clone(),
                contents: invocation.inputs.contents.clone(),
            }],
            deletes: Vec::new(),
        },
        invocation.bounds,
    )?;
    let write = report
        .get("writes")
        .and_then(JsonValue::as_array)
        .and_then(|writes| writes.first())
        .and_then(JsonValue::as_object)
        .ok_or_else(|| invalid_input(WRITE_TOOL, "filesystem transaction omitted write proof"))?;
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
    let writes = invocation
        .inputs
        .writes
        .iter()
        .map(|write| TextFileWrite {
            path: write.path.clone(),
            contents: write.contents.clone(),
        })
        .collect();
    let report = apply_text_bundle(
        APPLY_BUNDLE_TOOL,
        &repo_root,
        &TextBundle {
            writes,
            deletes: invocation.inputs.deletes.clone(),
        },
        invocation.bounds,
    )?;
    decode_typed_output(APPLY_BUNDLE_TOOL, JsonValue::Object(report))
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
