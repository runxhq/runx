use runx_contracts::schema::NonEmptyString;

use crate::RuntimeError;

/// A value a native packet requires to be non-empty.
pub(crate) fn non_empty(
    tool: &str,
    field: &str,
    value: impl Into<String>,
) -> Result<NonEmptyString, RuntimeError> {
    NonEmptyString::new(value).ok_or_else(|| invalid_input(tool, format!("{field} is empty")))
}

/// The repository a git tool's result names: the one its caller named, never
/// where it lies on disk.
pub(crate) fn root_name(tool: &str, named: &str) -> Result<NonEmptyString, RuntimeError> {
    non_empty(tool, "repository", named)
}

pub(crate) fn invalid_input(tool: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::failed(tool.to_owned(), message.into())
}
