pub(super) use runx_contracts::GitBlobDigest;
use runx_contracts::{
    GitBranch, GitDiff, GitLog, GitPatch, GitShow, GitStatus, JsonObject, JsonValue,
};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitInput {
    pub(super) repo_root: String,
}

impl CapabilityInput for GitInput {
    fn defaults() -> JsonObject {
        JsonObject::from([("repo_root".to_owned(), JsonValue::String(".".to_owned()))])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitDiffInput {
    pub(super) repo_root: String,
    pub(super) base: String,
}

impl CapabilityInput for GitDiffInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            ("base".to_owned(), JsonValue::String("HEAD".to_owned())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitBlobDigestInput {
    pub(super) contents: String,
}

impl CapabilityInput for GitBlobDigestInput {}

impl CapabilityOutput for GitBranch {}
impl CapabilityOutput for GitStatus {}
impl CapabilityOutput for GitDiff {}
impl CapabilityOutput for GitLog {}
impl CapabilityOutput for GitShow {}
impl CapabilityOutput for GitPatch {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitBlobDigestOutput {
    pub(super) git_blob_digest: GitBlobDigest,
}

impl CapabilityOutput for GitBlobDigestOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitLogInput {
    pub(super) repo_root: String,
    pub(super) revision: String,
    pub(super) max_count: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) since: Option<String>,
}

impl CapabilityInput for GitLogInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            ("revision".to_owned(), JsonValue::String("HEAD".to_owned())),
            (
                "max_count".to_owned(),
                JsonValue::Number(runx_contracts::JsonNumber::U64(20)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitShowInput {
    pub(super) repo_root: String,
    pub(super) revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) path: Option<String>,
}

impl CapabilityInput for GitShowInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            ("revision".to_owned(), JsonValue::String("HEAD".to_owned())),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct GitPatchInput {
    pub(super) repo_root: String,
    pub(super) base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) head: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) paths: Vec<String>,
    pub(super) stat: bool,
}

impl CapabilityInput for GitPatchInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            ("base".to_owned(), JsonValue::String("HEAD".to_owned())),
            ("stat".to_owned(), JsonValue::Bool(false)),
        ])
    }
}

const LOG_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative Git repository root.",
    },
    CapabilityField {
        name: "revision",
        description: "Revision or range to walk, such as HEAD or main..feature.",
    },
    CapabilityField {
        name: "max_count",
        description: "The most commits to return, 1-200.",
    },
    CapabilityField {
        name: "paths",
        description: "Optional paths, relative to the root, that the commits must touch.",
    },
    CapabilityField {
        name: "since",
        description: "Optional earliest date, such as 2026-10-01 or 2.weeks.ago.",
    },
];

const SHOW_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative Git repository root.",
    },
    CapabilityField {
        name: "revision",
        description: "Commit or revision to show.",
    },
    CapabilityField {
        name: "path",
        description: "Optional file, relative to the root, to show as it was at the revision.",
    },
];

const PATCH_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative Git repository root.",
    },
    CapabilityField {
        name: "base",
        description: "Revision to compare from.",
    },
    CapabilityField {
        name: "head",
        description: "Optional revision to compare to; without it, the working tree.",
    },
    CapabilityField {
        name: "paths",
        description: "Optional paths, relative to the root, to limit the comparison to.",
    },
    CapabilityField {
        name: "stat",
        description: "Whether to return a change summary instead of the patch.",
    },
];

const ROOT_FIELDS: &[CapabilityField] = &[CapabilityField {
    name: "repo_root",
    description: "Workspace-relative Git repository root.",
}];

const DIFF_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative Git repository root.",
    },
    CapabilityField {
        name: "base",
        description: "Verified Git ref or commit to compare against.",
    },
];

const BLOB_FIELDS: &[CapabilityField] = &[CapabilityField {
    name: "contents",
    description: "Exact UTF-8 blob contents to bind to Git's canonical blob identity.",
}];

static CURRENT_BRANCH: TypedNativeCapability<GitInput, GitBranch> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "git.current_branch",
        source: "runx-runtime/git",
        summary: "Read the current branch or detached HEAD through native process supervision.",
        scopes: &["git.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "git_branch",
            packet: "runx.git.branch.v1",
        },
        fields: ROOT_FIELDS,
    },
    super::current_branch,
);

static STATUS: TypedNativeCapability<GitInput, GitStatus> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "git.status",
        source: "runx-runtime/git",
        summary: "Read bounded porcelain working-tree status without hooks, pagers, or locks.",
        scopes: &["git.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "git_status",
            packet: "runx.git.status.v1",
        },
        fields: ROOT_FIELDS,
    },
    super::status,
);

static DIFF: TypedNativeCapability<GitDiffInput, GitDiff> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "git.diff_name_only",
        source: "runx-runtime/git",
        summary: "List changed paths against one verified Git commit without external drivers.",
        scopes: &["git.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "git_diff",
            packet: "runx.git.diff.v1",
        },
        fields: DIFF_FIELDS,
    },
    super::diff_name_only,
);

static BLOB_DIGEST: TypedNativeCapability<GitBlobDigestInput, GitBlobDigestOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "git.blob_digest",
            source: "runx-runtime/git",
            summary: "Compute the canonical Git blob SHA-1 for exact UTF-8 contents.",
            scopes: &[],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Named {
                output: "git_blob_digest",
                packet: "runx.git.blob_digest.v1",
            },
            fields: BLOB_FIELDS,
        },
        super::blob_digest,
    );

static LOG: TypedNativeCapability<GitLogInput, GitLog> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "git.log",
        source: "runx-runtime/git",
        summary: "List commits with their authors, dates, subjects and changed files.",
        scopes: &["git.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "git_log",
            packet: "runx.git.log.v1",
        },
        fields: LOG_FIELDS,
    },
    super::history::log,
);

static SHOW: TypedNativeCapability<GitShowInput, GitShow> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "git.show",
        source: "runx-runtime/git",
        summary: "Show one commit with its patch, or one file as it was at a revision.",
        scopes: &["git.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "git_show",
            packet: "runx.git.show.v1",
        },
        fields: SHOW_FIELDS,
    },
    super::history::show,
);

static PATCH: TypedNativeCapability<GitPatchInput, GitPatch> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "git.diff",
        source: "runx-runtime/git",
        summary: "Show the patch between two revisions, or between a revision and the working tree.",
        scopes: &["git.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "git_patch",
            packet: "runx.git.patch.v1",
        },
        fields: PATCH_FIELDS,
    },
    super::history::diff,
);

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] = &[
    &CURRENT_BRANCH,
    &STATUS,
    &DIFF,
    &BLOB_DIGEST,
    &LOG,
    &SHOW,
    &PATCH,
];
