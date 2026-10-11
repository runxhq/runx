use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::RunxExportLoadError;
use super::discovery::canonicalize;

pub(super) fn resolve_skill_ref(
    root: &Path,
    reference: &str,
    source_roots: &[PathBuf],
    execution_env: Option<&BTreeMap<String, String>>,
) -> Result<PathBuf, RunxExportLoadError> {
    let empty_env = BTreeMap::new();
    let env = execution_env.unwrap_or(&empty_env);
    let paths = crate::RunxPaths::resolve(env, root);
    let mut roots = paths.skill_roots(env);
    for source_root in source_roots {
        if !roots.contains(source_root) {
            roots.push(source_root.clone());
        }
    }
    if let Some(directory) =
        crate::resolve_local_skill_reference(&paths, Path::new(reference), &roots)
            .map_err(|error| RunxExportLoadError::Parse(error.to_string()))?
    {
        return canonicalize(&directory, "canonicalizing local skill reference");
    }
    if let Some(installed) = crate::registry::materialize_bundled_skill(reference, &paths, None)
        .map_err(RunxExportLoadError::Parse)?
    {
        let directory = installed.destination.parent().ok_or_else(|| {
            RunxExportLoadError::InvalidArgs("bundled skill has no package directory".to_owned())
        })?;
        return canonicalize(directory, "canonicalizing bundled skill reference");
    }
    Err(RunxExportLoadError::InvalidArgs(format!(
        "skill reference {reference} does not resolve to a SKILL.md package"
    )))
}
