use std::fmt;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub enum ToolCatalogError {
    Io {
        action: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    Json {
        action: &'static str,
        path: PathBuf,
        source: serde_json::Error,
    },
    InvalidManifest {
        path: PathBuf,
        message: String,
    },
    InvalidRequest(String),
    /// A tool name with no namespace, which names no catalog tool.
    Unqualified(String),
    NotFound(String),
}

impl ToolCatalogError {
    pub(crate) fn io(action: &'static str, path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn json(
        action: &'static str,
        path: impl Into<PathBuf>,
        source: serde_json::Error,
    ) -> Self {
        Self::Json {
            action,
            path: path.into(),
            source,
        }
    }

    pub(crate) fn concise_message(&self) -> String {
        match self {
            Self::InvalidManifest { message, .. }
            | Self::InvalidRequest(message)
            | Self::Unqualified(message)
            | Self::NotFound(message) => message.clone(),
            Self::Io {
                action,
                path,
                source,
            } => format!("{action} {}: {source}", path.display()),
            Self::Json {
                action,
                path,
                source,
            } => format!("{action} {}: {source}", path.display()),
        }
    }
}

impl fmt::Display for ToolCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                action,
                path,
                source,
            } => write!(formatter, "{action} {}: {source}", path.display()),
            Self::Json {
                action,
                path,
                source,
            } => write!(formatter, "{action} {}: {source}", path.display()),
            Self::InvalidManifest { path, message } => {
                write!(formatter, "{}: {message}", path.display())
            }
            Self::InvalidRequest(message)
            | Self::Unqualified(message)
            | Self::NotFound(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ToolCatalogError {}
