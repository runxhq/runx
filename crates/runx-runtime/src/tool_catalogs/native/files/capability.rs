use runx_contracts::{JsonNumber, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileReadInput {
    pub(super) repo_root: String,
    pub(super) path_scope: String,
    pub(super) path: String,
    pub(super) max_bytes: u64,
}

impl CapabilityInput for FileReadInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            (
                "path_scope".to_owned(),
                JsonValue::String("workspace".to_owned()),
            ),
            (
                "max_bytes".to_owned(),
                JsonValue::Number(JsonNumber::U64(1024 * 1024)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileReadBundleInput {
    pub(super) repo_root: String,
    pub(super) path_scope: String,
    pub(super) paths: Vec<String>,
    pub(super) max_bytes: u64,
    pub(super) on_missing: String,
}

impl CapabilityInput for FileReadBundleInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            (
                "path_scope".to_owned(),
                JsonValue::String("workspace".to_owned()),
            ),
            (
                "max_bytes".to_owned(),
                JsonValue::Number(JsonNumber::U64(8 * 1024 * 1024)),
            ),
            (
                "on_missing".to_owned(),
                JsonValue::String("error".to_owned()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileWriteInput {
    pub(super) repo_root: String,
    pub(super) path: String,
    pub(super) contents: String,
    /// The digest of the content it replaces, as a read returned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) expected_digest: Option<String>,
    /// The file must not exist yet.
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub(super) create: bool,
}

impl CapabilityInput for FileWriteInput {
    fn defaults() -> JsonObject {
        JsonObject::from([("repo_root".to_owned(), JsonValue::String(".".to_owned()))])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BundleWrite {
    pub(super) path: String,
    pub(super) contents: String,
    /// The digest of the content it replaces, as a read returned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) expected_digest: Option<String>,
    /// The file must not exist yet.
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub(super) create: bool,
}

/// A file to delete: its path, or its path with the digest of the content
/// it removes.
#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(untagged)]
pub(super) enum BundleDelete {
    Path(String),
    Bound(BoundDelete),
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct BoundDelete {
    pub(super) path: String,
    pub(super) expected_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileApplyBundleInput {
    pub(super) repo_root: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) writes: Vec<BundleWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) deletes: Vec<BundleDelete>,
}

impl CapabilityInput for FileApplyBundleInput {
    fn defaults() -> JsonObject {
        JsonObject::from([("repo_root".to_owned(), JsonValue::String(".".to_owned()))])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileReadOutput {
    pub(super) path: String,
    pub(super) repo_root: String,
    pub(super) contents: String,
    pub(super) bytes: u64,
    pub(super) truncated: bool,
    pub(super) content_digest: String,
}

impl CapabilityOutput for FileReadOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileReadBundleOutput {
    pub(super) repo_root: String,
    pub(super) file_count: u64,
    pub(super) total_bytes: u64,
    pub(super) files: Vec<FileReadOutput>,
    pub(super) missing: Vec<MissingFile>,
}

impl CapabilityOutput for FileReadBundleOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct MissingFile {
    pub(super) path: String,
    pub(super) reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileWriteOutput {
    pub(super) path: String,
    pub(super) repo_root: String,
    pub(super) bytes_written: u64,
    pub(super) sha256: String,
    /// What the file held before, when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) before_digest: Option<String>,
    /// The run's journal entry holding what it held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) journal: Option<u64>,
}

impl CapabilityOutput for FileWriteOutput {}

/// One exact replacement: `old` occurs once in the file, or at least once
/// with `replace_all`.
#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileEdit {
    pub(super) old: String,
    pub(super) new: String,
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub(super) replace_all: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileEditInput {
    pub(super) repo_root: String,
    pub(super) path: String,
    pub(super) edits: Vec<FileEdit>,
}

impl CapabilityInput for FileEditInput {
    fn defaults() -> JsonObject {
        JsonObject::from([("repo_root".to_owned(), JsonValue::String(".".to_owned()))])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileEditOutput {
    pub(super) path: String,
    pub(super) repo_root: String,
    /// How many places the edits replaced.
    pub(super) replacements: u64,
    /// The digest of the content the edits replaced.
    pub(super) before_digest: String,
    /// The digest of what the file now holds.
    pub(super) content_digest: String,
    pub(super) bytes_written: u64,
    /// The run's journal entry holding what it held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) journal: Option<u64>,
}

impl CapabilityOutput for FileEditOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileApplyBundleOutput {
    pub(super) repo_root: String,
    pub(super) write_count: u64,
    pub(super) delete_count: u64,
    pub(super) writes: Vec<AppliedWrite>,
    pub(super) deletes: Vec<AppliedDelete>,
    /// The run's journal entry holding what each file held before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) journal: Option<u64>,
}

impl CapabilityOutput for FileApplyBundleOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AppliedWrite {
    pub(super) path: String,
    pub(super) bytes_written: u64,
    pub(super) sha256: String,
    /// What the file held before, when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) before_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct AppliedDelete {
    pub(super) path: String,
    pub(super) status: String,
    /// What the file held before, when it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) before_digest: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileListInput {
    pub(super) repo_root: String,
    pub(super) path_scope: String,
    pub(super) path: String,
    pub(super) depth: u64,
    pub(super) max_entries: u64,
}

impl CapabilityInput for FileListInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            (
                "path_scope".to_owned(),
                JsonValue::String("workspace".to_owned()),
            ),
            ("path".to_owned(), JsonValue::String(".".to_owned())),
            ("depth".to_owned(), JsonValue::Number(JsonNumber::U64(2))),
            (
                "max_entries".to_owned(),
                JsonValue::Number(JsonNumber::U64(500)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileListEntry {
    pub(super) path: String,
    pub(super) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) bytes: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileListOutput {
    pub(super) repo_root: String,
    pub(super) path: String,
    pub(super) entries: Vec<FileListEntry>,
    pub(super) truncated: bool,
}

impl CapabilityOutput for FileListOutput {}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileSearchInput {
    pub(super) repo_root: String,
    pub(super) path_scope: String,
    pub(super) path: String,
    pub(super) pattern: String,
    pub(super) regex: bool,
    pub(super) case_sensitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) glob: Option<String>,
    pub(super) max_results: u64,
    pub(super) context_lines: u64,
}

impl CapabilityInput for FileSearchInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            (
                "path_scope".to_owned(),
                JsonValue::String("workspace".to_owned()),
            ),
            ("path".to_owned(), JsonValue::String(".".to_owned())),
            ("regex".to_owned(), JsonValue::Bool(false)),
            ("case_sensitive".to_owned(), JsonValue::Bool(true)),
            (
                "max_results".to_owned(),
                JsonValue::Number(JsonNumber::U64(100)),
            ),
            (
                "context_lines".to_owned(),
                JsonValue::Number(JsonNumber::U64(1)),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileSearchMatch {
    pub(super) path: String,
    pub(super) line: u64,
    pub(super) text: String,
    pub(super) before: Vec<String>,
    pub(super) after: Vec<String>,
    pub(super) content_digest: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileSearchOutput {
    pub(super) repo_root: String,
    pub(super) matches: Vec<FileSearchMatch>,
    pub(super) files_searched: u64,
    pub(super) truncated: bool,
}

impl CapabilityOutput for FileSearchOutput {}

const LIST_FIELDS: &[CapabilityField] = &[
    root_field(),
    path_scope_field(),
    CapabilityField {
        name: "path",
        description: "Directory to list, relative to the resolved trusted root.",
    },
    CapabilityField {
        name: "depth",
        description: "How many directory levels to descend, 1-16.",
    },
    CapabilityField {
        name: "max_entries",
        description: "The most entries to return, 1-5000; more marks the listing truncated.",
    },
];

const SEARCH_FIELDS: &[CapabilityField] = &[
    root_field(),
    path_scope_field(),
    CapabilityField {
        name: "path",
        description: "Directory to search under, relative to the resolved trusted root.",
    },
    CapabilityField {
        name: "pattern",
        description: "Text to find in each line; a regular expression when regex is true.",
    },
    CapabilityField {
        name: "regex",
        description: "Whether pattern is a regular expression rather than literal text.",
    },
    CapabilityField {
        name: "case_sensitive",
        description: "Whether the match respects case.",
    },
    CapabilityField {
        name: "glob",
        description: "Optional glob a file's root-relative path must match, such as **/*.rs.",
    },
    CapabilityField {
        name: "max_results",
        description: "The most matching lines to return, 1-1000; more marks the search truncated.",
    },
    CapabilityField {
        name: "context_lines",
        description: "Lines of context before and after each match, 0-3.",
    },
];

const READ_FIELDS: &[CapabilityField] = &[
    root_field(),
    path_scope_field(),
    CapabilityField {
        name: "path",
        description: "UTF-8 file path relative to the resolved trusted root.",
    },
    CapabilityField {
        name: "max_bytes",
        description: "Positive capture limit up to eight MiB.",
    },
];

const READ_BUNDLE_FIELDS: &[CapabilityField] = &[
    root_field(),
    path_scope_field(),
    CapabilityField {
        name: "paths",
        description: "Up to sixteen distinct relative UTF-8 file paths.",
    },
    CapabilityField {
        name: "max_bytes",
        description: "Positive per-file capture limit up to eight MiB.",
    },
    CapabilityField {
        name: "on_missing",
        description: "Unavailable-file policy: error or report.",
    },
];

const WRITE_FIELDS: &[CapabilityField] = &[
    root_field(),
    CapabilityField {
        name: "path",
        description: "UTF-8 file path relative to the resolved trusted root.",
    },
    CapabilityField {
        name: "contents",
        description: "Exact UTF-8 contents to write.",
    },
    CapabilityField {
        name: "expected_digest",
        description: "The content_digest a read returned for the file this replaces; the write is refused if it changed since.",
    },
    CapabilityField {
        name: "create",
        description: "The file must not exist yet.",
    },
];

const EDIT_FIELDS: &[CapabilityField] = &[
    root_field(),
    CapabilityField {
        name: "path",
        description: "UTF-8 file path relative to the resolved trusted root; the file must exist.",
    },
    CapabilityField {
        name: "edits",
        description: "Exact replacements applied in order, each {old, new, replace_all?}: old occurs once, or at least once with replace_all.",
    },
];

const APPLY_FIELDS: &[CapabilityField] = &[
    root_field(),
    CapabilityField {
        name: "writes",
        description: "Bounded array of relative-path UTF-8 text writes, each with the expected_digest of what it replaces or create for a new file.",
    },
    CapabilityField {
        name: "deletes",
        description: "Bounded array of relative file paths to delete, each a path or {path, expected_digest}.",
    },
];

const fn root_field() -> CapabilityField {
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative project root; the runtime owns the workspace boundary.",
    }
}

const fn path_scope_field() -> CapabilityField {
    CapabilityField {
        name: "path_scope",
        description: "Trusted root selection: workspace or the owning skill package.",
    }
}

static READ: TypedNativeCapability<FileReadInput, FileReadOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.read",
        owner: "runx-runtime/filesystem",
        summary: "Read one bounded UTF-8 file through native workspace and symlink containment.",
        scopes: &["fs.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "file_read",
            packet: "runx.fs.file_read.v1",
        },
        fields: READ_FIELDS,
    },
    super::read,
);

static READ_BUNDLE: TypedNativeCapability<FileReadBundleInput, FileReadBundleOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "fs.read_bundle",
            owner: "runx-runtime/filesystem",
            summary: "Read a bounded set of UTF-8 files through one native containment boundary.",
            scopes: &["fs.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "file_read_bundle",
                packet: "runx.fs.file_read_bundle.v1",
            },
            fields: READ_BUNDLE_FIELDS,
        },
        super::read_bundle,
    );

static WRITE: TypedNativeCapability<FileWriteInput, FileWriteOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.write",
        owner: "runx-runtime/filesystem",
        summary: "Write one bounded UTF-8 file through transactional workspace containment.",
        scopes: &["fs.write"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "file_write",
            packet: "runx.fs.file_write.v1",
        },
        fields: WRITE_FIELDS,
    },
    super::write,
)
.describing(super::write_effect);

static EDIT: TypedNativeCapability<FileEditInput, FileEditOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.edit",
        owner: "runx-runtime/filesystem",
        summary: "Replace exact text in one UTF-8 file, bound to the content it was read as.",
        scopes: &["fs.write"],
        effect: CapabilityEffect::Mutate,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "file_edit",
            packet: "runx.fs.file_edit.v1",
        },
        fields: EDIT_FIELDS,
    },
    super::edit,
)
.describing(super::edit_effect);

static APPLY: TypedNativeCapability<FileApplyBundleInput, FileApplyBundleOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "fs.apply_bundle",
            owner: "runx-runtime/filesystem",
            summary: "Apply bounded text writes and deletions transactionally inside one workspace root.",
            scopes: &["fs.write", "fs.delete"],
            effect: CapabilityEffect::Mutate,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "file_bundle_apply",
                packet: "runx.fs.apply_bundle.v1",
            },
            fields: APPLY_FIELDS,
        },
        super::apply_files,
    )
    .describing(super::files_effect);

static LIST: TypedNativeCapability<FileListInput, FileListOutput> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.list",
        owner: "runx-runtime/filesystem",
        summary: "List entries under a directory to a bounded depth, honouring the project's ignore files.",
        scopes: &["fs.read"],
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "file_list",
            packet: "runx.fs.file_list.v1",
        },
        fields: LIST_FIELDS,
    },
    super::walk::list,
);

static SEARCH: TypedNativeCapability<FileSearchInput, FileSearchOutput> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "fs.search",
            owner: "runx-runtime/filesystem",
            summary: "Search text files for a literal or regular expression, honouring the project's ignore files.",
            scopes: &["fs.read"],
            effect: CapabilityEffect::Read,
            approval: CapabilityApproval::None,
            artifacts: CapabilityArtifacts::Wrapped {
                output: "file_search",
                packet: "runx.fs.file_search.v1",
            },
            fields: SEARCH_FIELDS,
        },
        super::walk::search,
    );

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] = &[
    &READ,
    &READ_BUNDLE,
    &LIST,
    &SEARCH,
    &WRITE,
    &EDIT,
    &APPLY,
    &super::inverse::INVERSE,
];
