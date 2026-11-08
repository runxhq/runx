use std::path::{Path, PathBuf};

use crate::RuntimeError;

use super::invalid_input;

pub(crate) fn resolve_repo_root_for(
    tool: &str,
    requested: &str,
    env: &std::collections::BTreeMap<String, String>,
    skill_directory: &Path,
    reach: runx_contracts::Reach,
) -> Result<PathBuf, RuntimeError> {
    crate::services::resolve_scoped_root(requested, "workspace", env, skill_directory, reach)
        .map_err(|error| invalid_input(tool, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reach_inputs_roots_accept_workspace_relative_and_explicit_absolute_paths()
    -> Result<(), Box<dyn std::error::Error>> {
        let inputs = runx_contracts::Reach::Inputs;
        let workspace = tempfile::tempdir()?;
        let external = tempfile::tempdir()?;
        let env = std::collections::BTreeMap::from([(
            crate::paths::RUNX_CWD_ENV.to_owned(),
            workspace.path().to_string_lossy().into_owned(),
        )]);

        assert_eq!(
            resolve_repo_root_for("fs.read", ".", &env, workspace.path(), inputs)?,
            workspace.path().canonicalize()?
        );
        assert_eq!(
            resolve_repo_root_for(
                "fs.read",
                &external.path().to_string_lossy(),
                &env,
                workspace.path(),
                inputs,
            )?,
            external.path().canonicalize()?
        );
        Ok(())
    }
}
