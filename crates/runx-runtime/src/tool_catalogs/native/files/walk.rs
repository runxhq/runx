//! Native bounded listing and search, ignore-file aware.
//!
//! A listing walks a directory; a search walks one, or reads one file. Both
//! walk beneath one resolved root, honour the project's ignore files as
//! Git does, never descend into `.git` or `.runx`, and skip a nested Git
//! worktree (a directory whose `.git` is a file) unless the request names a
//! path inside it, so agents that keep worktrees under the project do not see
//! every file twice. Within agent bounds a secret file is never listed or
//! searched.

use std::io::Read as _;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};

use runx_contracts::schema::BoundedVec;
use runx_contracts::{
    Digest, FS_LIST_MAX_ENTRIES, FS_SEARCH_MAX_CONTEXT_LINES, FS_SEARCH_MAX_MATCHES, FileEntryKind,
    FileList, FileListEntry, FileSearch, FileSearchMatch,
};

use super::super::{NativeInvocation, invalid_input, non_empty, root_name};
use super::capability::{FileListInput, FileSearchInput};
use crate::RuntimeError;
use crate::services::{WorkspaceBounds, is_secret_path};

pub(super) const LIST_TOOL: &str = "fs.list";
pub(super) const SEARCH_TOOL: &str = "fs.search";
const MAX_DEPTH: u64 = 16;
const MAX_LIST_ENTRIES: u64 = FS_LIST_MAX_ENTRIES as u64;
const MAX_SEARCH_RESULTS: u64 = FS_SEARCH_MAX_MATCHES as u64;
const MAX_CONTEXT_LINES: u64 = FS_SEARCH_MAX_CONTEXT_LINES as u64;
const MAX_SEARCHED_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_LINE_CHARS: usize = 500;
const BINARY_PROBE_BYTES: usize = 8 * 1024;

pub(super) fn list(
    invocation: &NativeInvocation<'_, FileListInput>,
) -> Result<FileList, RuntimeError> {
    let inputs = invocation.inputs;
    let root = super::root(LIST_TOOL, &inputs.repo_root, &inputs.path_scope, invocation)?;
    let depth = bounded(LIST_TOOL, "depth", inputs.depth, MAX_DEPTH)?;
    let limit = bounded(
        LIST_TOOL,
        "max_entries",
        inputs.max_entries,
        MAX_LIST_ENTRIES,
    )?;
    let start = walk_start(LIST_TOOL, &root, &inputs.path, invocation.bounds)?;
    if !start.is_dir() {
        return Err(invalid_input(LIST_TOOL, "path must be a directory"));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    let depth = usize::try_from(depth).unwrap_or(usize::MAX);
    for entry in walker(&root, &start, Some(depth), invocation.bounds) {
        let entry = entry.map_err(|error| invalid_input(LIST_TOOL, error.to_string()))?;
        if entry.depth() == 0 {
            continue;
        }
        if entries.len() as u64 == limit {
            truncated = true;
            break;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_dir() {
            FileEntryKind::Directory
        } else if file_type.is_symlink() {
            FileEntryKind::Symlink
        } else {
            FileEntryKind::File
        };
        let bytes = (kind == FileEntryKind::File)
            .then(|| entry.metadata().ok().map(|metadata| metadata.len()))
            .flatten();
        entries.push(FileListEntry {
            path: non_empty(LIST_TOOL, "path", relative(&root, entry.path()))?,
            kind,
            bytes,
        });
    }
    Ok(FileList {
        repo_root: root_name(LIST_TOOL, &root)?,
        path: non_empty(LIST_TOOL, "path", relative(&root, &start))?,
        entries: BoundedVec::new(entries)
            .ok_or_else(|| invalid_input(LIST_TOOL, "listing exceeded its entry bound"))?,
        truncated,
    })
}

pub(super) fn search(
    invocation: &NativeInvocation<'_, FileSearchInput>,
) -> Result<FileSearch, RuntimeError> {
    let inputs = invocation.inputs;
    let root = super::root(
        SEARCH_TOOL,
        &inputs.repo_root,
        &inputs.path_scope,
        invocation,
    )?;
    let limit = bounded(
        SEARCH_TOOL,
        "max_results",
        inputs.max_results,
        MAX_SEARCH_RESULTS,
    )?;
    let context = usize::try_from(inputs.context_lines)
        .ok()
        .filter(|lines| *lines as u64 <= MAX_CONTEXT_LINES)
        .ok_or_else(|| {
            invalid_input(
                SEARCH_TOOL,
                format!("context_lines must be 0-{MAX_CONTEXT_LINES}"),
            )
        })?;
    let pattern = pattern(inputs)?;
    let glob = inputs
        .glob
        .as_deref()
        .map(|glob| {
            globset::Glob::new(glob)
                .map(|glob| glob.compile_matcher())
                .map_err(|error| invalid_input(SEARCH_TOOL, format!("glob is invalid: {error}")))
        })
        .transpose()?;
    let start = walk_start(SEARCH_TOOL, &root, &inputs.path, invocation.bounds)?;
    let mut matches = Vec::new();
    let mut files_searched = 0_u64;
    let mut truncated = false;
    'files: for entry in walker(&root, &start, None, invocation.bounds) {
        let entry = entry.map_err(|error| invalid_input(SEARCH_TOOL, error.to_string()))?;
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = relative(&root, entry.path());
        if glob.as_ref().is_some_and(|glob| !glob.is_match(&path)) {
            continue;
        }
        let Some((text, digest)) = searchable_text(entry.path())? else {
            continue;
        };
        files_searched += 1;
        let lines = text.lines().collect::<Vec<_>>();
        for (index, line) in lines.iter().enumerate() {
            if !pattern.is_match(line) {
                continue;
            }
            if matches.len() as u64 == limit {
                truncated = true;
                break 'files;
            }
            let context_of = |lines: &[&str]| {
                BoundedVec::new(lines.iter().map(|line| clip(line)).collect())
                    .ok_or_else(|| invalid_input(SEARCH_TOOL, "context exceeded its line bound"))
            };
            matches.push(FileSearchMatch {
                path: non_empty(SEARCH_TOOL, "path", path.clone())?,
                line: NonZeroU64::MIN.saturating_add(index as u64),
                text: clip(line),
                before: context_of(&lines[index.saturating_sub(context)..index])?,
                after: context_of(&lines[index + 1..(index + 1 + context).min(lines.len())])?,
                content_digest: digest.clone(),
            });
        }
    }
    Ok(FileSearch {
        repo_root: root_name(SEARCH_TOOL, &root)?,
        matches: BoundedVec::new(matches)
            .ok_or_else(|| invalid_input(SEARCH_TOOL, "search exceeded its match bound"))?,
        files_searched,
        truncated,
    })
}

fn pattern(inputs: &FileSearchInput) -> Result<Regex, RuntimeError> {
    if inputs.pattern.is_empty() {
        return Err(invalid_input(SEARCH_TOOL, "pattern must not be empty"));
    }
    let source = if inputs.regex {
        inputs.pattern.clone()
    } else {
        regex::escape(&inputs.pattern)
    };
    RegexBuilder::new(&source)
        .case_insensitive(!inputs.case_sensitive)
        .size_limit(1024 * 1024)
        .build()
        .map_err(|error| invalid_input(SEARCH_TOOL, format!("pattern is invalid: {error}")))
}

/// What `requested` names under `root`, through any symlink, which must stay
/// inside it and within `bounds` both as named and as resolved.
fn walk_start(
    tool: &str,
    root: &Path,
    requested: &str,
    bounds: WorkspaceBounds,
) -> Result<PathBuf, RuntimeError> {
    let requested = Path::new(requested);
    if requested.is_absolute()
        || requested
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(invalid_input(
            tool,
            "path must be a relative path inside the root",
        ));
    }
    let admit = |path: &Path| {
        bounds
            .admit(path)
            .map_err(|error| invalid_input(tool, error.to_string()))
    };
    admit(requested)?;
    let start = std::fs::canonicalize(root.join(requested))
        .map_err(|error| invalid_input(tool, format!("path is unavailable: {error}")))?;
    admit(
        start
            .strip_prefix(root)
            .map_err(|_| invalid_input(tool, "path escapes the root"))?,
    )?;
    Ok(start)
}

