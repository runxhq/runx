use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::{
    JsonNumber, JsonObject, JsonValue, SkillPackageDelta, SkillPackageMetrics, sha256_prefixed,
};

use super::path::{
    display_relative, ignored_package_entry, invalid_skill_change, is_executable_path,
};
use super::{MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES};
use crate::RuntimeError;
use crate::filesystem::read_dir_sorted;

pub(super) struct PackageFile {
    pub(super) relative: PathBuf,
    pub(super) contents: Vec<u8>,
    pub(super) permissions: fs::Permissions,
    permission_fingerprint: u32,
}

pub(super) struct PackageSnapshot {
    pub(super) digest: String,
    pub(super) files: Vec<PackageFile>,
}

pub(super) fn package_metrics(snapshot: &PackageSnapshot) -> SkillPackageMetrics {
    let mut metrics = SkillPackageMetrics::default();
    for file in &snapshot.files {
        metrics.files = metrics.files.saturating_add(1);
        metrics.bytes = metrics.bytes.saturating_add(file.contents.len() as u64);
        let lines = text_line_count(&file.contents) as u64;
        let relative = display_relative(&file.relative);
        match line_class(&relative) {
            LineClass::Production => {
                metrics.production_lines = metrics.production_lines.saturating_add(lines);
            }
            LineClass::Test => {
                metrics.test_lines = metrics.test_lines.saturating_add(lines);
            }
            LineClass::Generated => {
                metrics.generated_lines = metrics.generated_lines.saturating_add(lines);
            }
        }
        if is_executable_path(&relative) {
            metrics.executable_files += 1;
            metrics.executable_lines = metrics.executable_lines.saturating_add(lines);
        }
    }
    metrics
}

pub(super) fn package_metrics_json(metrics: &SkillPackageMetrics) -> JsonValue {
    JsonValue::Object(JsonObject::from([
        (
            "files".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.files)),
        ),
        (
            "bytes".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.bytes)),
        ),
        (
            "production_lines".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.production_lines)),
        ),
        (
            "test_lines".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.test_lines)),
        ),
        (
            "generated_lines".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.generated_lines)),
        ),
        (
            "executable_files".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.executable_files)),
        ),
        (
            "executable_lines".to_owned(),
            JsonValue::Number(JsonNumber::U64(metrics.executable_lines)),
        ),
    ]))
}

pub(super) fn package_delta(
    before: &SkillPackageMetrics,
    after: &SkillPackageMetrics,
) -> SkillPackageDelta {
    SkillPackageDelta {
        files: delta(after.files, before.files),
        bytes: delta(after.bytes, before.bytes),
        production_lines: delta(after.production_lines, before.production_lines),
        test_lines: delta(after.test_lines, before.test_lines),
        generated_lines: delta(after.generated_lines, before.generated_lines),
        executable_files: delta(after.executable_files, before.executable_files),
        executable_lines: delta(after.executable_lines, before.executable_lines),
    }
}

fn delta(after: u64, before: u64) -> i64 {
    i64::try_from(after)
        .and_then(|after| i64::try_from(before).map(|before| after - before))
        .unwrap_or(if after >= before { i64::MAX } else { i64::MIN })
}

enum LineClass {
    Production,
    Test,
    Generated,
}

fn line_class(relative: &str) -> LineClass {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    if relative.starts_with("dist/")
        || relative.starts_with("generated/")
        || name.contains(".generated.")
    {
        return LineClass::Generated;
    }
    if relative.starts_with("fixtures/")
        || relative.starts_with("tests/")
        || relative.starts_with("test/")
        || name.contains(".test.")
        || name.ends_with("_test.rs")
    {
        return LineClass::Test;
    }
    LineClass::Production
}

fn text_line_count(contents: &[u8]) -> usize {
    if contents.is_empty() || std::str::from_utf8(contents).is_err() {
        return 0;
    }
    contents
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        .saturating_add(usize::from(contents.last() != Some(&b'\n')))
}

pub(super) fn package_snapshot(root: &Path) -> Result<PackageSnapshot, RuntimeError> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PackageSnapshot {
                digest: absent_package_digest(),
                files: Vec::new(),
            });
        }
        Err(source) => {
            return Err(RuntimeError::io(
                format!("reading skill package target {}", root.display()),
                source,
            ));
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(invalid_skill_change(format!(
                "skill package target cannot be a symlink: {}",
                root.display()
            )));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(invalid_skill_change(format!(
                "skill package target is not a directory: {}",
                root.display()
            )));
        }
        Ok(_) => {}
    }
    let mut file_count = 0usize;
    let mut byte_count = 0usize;
    let mut files = Vec::new();
    collect_package_files(root, root, &mut files, &mut file_count, &mut byte_count)?;
    let mut canonical = Vec::with_capacity(byte_count.saturating_add(file_count * 32));
    canonical.extend_from_slice(b"directory\0");
    for file in &files {
        let path = display_relative(&file.relative);
        append_digest_field(&mut canonical, path.as_bytes());
        append_digest_field(&mut canonical, &file.permission_fingerprint.to_be_bytes());
        append_digest_field(&mut canonical, &file.contents);
    }
    Ok(PackageSnapshot {
        digest: sha256_prefixed(&canonical),
        files,
    })
}

pub(super) fn absent_package_digest() -> String {
    sha256_prefixed(b"absent")
}

fn collect_package_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<PackageFile>,
    file_count: &mut usize,
    byte_count: &mut usize,
) -> Result<(), RuntimeError> {
    for entry in read_dir_sorted(current)? {
        if ignored_package_entry(&entry.name) {
            continue;
        }
        let metadata = fs::symlink_metadata(&entry.path).map_err(|source| {
            RuntimeError::io(format!("reading {}", entry.path.display()), source)
        })?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_skill_change(format!(
                "skill package contains unsupported symlink: {}",
                entry.path.display()
            )));
        }
        if entry.is_dir {
            collect_package_files(root, &entry.path, files, file_count, byte_count)?;
        } else if entry.is_file {
            let over_limit = || {
                invalid_skill_change(format!(
                    "skill package exceeds validation limit ({MAX_PACKAGE_FILES} files, {MAX_PACKAGE_BYTES} bytes)"
                ))
            };
            *file_count += 1;
            if *file_count > MAX_PACKAGE_FILES {
                return Err(over_limit());
            }
            let contents = crate::skill_package::read_within(
                &entry.path,
                MAX_PACKAGE_BYTES.saturating_sub(*byte_count),
            )
            .map_err(|source| {
                RuntimeError::io(
                    format!("reading package file {}", entry.path.display()),
                    source,
                )
            })?
            .ok_or_else(over_limit)?;
            *byte_count += contents.len();
            files.push(PackageFile {
                relative: entry
                    .path
                    .strip_prefix(root)
                    .map(Path::to_path_buf)
                    .map_err(|_| invalid_skill_change("package file escaped target directory"))?,
                contents,
                permissions: metadata.permissions(),
                permission_fingerprint: permission_fingerprint(&metadata),
            });
        } else {
            return Err(invalid_skill_change(format!(
                "skill package contains unsupported file type: {}",
                entry.path.display()
            )));
        }
    }
    Ok(())
}

fn append_digest_field(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

#[cfg(unix)]
fn permission_fingerprint(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode()
}

#[cfg(not(unix))]
fn permission_fingerprint(metadata: &fs::Metadata) -> u32 {
    u32::from(metadata.permissions().readonly())
}
