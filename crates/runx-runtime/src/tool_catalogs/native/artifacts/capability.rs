use runx_contracts::{Artifact, ArtifactLabels, ArtifactPage, JsonNumber, JsonObject, JsonValue};
use serde::{Deserialize, Serialize};

use crate::services::DEFAULT_ARTIFACT_PAGE_BYTES;
use crate::{
    CapabilityApproval, CapabilityArtifacts, CapabilityDefinition, CapabilityEffect,
    CapabilityField, CapabilityInput, CapabilityOutput,
};

use super::super::capability::{NativeCapability, TypedNativeCapability};

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactAdmitInput {
    pub(super) repo_root: String,
    pub(super) path_scope: String,
    pub(super) path: String,
    pub(super) media_type: String,
    pub(super) labels: ArtifactLabels,
}

impl CapabilityInput for ArtifactAdmitInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            (
                "labels".to_owned(),
                JsonValue::Object(JsonObject::from([(
                    "sharing".to_owned(),
                    JsonValue::String("configured_model".to_owned()),
                )])),
            ),
            ("repo_root".to_owned(), JsonValue::String(".".to_owned())),
            (
                "path_scope".to_owned(),
                JsonValue::String("workspace".to_owned()),
            ),
            (
                "media_type".to_owned(),
                JsonValue::String("application/octet-stream".to_owned()),
            ),
        ])
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, runx_contracts::schema::RunxSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct ArtifactReadInput {
    #[serde(rename = "ref")]
    pub(super) reference: String,
    pub(super) offset: u64,
    pub(super) max_bytes: u64,
    pub(super) encoding: String,
}

impl CapabilityInput for ArtifactReadInput {
    fn defaults() -> JsonObject {
        JsonObject::from([
            ("offset".to_owned(), JsonValue::Number(JsonNumber::U64(0))),
            (
                "max_bytes".to_owned(),
                JsonValue::Number(JsonNumber::U64(DEFAULT_ARTIFACT_PAGE_BYTES as u64)),
            ),
            (
                "encoding".to_owned(),
                JsonValue::String("base64".to_owned()),
            ),
        ])
    }
}

impl CapabilityOutput for Artifact {}
impl CapabilityOutput for ArtifactPage {}

const ADMIT_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "repo_root",
        description: "Workspace-relative root containing the immutable source file.",
    },
    CapabilityField {
        name: "path_scope",
        description: "Trusted root selection: workspace or the owning skill package.",
    },
    CapabilityField {
        name: "path",
        description: "Contained relative source path; it is never returned in the artifact reference.",
    },
    CapabilityField {
        name: "media_type",
        description: "Media type bound into the immutable artifact identity.",
    },
    CapabilityField {
        name: "labels",
        description: "Where the artifact may be shared: local_only keeps it from any model that may run elsewhere. Defaults to configured_model.",
    },
];

const READ_FIELDS: &[CapabilityField] = &[
    CapabilityField {
        name: "ref",
        description: "Opaque logical-run reference returned by artifact.admit or artifact.open; survives native process restart but grants no authority.",
    },
    CapabilityField {
        name: "offset",
        description: "Exact byte offset; continue with the prior page's next_offset.",
    },
    CapabilityField {
        name: "max_bytes",
        description: "Positive page size; defaults to one MiB and cannot exceed the four MiB runtime ceiling.",
    },
    CapabilityField {
        name: "encoding",
        description: "Page encoding: base64 exact bytes, character-safe utf8, or framed json_array records.",
    },
];

static ADMIT: TypedNativeCapability<ArtifactAdmitInput, Artifact> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "artifact.admit",
        source: "runx-runtime/local-artifacts",
        summary: "Bind one contained local file to an immutable opaque artifact reference.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "artifact",
            packet: runx_contracts::ARTIFACT_SCHEMA,
        },
        fields: ADMIT_FIELDS,
    },
    super::admit,
);

static READ: TypedNativeCapability<ArtifactReadInput, ArtifactPage> = TypedNativeCapability::new(
    CapabilityDefinition {
        id: "artifact.read",
        source: "runx-runtime/local-artifacts",
        summary: "Read one exact bounded page from a previously admitted local artifact.",
        effect: CapabilityEffect::Read,
        approval: CapabilityApproval::None,
        artifacts: CapabilityArtifacts::Wrapped {
            output: "artifact_page",
            packet: "runx.artifact_page.v1",
        },
        fields: READ_FIELDS,
    },
    super::read,
);

pub(in crate::tool_catalogs::native) const CAPABILITIES: &[&dyn NativeCapability] =
    &[&ADMIT, &READ];
