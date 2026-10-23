use std::path::Path;

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

/// A resolved root as a native packet names it.
pub(crate) fn root_name(tool: &str, root: &Path) -> Result<NonEmptyString, RuntimeError> {
    non_empty(tool, "repo_root", root.to_string_lossy())
}

pub(crate) fn invalid_input(tool: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: tool.to_owned(),
        message: message.into(),
    }
}
