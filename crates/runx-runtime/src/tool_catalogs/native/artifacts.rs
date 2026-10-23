//! Immutable local-artifact admission and bounded page reads.

use std::path::Path;

use super::{NativeInvocation, invalid_input};
use crate::RuntimeError;
use crate::services::ArtifactPageEncoding;

mod capability;

pub(super) use capability::CAPABILITIES;
use capability::{ArtifactAdmitInput, ArtifactAdmitOutput, ArtifactReadInput, ArtifactReadOutput};

const ADMIT_TOOL: &str = "artifact.admit";
const READ_TOOL: &str = "artifact.read";

fn admit(
    invocation: &NativeInvocation<'_, ArtifactAdmitInput>,
) -> Result<ArtifactAdmitOutput, RuntimeError> {
    let root = super::files::root(
        ADMIT_TOOL,
        &invocation.inputs.repo_root,
        &invocation.inputs.path_scope,
        invocation,
    )?;
    let artifact = invocation
        .local_artifacts
        .admit(
            &root,
            Path::new(&invocation.inputs.path),
            &invocation.inputs.media_type,
            invocation.bounds,
        )
        .map_err(|error| invalid_input(ADMIT_TOOL, error.to_string()))?;
    Ok(ArtifactAdmitOutput {
        artifact_uri: artifact.reference,
        media_type: artifact.media_type,
        bytes: artifact.bytes,
        whole_digest: artifact.whole_digest,
    })
}

fn read(
    invocation: &NativeInvocation<'_, ArtifactReadInput>,
) -> Result<ArtifactReadOutput, RuntimeError> {
    let maximum = usize::try_from(invocation.inputs.max_bytes)
        .map_err(|_| invalid_input(READ_TOOL, "max_bytes is too large"))?;
    if invocation.inputs.encoding == "json_array" {
        let page = invocation
            .local_artifacts
            .read_json_array_page(
                &invocation.inputs.artifact_uri,
                invocation.inputs.offset,
                maximum,
            )
            .map_err(|error| invalid_input(READ_TOOL, error.to_string()))?;
        return Ok(ArtifactReadOutput {
            artifact_uri: page.artifact.reference,
            media_type: page.artifact.media_type,
            offset: page.offset,
            length: page.length,
            next_offset: page.next_offset,
            eof: page.eof,
            range_digest: page.range_digest,
            whole_digest: page.artifact.whole_digest,
            encoding: "json_array".to_owned(),
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
            &invocation.inputs.artifact_uri,
            invocation.inputs.offset,
            maximum,
            encoding,
        )
        .map_err(|error| invalid_input(READ_TOOL, error.to_string()))?;
    Ok(ArtifactReadOutput {
        artifact_uri: page.artifact.reference,
        media_type: page.artifact.media_type,
        offset: page.offset,
        length: page.length,
        next_offset: page.next_offset,
        eof: page.eof,
        range_digest: page.range_digest,
        whole_digest: page.artifact.whole_digest,
        encoding: match page.encoding {
            ArtifactPageEncoding::Base64 => "base64",
            ArtifactPageEncoding::Utf8 => "utf8",
        }
        .to_owned(),
        data: Some(page.data),
        records: Vec::new(),
    })
}
