//! The persisted form of a local snapshot. The run's existing artifact root
//! owns it; no original file is reopened when a later process reads a page.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::MAX_ARTIFACT_BYTES;
use super::reader::SnapshotReader;
use crate::services::{WorkspaceFile, WorkspaceFileError};
use runx_contracts::schema::NonEmptyString;
use runx_contracts::{Artifact, ArtifactLabels, ArtifactSchema, Reach};

pub(super) const SNAPSHOT_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const SNAPSHOT_FORMAT: &str = "runx.artifact_snapshot.v1";
/// The format a snapshot had before runx 0.12, which is refused by name.
const SNAPSHOT_FORMAT_BEFORE_THIS_RUNX: &str = "runx.local-artifact-snapshot.v1";
const REFERENCE_PREFIX: &str = "runx:local-artifact:sha256:";

enum Scope {
    Temporary(Mutex<Option<tempfile::TempDir>>),
    Run { project: PathBuf, namespace: String },
}

pub(super) struct SnapshotStore {
    scope: Scope,
}

impl Default for SnapshotStore {
    fn default() -> Self {
        Self {
            scope: Scope::Temporary(Mutex::new(None)),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotManifest {
    format: String,
    media_type: String,
    bytes: u64,
    whole_digest: runx_contracts::Digest,
    chunks: Vec<String>,
    /// Where the snapshot may be shared: its source's labels, kept with the
    /// bytes so every later read carries them.
    labels: ArtifactLabels,
}

/// Only a record's format, read first so a snapshot of another format is
/// named before its fields are.
#[derive(Deserialize)]
struct SnapshotFormat {
    format: String,
}

pub(super) struct StoredArtifact {
    pub(super) descriptor: Artifact,
    root: PathBuf,
    name: String,
    manifest: SnapshotManifest,
}

impl StoredArtifact {
    pub(super) fn reader(&self) -> Result<SnapshotReader, WorkspaceFileError> {
        let file = checked_file(&self.root, &format!("{}.blob", self.name))?;
        SnapshotReader::new(file, self.manifest.bytes, self.manifest.chunks.clone())
            .map_err(WorkspaceFileError::SnapshotUnavailable)
    }
}

impl SnapshotStore {
    pub(super) fn for_run(project: &Path, root_run_id: &str) -> Self {
        let mut identity = b"runx.local-artifact-scope.v1\0".to_vec();
        // The protected project root provides containment. Its absolute path
        // must not change a paused run's identity after a move or remount.
        identity.extend_from_slice(root_run_id.as_bytes());
        Self {
            scope: Scope::Run {
                project: project.to_path_buf(),
                namespace: runx_contracts::sha256_hex(&identity),
            },
        }
    }

    fn directory(&self, create: bool) -> Result<(PathBuf, String), WorkspaceFileError> {
        match &self.scope {
            Scope::Temporary(root) => {
                let mut root = root
                    .lock()
                    .map_err(|_| WorkspaceFileError::ArtifactStoreUnavailable)?;
                if root.is_none() {
                    if !create {
                        return Err(WorkspaceFileError::UnknownArtifactReference);
                    }
                    *root =
                        Some(tempfile::tempdir().map_err(WorkspaceFileError::SnapshotUnavailable)?);
                }
                let root = root
                    .as_ref()
                    .ok_or(WorkspaceFileError::ArtifactStoreUnavailable)?
                    .path()
                    .to_path_buf();
                let namespace = runx_contracts::sha256_hex(root.as_os_str().as_encoded_bytes());
                Ok((root, namespace))
            }
            Scope::Run { project, namespace } => {
                let mut directory = project.clone();
                for segment in [crate::paths::ARTIFACTS_DIR, "snapshots", namespace.as_str()] {
                    directory.push(segment);
                    checked_directory(&directory, create)?;
                }
                Ok((directory, namespace.clone()))
            }
        }
    }

    pub(super) fn capture(
        &self,
        source: WorkspaceFile,
        media_type: &str,
        expected: Option<&Artifact>,
        labels: ArtifactLabels,
    ) -> Result<Artifact, WorkspaceFileError> {
        let media_type = media_type.trim();
        if media_type.is_empty() {
            return Err(WorkspaceFileError::InvalidMediaType);
        }
        if source.bytes() > MAX_ARTIFACT_BYTES {
            return Err(WorkspaceFileError::ArtifactTooLarge {
                path: source.relative_path.clone().into(),
                max_bytes: MAX_ARTIFACT_BYTES,
            });
        }
        let (root, namespace) = self.directory(true)?;
        let mut snapshot = tempfile::NamedTempFile::new_in(&root)
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        let mut indexed = SnapshotWriter::new(snapshot.as_file_mut());
        let (bytes, whole_digest) = source
            .copy_and_digest(
                &mut indexed,
                expected.map_or(MAX_ARTIFACT_BYTES, |artifact| artifact.size_bytes),
            )
            .map_err(|error| match error {
                WorkspaceFileError::TooLarge { path, max_bytes } => {
                    WorkspaceFileError::ArtifactTooLarge { path, max_bytes }
                }
                other => other,
            })?;
        let chunks = indexed.finish();
        if expected
            .is_some_and(|artifact| artifact.size_bytes != bytes || artifact.digest != whole_digest)
        {
            return Err(WorkspaceFileError::ArtifactDescriptorMismatch);
        }
        snapshot
            .as_file_mut()
            .sync_all()
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        let manifest = SnapshotManifest {
            format: SNAPSHOT_FORMAT.to_owned(),
            media_type: media_type.to_owned(),
            bytes,
            whole_digest,
            chunks,
            labels,
        };
        let encoded = crate::services::bounded_json_bytes(&manifest, MAX_MANIFEST_BYTES)
            .map_err(|_| WorkspaceFileError::ArtifactStoreUnavailable)?;
        let reference = reference(&namespace, &encoded);
        let name = reference_name(&reference)?;
        let stored = StoredArtifact {
            descriptor: descriptor(&reference, &manifest),
            root: root.clone(),
            name: name.to_owned(),
            manifest,
        };
        if !publish(snapshot, &root.join(format!("{name}.blob")))? {
            // The existing content-addressed object must actually be identical.
            std::io::copy(&mut stored.reader()?, &mut std::io::sink())
                .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        }
        let mut record = tempfile::NamedTempFile::new_in(&root)
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        record
            .write_all(&encoded)
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        record
            .as_file()
            .sync_all()
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        if !publish(record, &root.join(format!("{name}.json")))? {
            let existing = self.resolve(&reference)?;
            if existing.descriptor != stored.descriptor {
                return Err(WorkspaceFileError::SnapshotIntegrity);
            }
        }
        Ok(stored.descriptor)
    }

    pub(super) fn resolve(&self, reference: &str) -> Result<StoredArtifact, WorkspaceFileError> {
        let name = reference_name(reference)?;
        let (root, namespace) = self.directory(false)?;
        let mut record = checked_file(&root, &format!("{name}.json"))?;
        let mut encoded = Vec::new();
        Read::by_ref(&mut record)
            .take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut encoded)
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        if encoded.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(WorkspaceFileError::SnapshotIntegrity);
        }
        if serde_json::from_slice::<SnapshotFormat>(&encoded)
            .is_ok_and(|record| record.format == SNAPSHOT_FORMAT_BEFORE_THIS_RUNX)
        {
            return Err(WorkspaceFileError::SnapshotBeforeThisRunx);
        }
        let manifest: SnapshotManifest =
            serde_json::from_slice(&encoded).map_err(|_| WorkspaceFileError::SnapshotIntegrity)?;
        let canonical = crate::services::bounded_json_bytes(&manifest, MAX_MANIFEST_BYTES)
            .map_err(|_| WorkspaceFileError::SnapshotIntegrity)?;
        if manifest.format != SNAPSHOT_FORMAT
            || manifest.media_type.is_empty()
            || manifest.bytes > MAX_ARTIFACT_BYTES
            || manifest.chunks.len() as u64 != manifest.bytes.div_ceil(SNAPSHOT_CHUNK_BYTES as u64)
            || manifest
                .chunks
                .iter()
                .any(|digest| runx_contracts::Digest::new(digest).is_none())
            || reference != self::reference(&namespace, &canonical)
        {
            return Err(WorkspaceFileError::SnapshotIntegrity);
        }
        Ok(StoredArtifact {
            descriptor: descriptor(reference, &manifest),
            root,
            name: name.to_owned(),
            manifest,
        })
    }
}

/// A snapshot as the artifact it is. Its bytes are never in the project, so
/// it names no path.
fn descriptor(reference: &str, manifest: &SnapshotManifest) -> Artifact {
    Artifact {
        schema: ArtifactSchema::V1,
        reference: NonEmptyString::from(reference),
        digest: manifest.whole_digest.clone(),
        size_bytes: manifest.bytes,
        media_type: NonEmptyString::from(manifest.media_type.as_str()),
        created_at: None,
        produced_by: None,
        path: None,
        summary: None,
        sources: Vec::new(),
        labels: manifest.labels,
    }
}

fn reference(namespace: &str, manifest: &[u8]) -> String {
    let mut identity = namespace.as_bytes().to_vec();
    identity.push(0);
    identity.extend_from_slice(manifest);
    format!(
        "runx:local-artifact:{}",
        runx_contracts::sha256_prefixed(&identity)
    )
}

fn reference_name(reference: &str) -> Result<&str, WorkspaceFileError> {
    let name = reference
        .strip_prefix(REFERENCE_PREFIX)
        .ok_or(WorkspaceFileError::InvalidArtifactReference)?;
    if name.len() != 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(WorkspaceFileError::InvalidArtifactReference);
    }
    Ok(name)
}

