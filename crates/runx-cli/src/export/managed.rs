use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::{
    CODEX_RULE_END, CODEX_RULE_RUNX_ON_PATH, CODEX_RULE_RUNX_RESUME_ON_PATH, CODEX_RULE_START,
    ExportError, GeneratedFile, Target, display_path,
};

pub(super) fn validate_write_targets(
    target: Target,
    files: &[GeneratedFile],
    export_root: &Path,
    full_export: bool,
) -> Result<(), ExportError> {
    for file in files {
        if !file.path.exists() {
            continue;
        }
        let existing = read_to_string(&file.path)?;
        let existing_source = managed_source(&existing, target);
        let generated_source = managed_source(&file.contents, target);
        let owned_by_this_export = existing_source == generated_source
            || (full_export
                && existing_source
                    .is_some_and(|source| source_belongs_to_export_root(source, export_root)));
        if !owned_by_this_export {
            return Err(ExportError::InvalidArgs(format!(
                "refusing to overwrite {} because it is not managed by this export",
                display_path(&file.path)
            )));
        }
    }
    Ok(())
}

pub(super) fn write_files(files: &[GeneratedFile]) -> Result<(), ExportError> {
    for file in files {
        let parent = file.path.parent().ok_or_else(|| ExportError::Io {
            context: format!("resolving parent for {}", display_path(&file.path)),
            source: std::io::Error::other("path has no parent"),
        })?;
        fs::create_dir_all(parent).map_err(|source| ExportError::Io {
            context: format!("creating {}", display_path(parent)),
            source,
        })?;
        fs::write(&file.path, &file.contents).map_err(|source| ExportError::Io {
            context: format!("writing {}", display_path(&file.path)),
            source,
        })?;
    }
    Ok(())
}

pub(super) fn managed_drift_warnings(
    target: Target,
    files: &[GeneratedFile],
) -> Result<Vec<String>, ExportError> {
    let mut warnings = Vec::new();
    for file in files {
        if !file.path.exists() {
            continue;
        }
        let existing = read_to_string(&file.path)?;
        let Some(existing_digest) = managed_package_digest(&existing, target) else {
            continue;
        };
        let Some(generated_digest) = managed_package_digest(&file.contents, target) else {
            continue;
        };
        if existing_digest != generated_digest {
            warnings.push(format!(
                "replaced stale managed export {} ({} -> {})",
                display_path(&file.path),
                existing_digest,
                generated_digest
            ));
        }
    }
    Ok(warnings)
}

pub(super) fn prune_managed_files(
    target: Target,
    skill_dir: &Path,
    files: &[GeneratedFile],
    export_root: &Path,
) -> Result<Vec<String>, ExportError> {
    let wanted = files
        .iter()
        .map(|file| file.path.clone())
        .collect::<BTreeSet<_>>();
    let entries = match fs::read_dir(skill_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(ExportError::Io {
                context: format!("reading {}", display_path(skill_dir)),
                source,
            });
        }
    };
    let mut pruned = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| ExportError::Io {
            context: format!("reading {}", display_path(skill_dir)),
            source,
        })?;
        let skill_file = entry.path().join("SKILL.md");
        if wanted.contains(&skill_file) || !skill_file.exists() {
            continue;
        }
        let contents = read_to_string(&skill_file)?;
        if !contents.contains(target.marker()) {
            continue;
        }
        let Some(source) = managed_source(&contents, target) else {
            continue;
        };
        if !source_belongs_to_export_root(source, export_root) {
            continue;
        }
        fs::remove_file(&skill_file).map_err(|source| ExportError::Io {
            context: format!("removing {}", display_path(&skill_file)),
            source,
        })?;
        let _ignored = fs::remove_dir(entry.path());
        pruned.push(display_path(&skill_file));
    }
    pruned.sort();
    Ok(pruned)
}

fn managed_source(contents: &str, target: Target) -> Option<&str> {
    let prefix = format!("<!-- {} source=", target.marker());
    let suffix = " - generated, do not edit -->";
    contents.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(suffix))
            .map(|value| {
                value
                    .split_once(" package-digest=")
                    .map_or(value, |(source, _)| source)
            })
    })
}

fn managed_package_digest(contents: &str, target: Target) -> Option<&str> {
    let prefix = format!("<!-- {} source=", target.marker());
    contents.lines().find_map(|line| {
        line.strip_prefix(&prefix)?
            .split_once(" package-digest=")?
            .1
            .strip_suffix(" - generated, do not edit -->")
    })
}

/// A project export names its source relative to the export root, so it
/// reads the same on every machine; a user export names it absolutely.
fn source_belongs_to_export_root(source: &str, export_root: &Path) -> bool {
    let source = export_root.join(source);
    (export_root.join("SKILL.md").is_file() && source == export_root)
        || (export_root.join("skills").is_dir() && source.starts_with(export_root.join("skills")))
}

pub(super) fn merge_codex_rules(path: &Path, runx_bin: &Path) -> Result<PathBuf, ExportError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| ExportError::Io {
            context: format!("creating {}", display_path(parent)),
            source,
        })?;
    }
    let existing = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ExportError::Io {
                context: format!("reading {}", display_path(path)),
                source,
            });
        }
    };
    let block = format!(
        "{CODEX_RULE_START}\n{CODEX_RULE_RUNX_ON_PATH}\n{CODEX_RULE_RUNX_RESUME_ON_PATH}\n{}\n{CODEX_RULE_END}\n",
        codex_rule_for_binary(runx_bin)
    );
    let contents = replace_or_append_block(&existing, &block);
    fs::write(path, contents).map_err(|source| ExportError::Io {
        context: format!("writing {}", display_path(path)),
        source,
    })?;
    Ok(path.to_path_buf())
}

fn codex_rule_for_binary(runx_bin: &Path) -> String {
    let path =
        serde_json::to_string(&display_path(runx_bin)).unwrap_or_else(|_| "\"runx\"".to_owned());
    format!(
        "prefix_rule(pattern = [{path}, \"skill\"], decision = \"allow\", justification = \"runx skill invocations are trusted\")\nprefix_rule(pattern = [{path}, \"resume\"], decision = \"allow\", justification = \"runx resume invocations are trusted\")"
    )
}

fn replace_or_append_block(existing: &str, block: &str) -> String {
    if let Some(start) = existing.find(CODEX_RULE_START)
        && let Some(relative_end) = existing[start..].find(CODEX_RULE_END)
    {
        let end = start + relative_end + CODEX_RULE_END.len();
        let mut output = String::new();
        output.push_str(&existing[..start]);
        output.push_str(block);
        let suffix = existing[end..].trim_start_matches(['\r', '\n']);
        output.push_str(suffix);
        return ensure_trailing_newline(output);
    }
    let mut output = ensure_trailing_newline(existing.to_owned());
    if !output.is_empty() && !output.ends_with("\n\n") {
        output.push('\n');
    }
    output.push_str(block);
    output
}

fn read_to_string(path: &Path) -> Result<String, ExportError> {
    fs::read_to_string(path).map_err(|source| ExportError::Io {
        context: format!("reading {}", display_path(path)),
        source,
    })
}

fn ensure_trailing_newline(mut value: String) -> String {
    if !value.is_empty() && !value.ends_with('\n') {
        value.push('\n');
    }
    value
}
