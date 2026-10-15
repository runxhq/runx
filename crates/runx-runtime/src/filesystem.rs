use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::JsonObject;

use crate::RuntimeError;
use crate::capability::LocalEffect;
use crate::services::WorkspaceBounds;

mod admission;
mod apply;
mod effect;

pub(crate) use effect::text_bundle_effect;

use admission::{admit_file_target, admit_text_bundle, missing_parent_directories, snapshot_paths};
use apply::{apply_admitted_bundle, bundle_report, rollback_paths};

const MAX_BUNDLE_OPERATIONS: usize = 500;
const MAX_FILESYSTEM_MUTATION_BUNDLE_BYTES: usize = 20 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct DirectoryEntry {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) is_dir: bool,
    pub(crate) is_file: bool,
}

pub(crate) fn read_dir_sorted(directory: &Path) -> Result<Vec<DirectoryEntry>, RuntimeError> {
    match fs::read_dir(directory) {
        Ok(entries) => {
            let mut output = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|source| {
                    RuntimeError::io(format!("reading directory {}", directory.display()), source)
                })?;
                let file_type = entry.file_type().map_err(|source| {
                    RuntimeError::io(
                        format!("reading file type {}", entry.path().display()),
                        source,
                    )
                })?;
                output.push(DirectoryEntry {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: entry.path(),
                    is_dir: file_type.is_dir(),
                    is_file: file_type.is_file(),
                });
            }
            output.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(output)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(source) => Err(RuntimeError::io(
            format!("reading directory {}", directory.display()),
            source,
        )),
    }
}

pub(crate) fn find_files_named(
    directory: &Path,
    file_name: &str,
) -> Result<Vec<PathBuf>, RuntimeError> {
    let mut files = Vec::new();
    for entry in read_dir_sorted(directory)? {
        if entry.is_dir {
            if !matches!(
                entry.name.as_str(),
                ".git" | ".runx" | "node_modules" | "target"
            ) {
                files.extend(find_files_named(&entry.path, file_name)?);
            }
        } else if entry.is_file && entry.name == file_name {
            files.push(entry.path);
        }
    }
    files.sort();
    Ok(files)
}

pub(crate) fn read_to_string(path: &Path) -> Result<String, RuntimeError> {
    fs::read_to_string(path)
        .map_err(|source| RuntimeError::io(format!("reading {}", path.display()), source))
}

