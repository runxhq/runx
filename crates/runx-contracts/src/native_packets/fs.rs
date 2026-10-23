//! What the native file tools return, and the bundle of changes
//! `fs.apply_bundle` takes and `fs.inverse` returns.

use std::num::NonZeroU64;

use serde::{Deserialize, Serialize};

use crate::Digest;
use crate::schema::{self, BoundedU64, BoundedVec, NonEmptyString, NonEmptyVec, RunxSchema};

/// The most files one `fs.read_bundle` call reads.
pub const FS_BUNDLE_MAX_FILES: usize = 16;
/// The most bytes one `fs.read_bundle` call reads across its files.
pub const FS_BUNDLE_MAX_BYTES: u64 = 32 * 1024 * 1024;
/// The most entries one `fs.list` call returns.
pub const FS_LIST_MAX_ENTRIES: usize = 5_000;
/// The most matches one `fs.search` call returns.
pub const FS_SEARCH_MAX_MATCHES: usize = 1_000;
/// The most lines of context `fs.search` returns on each side of a match.
pub const FS_SEARCH_MAX_CONTEXT_LINES: usize = 3;

/// What `fs.read` returns: the lines read from one file, with the whole
/// file's size, line count and digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.file_read.v1")]
pub struct FileRead {
    pub path: NonEmptyString,
    pub repo_root: NonEmptyString,
    /// The lines read, from `start_line`, with their line endings.
    pub contents: String,
    /// The whole file's size.
    pub bytes: u64,
    /// The file goes on past the lines read.
    pub truncated: bool,
    /// The whole file's digest, which a write of it names.
    pub content_digest: Digest,
    /// The first line read, counting from 1.
    pub start_line: NonZeroU64,
    /// How many lines the whole file has.
    pub total_lines: u64,
}

/// What `fs.read_bundle` returns: each file it read whole, and each it could
/// not read when the call asked for the missing to be reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.file_read_bundle.v1")]
pub struct FileReadBundle {
    pub repo_root: NonEmptyString,
    pub file_count: BoundedU64<0, { FS_BUNDLE_MAX_FILES as u64 }>,
    pub total_bytes: BoundedU64<0, FS_BUNDLE_MAX_BYTES>,
    pub files: BoundedVec<FileRead, 0, FS_BUNDLE_MAX_FILES>,
    pub missing: BoundedVec<MissingFile, 0, FS_BUNDLE_MAX_FILES>,
}

/// A file `fs.read_bundle` could not read, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct MissingFile {
    pub path: NonEmptyString,
    pub reason: NonEmptyString,
}

/// What `fs.write` returns: the file written, and what it held before.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.file_write.v1")]
pub struct FileWrite {
    pub path: NonEmptyString,
    pub repo_root: NonEmptyString,
    pub bytes_written: u64,
    /// The digest of what the file now holds.
    pub content_digest: Digest,
    /// What the file held before, when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<Digest>,
    /// The unified diff from what the file held to what it holds now; empty
    /// when the write left the file as it was.
    pub diff: String,
    /// The run's journal entry holding what the file held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<NonZeroU64>,
}

/// What `fs.edit` returns: how many places the edits replaced, and the
/// file's digest before and after.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.file_edit.v1")]
pub struct FileEdit {
    pub path: NonEmptyString,
    pub repo_root: NonEmptyString,
    pub replacements: NonZeroU64,
    /// The digest of the content the edits replaced.
    pub before_digest: Digest,
    /// The digest of what the file now holds.
    pub content_digest: Digest,
    pub bytes_written: u64,
    /// The unified diff from what the file held to what it holds now.
    pub diff: String,
    /// The run's journal entry holding what the file held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<NonZeroU64>,
}

/// What `fs.apply_bundle` returns: each file it wrote and each it deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.apply_bundle.v1")]
pub struct FileApplyBundle {
    pub repo_root: NonEmptyString,
    pub write_count: u64,
    pub delete_count: u64,
    pub writes: Vec<FileAppliedWrite>,
    pub deletes: Vec<FileAppliedDelete>,
    /// The run's journal entry holding what each file held before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub journal: Option<NonZeroU64>,
}

/// One file `fs.apply_bundle` wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FileAppliedWrite {
    pub path: NonEmptyString,
    pub bytes_written: u64,
    /// The digest of what the file now holds.
    pub content_digest: Digest,
    /// What the file held before, when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<Digest>,
    /// The unified diff from what the file held to what it holds now.
    pub diff: String,
}

/// One file `fs.apply_bundle` deleted, or found already absent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FileAppliedDelete {
    pub path: NonEmptyString,
    pub status: FileDeleteStatus,
    /// What the file held before, when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<Digest>,
    /// The unified diff from what the file held to its removal; empty when
    /// it was already absent.
    pub diff: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileDeleteStatus {
    Deleted,
    AlreadyAbsent,
}

/// What `fs.list` returns: the entries under a path, to a depth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.file_list.v1")]
pub struct FileList {
    pub repo_root: NonEmptyString,
    pub path: NonEmptyString,
    pub entries: BoundedVec<FileListEntry, 0, FS_LIST_MAX_ENTRIES>,
    pub truncated: bool,
}

/// One entry `fs.list` found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FileListEntry {
    pub path: NonEmptyString,
    pub kind: FileEntryKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileEntryKind {
    File,
    Directory,
    Symlink,
}

/// What `fs.search` returns: each matching line, with its context and its
/// file's digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.file_search.v1")]
pub struct FileSearch {
    pub repo_root: NonEmptyString,
    pub matches: BoundedVec<FileSearchMatch, 0, FS_SEARCH_MAX_MATCHES>,
    pub files_searched: u64,
    pub truncated: bool,
}

/// One line `fs.search` matched.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FileSearchMatch {
    pub path: NonEmptyString,
    pub line: NonZeroU64,
    pub text: String,
    pub before: BoundedVec<String, 0, FS_SEARCH_MAX_CONTEXT_LINES>,
    pub after: BoundedVec<String, 0, FS_SEARCH_MAX_CONTEXT_LINES>,
    /// The whole file's digest, which a write of it names.
    pub content_digest: Digest,
}

/// What `fs.inverse` returns: the bundle that undoes a run's edits, for
/// `fs.apply_bundle` to apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.fs.inverse_bundle.v1")]
pub struct FileInverseBundle {
    /// The root the edits changed, relative to the workspace.
    pub repo_root: NonEmptyString,
    pub writes: Vec<FileBundleWrite>,
    pub deletes: Vec<FileBundleDelete>,
    /// The calls it undoes, in the order the run made them.
    pub calls: NonEmptyVec<NonEmptyString>,
}

/// A file a bundle writes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FileBundleWrite {
    pub path: NonEmptyString,
    pub contents: String,
    /// The digest of the content it replaces, as a read returned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_digest: Option<Digest>,
    /// The file must not exist yet.
    #[serde(default, skip_serializing_if = "schema::is_default")]
    pub create: bool,
}

/// A file a bundle deletes: its path, or its path with the digest of the
/// content it removes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(untagged)]
pub enum FileBundleDelete {
    Path(NonEmptyString),
    Bound(FileBoundDelete),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct FileBoundDelete {
    pub path: NonEmptyString,
    pub expected_digest: Digest,
}
