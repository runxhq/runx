//! Local package lookup shared by execution and host export.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{RuntimeError, RunxPaths};

/// Resolve an explicit local package or one under the caller's declared roots.
/// A directory containing several installed versions requires an exact path.
/// Package validation and execution admission remain at their existing boundary.
pub fn resolve_local_skill_reference(
    paths: &RunxPaths,
    reference: &Path,
    source_roots: &[PathBuf],
) -> Result<Option<PathBuf>, RuntimeError> {
    if let Some(directory) = skill_directory(&paths.existing_input(reference)) {
        return Ok(Some(directory));
    }
    for root in source_roots {
        let candidate = root.join(reference);
        if let Some(directory) = skill_directory(&candidate) {
            return Ok(Some(directory));
        }
        let entries = match fs::read_dir(&candidate) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(RuntimeError::io(
                    format!("reading skill source {}", candidate.display()),
                    source,
                ));
            }
        };
        let mut versions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| {
                RuntimeError::io(
                    format!("reading skill source {}", candidate.display()),
                    source,
                )
            })?;
            if let Some(directory) = skill_directory(&entry.path()) {
                versions.push(directory);
            }
        }
        match versions.len() {
            0 => {}
            1 => return Ok(versions.pop()),
            _ => {
                return Err(RuntimeError::SkillFailed {
                    skill_name: reference.to_string_lossy().into_owned(),
                    message: format!(
                        "skill reference is ambiguous in {}; use an exact package path",
                        candidate.display()
                    ),
                });
            }
        }
    }
    Ok(None)
}

fn skill_directory(candidate: &Path) -> Option<PathBuf> {
    let directory = if candidate.file_name().is_some_and(|name| name == "SKILL.md") {
        candidate.parent()?
    } else {
        candidate
    };
    directory
        .join("SKILL.md")
        .is_file()
        .then(|| directory.to_path_buf())
}
