use std::fs::File;
use std::io::{Read as _, Write};
use std::path::{Component, Path, PathBuf};

use sha2::{Digest as _, Sha256};
use thiserror::Error;

mod artifacts;
mod project_artifacts;

pub use project_artifacts::{persist_json_artifact, read_json_artifact};

pub(crate) use artifacts::{
    ArtifactPageEncoding, ArtifactRecordPage, DEFAULT_ARTIFACT_PAGE_BYTES, LocalArtifact,
    LocalArtifactService,
};

pub(crate) struct WorkspaceFile {
    path: PathBuf,
    relative_path: String,
    bytes: u64,
}

pub(crate) struct WorkspaceTextFile {
    pub(crate) relative_path: String,
    pub(crate) contents: String,
    pub(crate) bytes: u64,
    pub(crate) truncated: bool,
    pub(crate) digest: String,
}

#[derive(Debug, Error)]
pub enum WorkspaceFileError {
    #[error("workspace file path must be a non-empty relative path")]
    InvalidPath,
    #[error("workspace root is unavailable: {0}")]
    RootUnavailable(std::io::Error),
    #[error("workspace root is not a directory")]
    RootNotDirectory,
    #[error("path_scope must be workspace or skill")]
    InvalidPathScope,
    #[error("workspace file {path} is unavailable: {source}")]
    Unavailable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("workspace file {0} escapes the workspace root")]
    EscapesRoot(PathBuf),
    #[error("workspace file {0} is not a regular file")]
    NotAFile(PathBuf),
    #[error("workspace file {path} exceeds the {max_bytes}-byte structured-input limit")]
    TooLarge { path: PathBuf, max_bytes: u64 },
    #[error("workspace file {0} is not valid UTF-8")]
    InvalidUtf8(PathBuf),
    #[error("local artifact media_type must be non-empty")]
    InvalidMediaType,
    #[error("workspace artifact {path} exceeds the {max_bytes}-byte admission limit")]
    ArtifactTooLarge { path: PathBuf, max_bytes: u64 },
    #[error("local artifact snapshot is unavailable: {0}")]
    SnapshotUnavailable(std::io::Error),
    #[error("local artifact store is unavailable")]
    ArtifactStoreUnavailable,
    #[error("local artifact reference is malformed")]
    InvalidArtifactReference,
    #[error("local artifact reference is unknown in this invocation")]
    UnknownArtifactReference,
    #[error("artifact page size must be between 1 and {maximum} bytes")]
    InvalidPageSize { maximum: usize },
    #[error("artifact offset {offset} exceeds artifact length {bytes}")]
    InvalidArtifactOffset { offset: u64, bytes: u64 },
    #[error("artifact UTF-8 page offset {0} is not a character boundary")]
    InvalidArtifactUtf8Offset(u64),
    #[error("artifact JSON-array framing failed at byte {offset}: {message}")]
    InvalidArtifactFraming { offset: u64, message: String },
}

pub fn read_workspace_text(
    root: &Path,
    requested: &Path,
    max_bytes: u64,
) -> Result<String, WorkspaceFileError> {
    let source = WorkspaceFile::resolve(root, requested)?;
    if source.bytes > max_bytes {
        return Err(WorkspaceFileError::TooLarge {
            path: requested.to_path_buf(),
            max_bytes,
        });
    }
    Ok(source.read_text(max_bytes)?.contents)
}