/// One walk under `start`: ignore files honoured, hidden entries included, and
/// `.git`, `.runx`, nested worktrees and (within agent bounds) secrets left
/// out.
fn walker(
    root: &Path,
    start: &Path,
    depth: Option<usize>,
    bounds: WorkspaceBounds,
) -> ignore::Walk {
    let root = root.to_path_buf();
    let start_owned = start.to_path_buf();
    let mut builder = WalkBuilder::new(start);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|left, right| left.cmp(right))
        .max_depth(depth);
    builder.filter_entry(move |entry| {
        let path = entry.path();
        if path == start_owned {
            return true;
        }
        if entry.file_name() == ".git" || entry.file_name() == ".runx" {
            return false;
        }
        let is_dir = entry.file_type().is_some_and(|kind| kind.is_dir());
        if is_dir && path.join(".git").is_file() {
            return false;
        }
        let inside = path.strip_prefix(&root).unwrap_or(path);
        !(bounds == WorkspaceBounds::Agent && is_secret_path(inside))
    });
    builder.build()
}

/// A file's text and digest, or nothing for a file too large or binary to
/// search.
fn searchable_text(path: &Path) -> Result<Option<(String, Digest)>, RuntimeError> {
    let metadata = std::fs::metadata(path)
        .map_err(|source| RuntimeError::io(format!("reading {}", path.display()), source))?;
    if metadata.len() > MAX_SEARCHED_FILE_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)
        .and_then(|mut file| file.read_to_end(&mut bytes))
        .map_err(|source| RuntimeError::io(format!("reading {}", path.display()), source))?;
    if bytes[..bytes.len().min(BINARY_PROBE_BYTES)].contains(&0) {
        return Ok(None);
    }
    let digest = Digest::of(&bytes);
    Ok(String::from_utf8(bytes).ok().map(|text| (text, digest)))
}

fn bounded(tool: &str, field: &str, value: u64, maximum: u64) -> Result<u64, RuntimeError> {
    if value == 0 || value > maximum {
        return Err(invalid_input(tool, format!("{field} must be 1-{maximum}")));
    }
    Ok(value)
}

fn relative(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let text = relative
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    if text.is_empty() {
        ".".to_owned()
    } else {
        text
    }
}

fn clip(line: &str) -> String {
    if line.chars().count() <= MAX_LINE_CHARS {
        return line.to_owned();
    }
    let mut clipped = line.chars().take(MAX_LINE_CHARS).collect::<String>();
    clipped.push('…');
    clipped
}

#[cfg(test)]
mod tests;
