use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const MAX_DOCUMENT_INPUT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DocumentInputSource {
    Path(PathBuf),
    Stdin,
}

#[derive(Debug)]
pub enum DocumentInputError {
    Workspace {
        location: String,
        source: runx_runtime::WorkspaceFileError,
    },
    Read {
        location: String,
        source: io::Error,
    },
    TooLarge {
        location: String,
        max_bytes: u64,
    },
    InvalidUtf8 {
        location: String,
    },
}

impl DocumentInputError {
    pub fn is_stdin(&self) -> bool {
        match self {
            Self::Workspace { .. } => false,
            Self::Read { location, .. }
            | Self::TooLarge { location, .. }
            | Self::InvalidUtf8 { location } => location == "stdin",
        }
    }
}

impl fmt::Display for DocumentInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace { location, source } => write!(formatter, "{location}: {source}"),
            Self::Read { location, source } => write!(formatter, "{location}: {source}"),
            Self::TooLarge {
                location,
                max_bytes,
            } => write!(
                formatter,
                "{location} exceeds the {max_bytes}-byte structured-input limit"
            ),
            Self::InvalidUtf8 { location } => {
                write!(formatter, "{location} is not valid UTF-8")
            }
        }
    }
}

impl std::error::Error for DocumentInputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Workspace { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
            Self::TooLarge { .. } | Self::InvalidUtf8 { .. } => None,
        }
    }
}

pub fn read_document_input(
    source: &DocumentInputSource,
    env: &BTreeMap<String, String>,
    cwd: &Path,
) -> Result<String, DocumentInputError> {
    read_document_input_with(source, env, cwd, io::stdin())
}

pub fn read_document_input_with<R: Read>(
    source: &DocumentInputSource,
    env: &BTreeMap<String, String>,
    cwd: &Path,
    stdin: R,
) -> Result<String, DocumentInputError> {
    match source {
        DocumentInputSource::Path(path) => {
            let location = path.display().to_string();
            let root = runx_runtime::RunxPaths::resolve(env, cwd)
                .workspace()
                .to_path_buf();
            runx_runtime::read_workspace_text(&root, path, MAX_DOCUMENT_INPUT_BYTES).map_err(
                |source| DocumentInputError::Workspace {
                    location: location.clone(),
                    source,
                },
            )
        }
        DocumentInputSource::Stdin => {
            read_bounded_utf8(stdin, "stdin".to_owned(), MAX_DOCUMENT_INPUT_BYTES)
        }
    }
}

fn read_bounded_utf8<R: Read>(
    reader: R,
    location: String,
    max_bytes: u64,
) -> Result<String, DocumentInputError> {
    let mut bytes = Vec::new();
    reader
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| DocumentInputError::Read {
            location: location.clone(),
            source,
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > max_bytes {
        return Err(DocumentInputError::TooLarge {
            location,
            max_bytes,
        });
    }
    String::from_utf8(bytes).map_err(|_| DocumentInputError::InvalidUtf8 { location })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{DocumentInputError, read_bounded_utf8};

    #[test]
    fn bounded_reader_rejects_overflow_and_invalid_utf8() {
        let oversized = read_bounded_utf8(Cursor::new(b"12345"), "stdin".to_owned(), 4);
        assert!(matches!(
            oversized,
            Err(DocumentInputError::TooLarge { .. })
        ));

        let invalid = read_bounded_utf8(Cursor::new([0xff]), "stdin".to_owned(), 4);
        assert!(matches!(
            invalid,
            Err(DocumentInputError::InvalidUtf8 { .. })
        ));
    }
}