fn checked_directory(path: &Path, create: bool) -> Result<(), WorkspaceFileError> {
    if create {
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(WorkspaceFileError::SnapshotUnavailable(error)),
        }
    }
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkspaceFileError::UnknownArtifactReference
        } else {
            WorkspaceFileError::SnapshotUnavailable(error)
        }
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(WorkspaceFileError::ArtifactStoreUnavailable);
    }
    #[cfg(unix)]
    if create {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(WorkspaceFileError::SnapshotUnavailable)?;
    }
    Ok(())
}

fn checked_file(root: &Path, name: &str) -> Result<File, WorkspaceFileError> {
    let path = root.join(name);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            WorkspaceFileError::UnknownArtifactReference
        } else {
            WorkspaceFileError::SnapshotUnavailable(error)
        }
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(WorkspaceFileError::ArtifactStoreUnavailable);
    }
    let source = WorkspaceFile::resolve(root, Path::new(name), Reach::Inputs)?;
    File::open(source.path).map_err(WorkspaceFileError::SnapshotUnavailable)
}

fn publish(file: tempfile::NamedTempFile, path: &Path) -> Result<bool, WorkspaceFileError> {
    match file.persist_noclobber(path) {
        Ok(_) => Ok(true),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(WorkspaceFileError::SnapshotUnavailable(error.error)),
    }
}

struct SnapshotWriter<'a> {
    file: &'a mut File,
    hash: Sha256,
    used: usize,
    chunks: Vec<String>,
}

impl<'a> SnapshotWriter<'a> {
    fn new(file: &'a mut File) -> Self {
        Self {
            file,
            hash: Sha256::new(),
            used: 0,
            chunks: Vec::new(),
        }
    }

    fn record_chunk(&mut self) {
        self.chunks.push(format!(
            "sha256:{}",
            runx_contracts::hex_lower(&self.hash.finalize_reset())
        ));
        self.used = 0;
    }

    fn finish(mut self) -> Vec<String> {
        if self.used > 0 {
            self.record_chunk();
        }
        self.chunks
    }
}

impl Write for SnapshotWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.file.write_all(bytes)?;
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let length = remaining.len().min(SNAPSHOT_CHUNK_BYTES - self.used);
            self.hash.update(&remaining[..length]);
            self.used += length;
            remaining = &remaining[length..];
            if self.used == SNAPSHOT_CHUNK_BYTES {
                self.record_chunk();
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