impl WorkspaceFile {
    pub(crate) fn resolve(root: &Path, requested: &Path) -> Result<Self, WorkspaceFileError> {
        validate_relative_file_path(requested)?;
        let root = std::fs::canonicalize(root).map_err(WorkspaceFileError::RootUnavailable)?;
        let path = std::fs::canonicalize(root.join(requested)).map_err(|source| {
            WorkspaceFileError::Unavailable {
                path: requested.to_path_buf(),
                source,
            }
        })?;
        if path != root && !path.starts_with(&root) {
            return Err(WorkspaceFileError::EscapesRoot(requested.to_path_buf()));
        }
        if !path.is_file() {
            return Err(WorkspaceFileError::NotAFile(requested.to_path_buf()));
        }
        let bytes = path
            .metadata()
            .map_err(|source| WorkspaceFileError::Unavailable {
                path: requested.to_path_buf(),
                source,
            })?
            .len();
        let relative_path = path
            .strip_prefix(&root)
            .map_err(|_| WorkspaceFileError::EscapesRoot(requested.to_path_buf()))?
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        Ok(Self {
            path,
            relative_path,
            bytes,
        })
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn read_text(
        &self,
        max_bytes: u64,
    ) -> Result<WorkspaceTextFile, WorkspaceFileError> {
        if self.bytes > max_bytes {
            return Ok(WorkspaceTextFile {
                relative_path: self.relative_path.clone(),
                contents: String::new(),
                bytes: self.bytes,
                truncated: true,
                digest: String::new(),
            });
        }
        let capacity = usize::try_from(self.bytes.min(max_bytes)).unwrap_or(0);
        let mut bytes = Vec::with_capacity(capacity);
        let (bytes_read, digest) = self.copy_and_digest(&mut bytes, max_bytes)?;
        let contents = String::from_utf8(bytes)
            .map_err(|_| WorkspaceFileError::InvalidUtf8(self.relative_path.clone().into()))?;
        Ok(WorkspaceTextFile {
            relative_path: self.relative_path.clone(),
            contents,
            bytes: bytes_read,
            truncated: false,
            digest,
        })
    }

    pub(crate) fn copy_and_digest(
        &self,
        target: &mut impl Write,
        max_bytes: u64,
    ) -> Result<(u64, String), WorkspaceFileError> {
        if self.bytes > max_bytes {
            return Err(WorkspaceFileError::ArtifactTooLarge {
                path: self.relative_path.clone().into(),
                max_bytes,
            });
        }
        let mut source =
            File::open(&self.path).map_err(|source| WorkspaceFileError::Unavailable {
                path: self.relative_path.clone().into(),
                source,
            })?;
        let mut digest = Sha256::new();
        let mut total = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let read =
                source
                    .read(&mut buffer)
                    .map_err(|source| WorkspaceFileError::Unavailable {
                        path: self.relative_path.clone().into(),
                        source,
                    })?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| WorkspaceFileError::TooLarge {
                    path: self.relative_path.clone().into(),
                    max_bytes,
                })?;
            if total > max_bytes {
                return Err(WorkspaceFileError::TooLarge {
                    path: self.relative_path.clone().into(),
                    max_bytes,
                });
            }
            digest.update(&buffer[..read]);
            target
                .write_all(&buffer[..read])
                .map_err(WorkspaceFileError::SnapshotUnavailable)?;
        }
        Ok((
            total,
            format!("sha256:{}", runx_contracts::hex_lower(&digest.finalize())),
        ))
    }
}

fn validate_relative_file_path(requested: &Path) -> Result<(), WorkspaceFileError> {
    if requested.as_os_str().is_empty()
        || requested.is_absolute()
        || requested
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(WorkspaceFileError::InvalidPath);
    }
    Ok(())
}

pub(crate) fn resolve_scoped_root(
    requested_root: &str,
    path_scope: &str,
    env: &std::collections::BTreeMap<String, String>,
    skill_directory: &Path,
) -> Result<PathBuf, WorkspaceFileError> {
    if path_scope == "skill" {
        let root =
            std::fs::canonicalize(skill_directory).map_err(WorkspaceFileError::RootUnavailable)?;
        return root
            .is_dir()
            .then_some(root)
            .ok_or(WorkspaceFileError::RootNotDirectory);
    }
    if path_scope != "workspace" {
        return Err(WorkspaceFileError::InvalidPathScope);
    }
    let workspace = crate::RunxPaths::resolve(env, skill_directory)
        .workspace()
        .to_path_buf();
    let workspace =
        std::fs::canonicalize(workspace).map_err(WorkspaceFileError::RootUnavailable)?;
    if !workspace.is_dir() {
        return Err(WorkspaceFileError::RootNotDirectory);
    }
    let requested = Path::new(requested_root);
    let unresolved = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        workspace.join(requested)
    };
    let root = std::fs::canonicalize(unresolved).map_err(WorkspaceFileError::RootUnavailable)?;
    if !root.is_dir() {
        return Err(WorkspaceFileError::RootNotDirectory);
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::read_workspace_text;

    #[test]
    fn reads_only_bounded_contained_utf8_files() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("inputs"))?;
        std::fs::write(root.path().join("inputs/value.json"), "{\"ok\":true}")?;

        assert_eq!(
            read_workspace_text(root.path(), std::path::Path::new("inputs/value.json"), 64)?,
            "{\"ok\":true}"
        );
        assert!(
            read_workspace_text(root.path(), std::path::Path::new("../value.json"), 64).is_err()
        );
        assert!(
            read_workspace_text(root.path(), &root.path().join("inputs/value.json"), 64).is_err()
        );
        assert!(
            read_workspace_text(root.path(), std::path::Path::new("inputs/value.json"), 4).is_err()
        );
        Ok(())
    }
}
