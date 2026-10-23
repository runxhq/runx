//! What the native git tools return.

use serde::{Deserialize, Serialize};

use crate::schema::{BoundedVec, NonEmptyString, RunxSchema};

/// The most commits one `git.log` call returns.
pub const GIT_LOG_MAX_COMMITS: usize = 200;

/// What `git.branch` reports: the branch a repository has checked out, or
/// that its HEAD is detached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.git.branch.v1")]
pub struct GitBranch {
    pub repo_root: NonEmptyString,
    pub branch: String,
    pub detached: bool,
}

/// What `git.status` reports: whether the work tree is clean, and each entry
/// of the porcelain status when it is not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.git.status.v1")]
pub struct GitStatus {
    pub repo_root: NonEmptyString,
    pub clean: bool,
    pub entries: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

/// What `git.diff` reports: the files that differ from `base`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.git.diff.v1")]
pub struct GitDiff {
    pub repo_root: NonEmptyString,
    pub base: NonEmptyString,
    pub files: Vec<String>,
}

/// What `git.log` reports: the commits reachable from `revision`, newest
/// first, and whether more were left out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.git.log.v1")]
pub struct GitLog {
    pub repo_root: NonEmptyString,
    pub revision: NonEmptyString,
    pub commits: BoundedVec<GitCommit, 0, GIT_LOG_MAX_COMMITS>,
    pub truncated: bool,
}

/// One commit as `git.log` reports it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
pub struct GitCommit {
    pub commit: NonEmptyString,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub files: Vec<NonEmptyString>,
}

/// What `git.show` reports: one revision, or one file at it, as bounded text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.git.show.v1")]
pub struct GitShow {
    pub repo_root: NonEmptyString,
    pub revision: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<NonEmptyString>,
    pub text: String,
    pub files: Vec<NonEmptyString>,
    pub omitted: Vec<NonEmptyString>,
    pub truncated: bool,
}

/// What `git.patch` reports: the patch from `base` to `head`, or to the work
/// tree, as bounded text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(id = "runx.git.patch.v1")]
pub struct GitPatch {
    pub repo_root: NonEmptyString,
    pub base: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<NonEmptyString>,
    pub patch: String,
    pub files: Vec<NonEmptyString>,
    pub omitted: Vec<NonEmptyString>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, RunxSchema)]
#[serde(deny_unknown_fields)]
#[runx_schema(
    id = "runx.git.blob_digest.v1",
    url = "https://schemas.runx.ai/runx/git/blob-digest/v1.json"
)]
pub struct GitBlobDigest {
    pub algorithm: String,
    pub digest: String,
    pub bytes: u64,
}
