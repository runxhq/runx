//! Immutable local-artifact admission and bounded page reads.

use std::path::Path;

use runx_contracts::schema::NonEmptyString;
use runx_contracts::{Artifact, ArtifactPage};

use super::{NativeInvocation, invalid_input};
use crate::RuntimeError;
use crate::services::ArtifactPageEncoding;

mod capability;

pub(super) use capability::CAPABILITIES;
use capability::{ArtifactAdmitInput, ArtifactReadInput};

const ADMIT_TOOL: &str = "artifact.admit";
const READ_TOOL: &str = "artifact.read";

fn admit(invocation: &NativeInvocation<'_, ArtifactAdmitInput>) -> Result<Artifact, RuntimeError> {
    let root = super::files::root(
        ADMIT_TOOL,
        &invocation.inputs.repo_root,
        &invocation.inputs.path_scope,
        invocation,
    )?;
    invocation
        .local_artifacts
        .admit(
            &root,
            Path::new(&invocation.inputs.path),
            &invocation.inputs.media_type,
            invocation.reach,
            invocation.inputs.labels,
        )
        .map_err(|error| invalid_input(ADMIT_TOOL, error.to_string()))
}

fn read(
    invocation: &NativeInvocation<'_, ArtifactReadInput>,
) -> Result<ArtifactPage, RuntimeError> {
    let maximum = usize::try_from(invocation.inputs.max_bytes)
        .map_err(|_| invalid_input(READ_TOOL, "max_bytes is too large"))?;
    if invocation.inputs.encoding == "json_array" {
        let page = invocation
            .local_artifacts
            .read_json_array_page(
                &invocation.inputs.reference,
                invocation.inputs.offset,
                maximum,
            )
            .map_err(|error| invalid_input(READ_TOOL, error.to_string()))?;
        crate::work_counts::count_artifact_bytes(
            crate::work_counts::ArtifactCopy::ToResult,
            page.length,
        );
        return Ok(ArtifactPage {
            reference: page.artifact.reference,
            offset: page.offset,
            length: page.length,
            next_offset: page.next_offset,
            eof: page.eof,
            range_digest: page.range_digest,
            digest: page.artifact.digest,
            encoding: NonEmptyString::from("json_array"),
            data: None,
            records: page.records,
        });
    }
    let encoding = match invocation.inputs.encoding.as_str() {
        "base64" => ArtifactPageEncoding::Base64,
        "utf8" => ArtifactPageEncoding::Utf8,
        _ => {
            return Err(invalid_input(
                READ_TOOL,
                "encoding must be base64, utf8, or json_array",
            ));
        }
    };
    let page = invocation
        .local_artifacts
        .read_page(
            &invocation.inputs.reference,
            invocation.inputs.offset,
            maximum,
            encoding,
        )
        .map_err(|error| invalid_input(READ_TOOL, error.to_string()))?;
    crate::work_counts::count_artifact_bytes(
        crate::work_counts::ArtifactCopy::ToResult,
        page.length,
    );
    Ok(ArtifactPage {
        reference: page.artifact.reference,
        offset: page.offset,
        length: page.length,
        next_offset: page.next_offset,
        eof: page.eof,
        range_digest: page.range_digest,
        digest: page.artifact.digest,
        encoding: NonEmptyString::from(match page.encoding {
            ArtifactPageEncoding::Base64 => "base64",
            ArtifactPageEncoding::Utf8 => "utf8",
        }),
        data: Some(page.data),
        records: Vec::new(),
    })
}