/// Resolve one not-yet-existing file target beneath an existing trusted root.
///
/// This is the shared admission boundary for runtime-owned state files and
/// transactional text writes. It rejects absolute paths, traversal, symlink
/// components, and directory targets before a caller creates parent folders.
pub(crate) fn resolve_contained_file_target(
    operation: &str,
    root: &Path,
    requested: &str,
) -> Result<PathBuf, RuntimeError> {
    let root = fs::canonicalize(root).map_err(|source| {
        RuntimeError::io(
            format!("resolving file target root {}", root.display()),
            source,
        )
    })?;
    if !root.is_dir() {
        return Err(invalid_bundle(
            operation,
            "file target root must be a directory",
        ));
    }
    admit_file_target(operation, &root, requested, WorkspaceBounds::Operator)
        .map(|(_, absolute)| absolute)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextFileWrite {
    pub(crate) path: String,
    pub(crate) contents: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextBundle {
    pub(crate) writes: Vec<TextFileWrite>,
    pub(crate) deletes: Vec<String>,
    /// What each write or delete was written against, by the path as it is
    /// named: the content it replaces, or that the file does not exist.
    pub(crate) expected: std::collections::BTreeMap<String, Expected>,
}

/// What a change was written against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Expected {
    /// The digest of the content it replaces, as its writer read it.
    Digest(String),
    /// The file must not exist.
    Absent,
}

/// Apply a bounded text bundle beneath one existing workspace root.
///
/// All paths and rollback snapshots are admitted before mutation. Writes use a
/// same-directory temporary file followed by rename; any later failure restores
/// every touched file from the bounded snapshot. This is the single filesystem
/// mutation owner used by native file and skill-package tools.
///
/// `bounds` are those of whoever named the paths: a native file tool passes
/// its run's, and runx's own staging and records pass `Operator`.
pub(crate) fn apply_text_bundle(
    operation: &str,
    root: &Path,
    bundle: &TextBundle,
    bounds: WorkspaceBounds,
) -> Result<JsonObject, RuntimeError> {
    apply_text_bundle_verified(operation, root, bundle, bounds, || Ok(()))
        .map(|(report, ())| report)
}

/// Apply a bundle and keep its rollback snapshot live through caller-owned
/// verification. A failed postcondition restores the entire admitted bundle.
pub(crate) fn apply_text_bundle_verified<T>(
    operation: &str,
    root: &Path,
    bundle: &TextBundle,
    bounds: WorkspaceBounds,
    verify: impl FnOnce() -> Result<T, RuntimeError>,
) -> Result<(JsonObject, T), RuntimeError> {
    let admitted = admit_text_bundle(operation, root, bundle, bounds)?;
    let snapshots = snapshot_paths(&admitted)?;
    let created_directories = missing_parent_directories(&admitted)?;
    let apply_result = apply_admitted_bundle(&admitted, &snapshots).and_then(|()| verify());
    let verified = match apply_result {
        Ok(verified) => verified,
        Err(error) => {
            let rollback_error = rollback_paths(
                &admitted.operation,
                &admitted.root,
                &snapshots,
                &created_directories,
            )
            .err();
            return Err(match rollback_error {
                Some(rollback) => invalid_bundle(
                    operation,
                    format!("bundle apply failed ({error}); rollback also failed ({rollback})"),
                ),
                None => error,
            });
        }
    };
    Ok((bundle_report(&admitted, &snapshots), verified))
}

fn invalid_bundle(operation: &str, message: impl Into<String>) -> RuntimeError {
    RuntimeError::SkillFailed {
        skill_name: operation.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod bundle_tests {
    use std::fs;

    use super::{
        TextBundle, TextFileWrite, WorkspaceBounds, apply_text_bundle, apply_text_bundle_verified,
    };

    #[test]
    fn bounded_bundle_writes_and_deletes_files() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("old.txt"), "old")?;
        let report = apply_text_bundle(
            "fs.apply_bundle",
            temp.path(),
            &TextBundle {
                writes: vec![TextFileWrite {
                    path: "nested/new.txt".to_owned(),
                    contents: "new".to_owned(),
                }],
                deletes: vec!["old.txt".to_owned()],
                expected: Default::default(),
            },
            WorkspaceBounds::Operator,
        )?;

        assert_eq!(
            fs::read_to_string(temp.path().join("nested/new.txt"))?,
            "new"
        );
        assert!(!temp.path().join("old.txt").exists());
        assert_eq!(
            report["write_count"],
            runx_contracts::JsonValue::Number(runx_contracts::JsonNumber::U64(1))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bounded_bundle_preserves_existing_file_mode() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let script = temp.path().join("run.sh");
        fs::write(&script, "old")?;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755))?;

        apply_text_bundle(
            "fs.apply_bundle",
            temp.path(),
            &TextBundle {
                writes: vec![TextFileWrite {
                    path: "run.sh".to_owned(),
                    contents: "new".to_owned(),
                }],
                deletes: Vec::new(),
                expected: Default::default(),
            },
            WorkspaceBounds::Operator,
        )?;

        assert_eq!(fs::metadata(script)?.permissions().mode() & 0o777, 0o755);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bounded_bundle_rejects_symlink_escape() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        symlink(outside.path(), temp.path().join("linked"))?;
        let result = apply_text_bundle(
            "fs.apply_bundle",
            temp.path(),
            &TextBundle {
                writes: vec![TextFileWrite {
                    path: "linked/escape.txt".to_owned(),
                    contents: "blocked".to_owned(),
                }],
                deletes: Vec::new(),
                expected: Default::default(),
            },
            WorkspaceBounds::Operator,
        );

        assert!(result.is_err());
        assert!(!outside.path().join("escape.txt").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn skill_authoring_partial_write_rollback_restores_workspace()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let first = temp.path().join("first.txt");
        let blocked = temp.path().join("blocked");
        fs::write(&first, "before")?;
        fs::create_dir(&blocked)?;
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o555))?;

        let result = apply_text_bundle(
            "runx.skill.apply",
            temp.path(),
            &TextBundle {
                writes: vec![
                    TextFileWrite {
                        path: "first.txt".to_owned(),
                        contents: "after".to_owned(),
                    },
                    TextFileWrite {
                        path: "blocked/second.txt".to_owned(),
                        contents: "must fail".to_owned(),
                    },
                ],
                deletes: Vec::new(),
                expected: Default::default(),
            },
            WorkspaceBounds::Operator,
        );

        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755))?;
        assert!(result.is_err());
        assert_eq!(fs::read_to_string(first)?, "before");
        assert!(!blocked.join("second.txt").exists());
        Ok(())
    }

    #[test]
    fn verified_bundle_rolls_back_when_postcondition_fails()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let existing = temp.path().join("existing.txt");
        fs::write(&existing, "before")?;

        let result = apply_text_bundle_verified(
            "runx.skill.apply",
            temp.path(),
            &TextBundle {
                writes: vec![
                    TextFileWrite {
                        path: "existing.txt".to_owned(),
                        contents: "after".to_owned(),
                    },
                    TextFileWrite {
                        path: "created.txt".to_owned(),
                        contents: "created".to_owned(),
                    },
                ],
                deletes: Vec::new(),
                expected: Default::default(),
            },
            WorkspaceBounds::Operator,
            || {
                Err::<(), _>(super::invalid_bundle(
                    "runx.skill.apply",
                    "postcondition failed",
                ))
            },
        );

        assert!(result.is_err());
        assert_eq!(fs::read_to_string(existing)?, "before");
        assert!(!temp.path().join("created.txt").exists());
        Ok(())
    }

    #[test]
    fn agent_bounds_refuse_writing_a_secret_or_runx_state() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        fs::create_dir(temp.path().join(".git"))?;
        for path in [
            ".env",
            "app/.env.local",
            ".git/config",
            ".runx/receipts/x.json",
        ] {
            let result = apply_text_bundle(
                "fs.write",
                temp.path(),
                &TextBundle {
                    writes: vec![TextFileWrite {
                        path: path.to_owned(),
                        contents: "x".to_owned(),
                    }],
                    deletes: Vec::new(),
                    expected: Default::default(),
                },
                WorkspaceBounds::Agent,
            );
            assert!(result.is_err(), "{path} is refused");
            assert!(!temp.path().join(path).exists(), "{path} was not written");
        }
        let deleted = apply_text_bundle(
            "fs.apply_bundle",
            temp.path(),
            &TextBundle {
                writes: Vec::new(),
                deletes: vec![".git/config".to_owned()],
                expected: Default::default(),
            },
            WorkspaceBounds::Agent,
        );
        assert!(deleted.is_err(), "deleting runx or git state is refused");
        apply_text_bundle(
            "fs.write",
            temp.path(),
            &TextBundle {
                writes: vec![TextFileWrite {
                    path: ".env.example".to_owned(),
                    contents: "TOKEN=".to_owned(),
                }],
                deletes: Vec::new(),
                expected: Default::default(),
            },
            WorkspaceBounds::Agent,
        )?;
        Ok(())
    }
}
