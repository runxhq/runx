use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use runx_contracts::{Artifact, ArtifactLabels, Digest, JsonValue};

use super::{WorkspaceFile, WorkspaceFileError};

mod framing;
mod reader;
mod storage;

use framing::frame_json_array_page;
use reader::SnapshotReader;
use storage::{SnapshotStore, StoredArtifact};

pub(crate) const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const DEFAULT_ARTIFACT_PAGE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_ARTIFACT_PAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactPageEncoding {
    Base64,
    Utf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactPage {
    pub artifact: Artifact,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: Digest,
    pub encoding: ArtifactPageEncoding,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactRecordPage {
    pub artifact: Artifact,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: Digest,
    pub records: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct LocalArtifactService {
    store: Arc<SnapshotStore>,
}

impl Default for LocalArtifactService {
    fn default() -> Self {
        Self {
            store: Arc::new(SnapshotStore::default()),
        }
    }
}

impl LocalArtifactService {
    pub(crate) fn for_run(project: &Path, root_run_id: &str) -> Self {
        Self {
            store: Arc::new(SnapshotStore::for_run(project, root_run_id)),
        }
    }

    /// Describe a snapshot already admitted in this logical run. This never
    /// reopens or readmits the original source file.
    pub(crate) fn descriptor(&self, reference: &str) -> Result<Artifact, WorkspaceFileError> {
        Ok(self.store.resolve(reference)?.descriptor.clone())
    }

    /// The labels `value` carries: those of every artifact in it, and of every
    /// snapshot of this run it names. What a step or a call is given, and
    /// what it makes, is labelled by this one derivation.
    pub(crate) fn labels_of(&self, value: &JsonValue) -> ArtifactLabels {
        ArtifactLabels::derived_from([
            &ArtifactLabels::of_value(value),
            &self.labels_named_in(value),
        ])
    }

    /// The labels of what's read from the snapshots `value` names by
    /// reference, at any depth: a page of a `local_only` snapshot is
    /// `local_only`. A string that names no snapshot of this run adds none.
    pub(crate) fn labels_named_in(&self, value: &JsonValue) -> ArtifactLabels {
        let mut found = Vec::new();
        self.collect_named(value, &mut found);
        ArtifactLabels::derived_from(&found)
    }

    fn collect_named(&self, value: &JsonValue, found: &mut Vec<ArtifactLabels>) {
        match value {
            JsonValue::String(text) if storage::is_reference(text) => {
                if let Ok(snapshot) = self.store.resolve(text) {
                    found.push(snapshot.descriptor.labels);
                }
            }
            JsonValue::Object(object) => {
                for nested in object.values() {
                    self.collect_named(nested, found);
                }
            }
            JsonValue::Array(items) => {
                for nested in items {
                    self.collect_named(nested, found);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn admit(
        &self,
        root: &Path,
        requested: &Path,
        media_type: &str,
        reach: super::Reach,
        labels: ArtifactLabels,
    ) -> Result<Artifact, WorkspaceFileError> {
        let source = WorkspaceFile::resolve(root, requested, reach)?;
        self.store.capture(source, media_type, None, labels)
    }

    /// Admit a retained project artifact's bytes for page reads. The snapshot
    /// keeps the artifact's labels.
    pub(crate) fn open_project_artifact(
        &self,
        root: &Path,
        artifact: &Artifact,
    ) -> Result<Artifact, String> {
        let source = super::project_artifacts::project_artifact_source(root, artifact)?;
        self.store
            .capture(
                source,
                &artifact.media_type,
                Some(artifact),
                artifact.labels,
            )
            .map_err(|error| error.to_string())
    }

    pub(crate) fn read_page(
        &self,
        reference: &str,
        offset: u64,
        maximum: usize,
        encoding: ArtifactPageEncoding,
    ) -> Result<ArtifactPage, WorkspaceFileError> {
        if maximum == 0 || maximum > MAX_ARTIFACT_PAGE_BYTES {
            return Err(WorkspaceFileError::InvalidPageSize {
                maximum: MAX_ARTIFACT_PAGE_BYTES,
            });
        }
        let artifact = self.store.resolve(reference)?;
        if offset > artifact.descriptor.size_bytes {
            return Err(WorkspaceFileError::InvalidArtifactOffset {
                offset,
                bytes: artifact.descriptor.size_bytes,
            });
        }
        let mut file = artifact.reader()?;
        file.seek(SeekFrom::Start(offset))
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        // A page reads its window and nothing past it: a UTF-8 page that
        // stops inside a character ends before it, and the next page begins
        // with it.
        let remaining = artifact.descriptor.size_bytes.saturating_sub(offset);
        let capture = usize::try_from(remaining.min(maximum as u64)).unwrap_or(maximum);
        let mut bytes = vec![0; capture];
        file.read_exact(&mut bytes)
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        if encoding == ArtifactPageEncoding::Utf8 && (capture as u64) < remaining {
            while std::str::from_utf8(&bytes).is_err_and(|error| error.error_len().is_none()) {
                bytes.pop();
            }
        }
        let data = match encoding {
            ArtifactPageEncoding::Base64 => {
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            }
            ArtifactPageEncoding::Utf8 => std::str::from_utf8(&bytes)
                .map(str::to_owned)
                .map_err(|_| WorkspaceFileError::InvalidArtifactUtf8Offset(offset))?,
        };
        let length = bytes.len() as u64;
        let next_offset = offset + length;
        Ok(ArtifactPage {
            artifact: artifact.descriptor.clone(),
            offset,
            length,
            next_offset,
            eof: next_offset == artifact.descriptor.size_bytes,
            range_digest: Digest::of(&bytes),
            encoding,
            data,
        })
    }

    pub(crate) fn read_json_array_page(
        &self,
        reference: &str,
        offset: u64,
        maximum: usize,
    ) -> Result<ArtifactRecordPage, WorkspaceFileError> {
        self.read_json_array_page_with_record_budget(reference, offset, maximum, usize::MAX)
    }

    pub(crate) fn read_json_array_page_with_record_budget(
        &self,
        reference: &str,
        offset: u64,
        maximum: usize,
        maximum_encoded_records: usize,
    ) -> Result<ArtifactRecordPage, WorkspaceFileError> {
        self.pages(reference)?
            .json_array_page(offset, maximum, maximum_encoded_records)
    }

    /// The artifact `reference` names, open for page reads one after
    /// another: each chunk it reads stays read, so reading it whole reads
    /// each byte from the store once.
    pub(crate) fn pages(&self, reference: &str) -> Result<ArtifactPages, WorkspaceFileError> {
        let artifact = self.store.resolve(reference)?;
        let reader = artifact.reader()?;
        Ok(ArtifactPages { artifact, reader })
    }
}

/// An artifact open for page reads, with one reader for all of them.
pub(crate) struct ArtifactPages {
    artifact: StoredArtifact,
    reader: SnapshotReader,
}

impl ArtifactPages {
    /// The JSON array records from `offset`, at most `maximum` bytes of them
    /// and `maximum_encoded_records` encoded, and the digest of the range they
    /// were read from.
    pub(crate) fn json_array_page(
        &mut self,
        offset: u64,
        maximum: usize,
        maximum_encoded_records: usize,
    ) -> Result<ArtifactRecordPage, WorkspaceFileError> {
        if maximum == 0 || maximum > MAX_ARTIFACT_PAGE_BYTES {
            return Err(WorkspaceFileError::InvalidPageSize {
                maximum: MAX_ARTIFACT_PAGE_BYTES,
            });
        }
        if offset > self.artifact.descriptor.size_bytes {
            return Err(WorkspaceFileError::InvalidArtifactOffset {
                offset,
                bytes: self.artifact.descriptor.size_bytes,
            });
        }
        let frame =
            frame_json_array_page(&mut self.reader, offset, maximum, maximum_encoded_records)?;
        let range = read_exact_range(&mut self.reader, offset, frame.next_offset)?;
        Ok(ArtifactRecordPage {
            artifact: self.artifact.descriptor.clone(),
            offset,
            length: frame.next_offset.saturating_sub(offset),
            next_offset: frame.next_offset,
            eof: frame.eof,
            range_digest: Digest::of(&range),
            records: frame.records,
        })
    }
}

/// The bytes from `offset` to `end`, which a page's framing has just read,
/// so they're read again from the chunks the reader kept.
fn read_exact_range(
    file: &mut SnapshotReader,
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, WorkspaceFileError> {
    file.seek(SeekFrom::Start(offset))
        .map_err(WorkspaceFileError::SnapshotUnavailable)?;
    let length = usize::try_from(end.saturating_sub(offset)).map_err(|_| {
        WorkspaceFileError::InvalidArtifactFraming {
            offset,
            message: "page range does not fit memory bounds".to_owned(),
        }
    })?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes)
        .map_err(WorkspaceFileError::SnapshotUnavailable)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests;
