use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest as _, Sha256};

use super::{WorkspaceFile, WorkspaceFileError};

mod framing;

use framing::frame_json_array_page;

pub(crate) const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
pub(crate) const DEFAULT_ARTIFACT_PAGE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_ARTIFACT_PAGE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalArtifact {
    pub reference: String,
    pub media_type: String,
    pub bytes: u64,
    pub whole_digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArtifactPageEncoding {
    Base64,
    Utf8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactPage {
    pub artifact: LocalArtifact,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: String,
    pub encoding: ArtifactPageEncoding,
    pub data: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ArtifactRecordPage {
    pub artifact: LocalArtifact,
    pub offset: u64,
    pub length: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub range_digest: String,
    pub records: Vec<String>,
}

#[derive(Clone)]
pub(crate) struct LocalArtifactService {
    inner: Arc<Mutex<BTreeMap<String, Arc<StoredArtifact>>>>,
    namespace: Arc<[u8; 32]>,
}

struct StoredArtifact {
    descriptor: LocalArtifact,
    snapshot: tempfile::NamedTempFile,
}

impl Default for LocalArtifactService {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(BTreeMap::new())),
            namespace: Arc::new(artifact_namespace()),
        }
    }
}

impl LocalArtifactService {
    pub(crate) fn admit(
        &self,
        root: &Path,
        requested: &Path,
        media_type: &str,
        bounds: super::WorkspaceBounds,
    ) -> Result<LocalArtifact, WorkspaceFileError> {
        let source = WorkspaceFile::resolve(root, requested, bounds)?;
        let media_type = media_type.trim();
        if media_type.is_empty() {
            return Err(WorkspaceFileError::InvalidMediaType);
        }
        if source.bytes() > MAX_ARTIFACT_BYTES {
            return Err(WorkspaceFileError::ArtifactTooLarge {
                path: requested.to_path_buf(),
                max_bytes: MAX_ARTIFACT_BYTES,
            });
        }

        let mut snapshot =
            tempfile::NamedTempFile::new().map_err(WorkspaceFileError::SnapshotUnavailable)?;
        let (bytes, whole_digest) = source
            .copy_and_digest(snapshot.as_file_mut(), MAX_ARTIFACT_BYTES)
            .map_err(|error| match error {
                WorkspaceFileError::TooLarge { path, max_bytes } => {
                    WorkspaceFileError::ArtifactTooLarge { path, max_bytes }
                }
                error => error,
            })?;
        snapshot
            .as_file_mut()
            .sync_all()
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        let reference =
            artifact_reference(&self.namespace, media_type, bytes, whole_digest.as_str());
        let descriptor = LocalArtifact {
            reference: reference.clone(),
            media_type: media_type.to_owned(),
            bytes,
            whole_digest: whole_digest.into_string(),
        };
        let stored = Arc::new(StoredArtifact {
            descriptor: descriptor.clone(),
            snapshot,
        });
        let mut artifacts = self
            .inner
            .lock()
            .map_err(|_| WorkspaceFileError::ArtifactStoreUnavailable)?;
        artifacts.entry(reference).or_insert(stored);
        Ok(descriptor)
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
        let artifact = self.resolve(reference)?;
        if offset > artifact.descriptor.bytes {
            return Err(WorkspaceFileError::InvalidArtifactOffset {
                offset,
                bytes: artifact.descriptor.bytes,
            });
        }
        let mut file = artifact
            .snapshot
            .reopen()
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        let remaining = artifact.descriptor.bytes.saturating_sub(offset);
        let capture = usize::try_from(remaining.min((maximum + 4) as u64)).unwrap_or(maximum + 4);
        let mut bytes = vec![0; capture];
        file.read_exact(&mut bytes)
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        if encoding == ArtifactPageEncoding::Utf8 && bytes.len() > maximum {
            bytes.truncate(maximum);
            while std::str::from_utf8(&bytes).is_err_and(|error| error.error_len().is_none()) {
                bytes.pop();
            }
        } else if bytes.len() > maximum {
            bytes.truncate(maximum);
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
            eof: next_offset == artifact.descriptor.bytes,
            range_digest: runx_contracts::sha256_prefixed(&bytes),
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
        if maximum == 0 || maximum > MAX_ARTIFACT_PAGE_BYTES {
            return Err(WorkspaceFileError::InvalidPageSize {
                maximum: MAX_ARTIFACT_PAGE_BYTES,
            });
        }
        let artifact = self.resolve(reference)?;
        if offset > artifact.descriptor.bytes {
            return Err(WorkspaceFileError::InvalidArtifactOffset {
                offset,
                bytes: artifact.descriptor.bytes,
            });
        }
        let file = artifact
            .snapshot
            .reopen()
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        let frame = frame_json_array_page(file, offset, maximum, maximum_encoded_records)?;

        let range = read_exact_range(&artifact.snapshot, offset, frame.next_offset)?;
        Ok(ArtifactRecordPage {
            artifact: artifact.descriptor.clone(),
            offset,
            length: frame.next_offset.saturating_sub(offset),
            next_offset: frame.next_offset,
            eof: frame.eof,
            range_digest: runx_contracts::sha256_prefixed(&range),
            records: frame.records,
        })
    }

    fn resolve(&self, reference: &str) -> Result<Arc<StoredArtifact>, WorkspaceFileError> {
        if !reference.starts_with("runx:local-artifact:sha256:") {
            return Err(WorkspaceFileError::InvalidArtifactReference);
        }
        self.inner
            .lock()
            .map_err(|_| WorkspaceFileError::ArtifactStoreUnavailable)?
            .get(reference)
            .cloned()
            .ok_or(WorkspaceFileError::UnknownArtifactReference)
    }
}

fn read_exact_range(
    snapshot: &tempfile::NamedTempFile,
    offset: u64,
    end: u64,
) -> Result<Vec<u8>, WorkspaceFileError> {
    let mut file = snapshot
        .reopen()
        .map_err(WorkspaceFileError::SnapshotUnavailable)?;
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

fn artifact_reference(
    namespace: &[u8; 32],
    media_type: &str,
    bytes: u64,
    whole_digest: &str,
) -> String {
    let mut identity = namespace.to_vec();
    identity.extend_from_slice(format!("\0{media_type}\0{bytes}\0{whole_digest}").as_bytes());
    format!(
        "runx:local-artifact:{}",
        runx_contracts::sha256_prefixed(&identity)
    )
}

fn artifact_namespace() -> [u8; 32] {
    let mut namespace = [0_u8; 32];
    if SystemRandom::new().fill(&mut namespace).is_ok() {
        return namespace;
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static FALLBACK_SEQUENCE: AtomicU64 = AtomicU64::new(1);
    let fallback = format!(
        "{}:{}:{:?}",
        std::process::id(),
        FALLBACK_SEQUENCE.fetch_add(1, Ordering::Relaxed),
        std::time::SystemTime::now()
    );
    namespace.copy_from_slice(&Sha256::digest(fallback.as_bytes()));
    namespace
}

#[cfg(test)]
mod tests;
