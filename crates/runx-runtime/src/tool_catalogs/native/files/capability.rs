use runx_contracts::{
    Digest, FileApplyBundle, FileBoundDelete, FileBundleWrite, FileEdit, FileList, FileRead,
    FileReadBundle, FileSearch, FileWrite, JsonNumber, JsonObject, JsonValue,
};
use serde::{Deserialize, Serialize};

use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileReadInput {
    pub(super) path: String,
    pub(super) start_line: u64,
    pub(super) line_count: u64,
    pub(super) max_bytes: u64,
}

impl CapabilityInput for FileReadInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            (
                "start_line".to_owned(),
                JsonValue::Number(JsonNumber::U64(1)),
            ),
            (
                "line_count".to_owned(),
                JsonValue::Number(JsonNumber::U64(2000)),
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
    pub(super) paths: Vec<String>,
    pub(super) max_bytes: u64,
    pub(super) on_missing: String,
}

impl CapabilityInput for FileReadBundleInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
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
    pub(super) path: String,
    pub(super) contents: String,
    /// The digest of the content it replaces, as a read returned it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) expected_digest: Option<Digest>,
    /// The file must not exist yet.
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub(super) create: bool,
}

impl CapabilityInput for FileWriteInput {
    fn defaults() -> JsonObject {
        JsonObject::new()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileApplyBundleInput {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) writes: Vec<FileBundleWrite>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) deletes: Vec<FileBoundDelete>,
}

impl CapabilityInput for FileApplyBundleInput {
    fn defaults() -> JsonObject {
        JsonObject::new()
    }
}

impl CapabilityOutput for FileRead {}
impl CapabilityOutput for FileReadBundle {}
impl CapabilityOutput for FileWrite {}
impl CapabilityOutput for FileEdit {}
impl CapabilityOutput for FileApplyBundle {}
impl CapabilityOutput for FileList {}
impl CapabilityOutput for FileSearch {}

/// One exact replacement: `old` occurs once in the file, or at least once
/// with `replace_all`.
#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct TextReplacement {
    pub(super) old: String,
    pub(super) new: String,
    #[serde(default, skip_serializing_if = "runx_contracts::schema::is_default")]
    pub(super) replace_all: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileEditInput {
    pub(super) path: String,
    pub(super) edits: Vec<TextReplacement>,
}

impl CapabilityInput for FileEditInput {
    fn defaults() -> JsonObject {
        JsonObject::new()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct FileListInput {
    pub(super) path: String,
    pub(super) depth: u64,
    pub(super) max_entries: u64,
}

impl CapabilityInput for FileListInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
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
pub(super) struct FileSearchInput {
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

const LIST_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "path",
        description: "Directory to list: from the workspace root, $package/<path> in the skill's own package, or an absolute path the run's reach admits.",
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
    CapabilityField {
        name: "path",
        description: "Directory to search under, or one file to search: from the workspace root, $package/<path> in the skill's own package, or an absolute path the run's reach admits.",
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
        description: "Optional glob a file's path must match, such as **/*.rs, from the directory its path is read under: the workspace root, the package's, or an absolute path's parent.",
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
    CapabilityField {
        name: "path",
        description: "UTF-8 file path: from the workspace root, $package/<path> for a file of the skill's own package, or an absolute path the run's reach admits.",
    },
    CapabilityField {
        name: "start_line",
        description: "The first line to read, counting from 1; 1 by default.",
    },
    CapabilityField {
        name: "line_count",
        description: "The most lines to read; 2000 by default.",
    },
    CapabilityField {
        name: "max_bytes",
        description: "Positive bound on the bytes read, up to eight MiB; a read stops at the last whole line within it.",
    },
];

const READ_BUNDLE_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "paths",
        description: "Up to sixteen distinct UTF-8 file paths, each from the workspace root, $package/<path>, or an absolute path the run's reach admits.",
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
    CapabilityField {
        name: "path",
        description: "UTF-8 file path: from the workspace root, $package/<path> for a file of the skill's own package, or an absolute path the run's reach admits.",
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
    CapabilityField {
        name: "path",
        description: "UTF-8 file path from the workspace root; the file must exist.",
    },
    CapabilityField {
        name: "edits",
        description: "Exact replacements applied in order, each {old, new, replace_all?}: old occurs once, or at least once with replace_all.",
    },
];

const APPLY_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "writes",
        description: "Bounded array of UTF-8 text writes, each path from the workspace root, each with the expected_digest of what it replaces or create for a new file.",
    },
    CapabilityField {
        name: "deletes",
        description: "Bounded array of file paths to delete, from the workspace root, each a path or {path, expected_digest}.",
    },
];

static READ: TypedNativeCapability<FileReadInput, FileRead> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.read",
        source: "runx-runtime/filesystem",
        summary: "Read lines of one UTF-8 file of up to eight MiB through native workspace and symlink containment: from start_line, at most line_count lines within max_bytes, with the file's total lines and whole-file digest.",
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

static READ_BUNDLE: TypedNativeCapability<FileReadBundleInput, FileReadBundle> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "fs.read_bundle",
            source: "runx-runtime/filesystem",
            summary: "Read a bounded set of UTF-8 files through one native containment boundary.",
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

static WRITE: TypedNativeCapability<FileWriteInput, FileWrite> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.write",
        source: "runx-runtime/filesystem",
        summary: "Write one bounded UTF-8 file through transactional workspace containment.",
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

static EDIT: TypedNativeCapability<FileEditInput, FileEdit> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.edit",
        source: "runx-runtime/filesystem",
        summary: "Replace exact text in one UTF-8 file, bound to the content it was read as.",
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

static APPLY: TypedNativeCapability<FileApplyBundleInput, FileApplyBundle> =
    TypedNativeCapability::new(
        CapabilityDefinition {
            id: "fs.apply_bundle",
            source: "runx-runtime/filesystem",
            summary: "Apply bounded text writes and deletions transactionally inside one workspace root.",
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

static LIST: TypedNativeCapability<FileListInput, FileList> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.list",
        source: "runx-runtime/filesystem",
        summary: "List entries under a directory to a bounded depth, honouring the project's ignore files.",
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

static SEARCH: TypedNativeCapability<FileSearchInput, FileSearch> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "fs.search",
        source: "runx-runtime/filesystem",
        summary: "Search text files for a literal or regular expression, honouring the project's ignore files.",
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
