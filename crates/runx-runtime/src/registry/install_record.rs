//! What the project records beside the packages it holds: each package a
//! registry added, with its registry identity and trust tier, so the copy on
//! disk is never taken for the project's own (agent-sessions §4.1).

use std::fs;
use std::path::{Path, PathBuf};

use runx_contracts::TrustTier;
use serde::{Deserialize, Serialize};

use crate::RunxPaths;

/// A package a registry added to the project.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRecord {
    /// `<publisher>/<name>@<version>` of a skill, `<name>@<version>` of a
    /// connector.
    pub package: String,
    pub trust_tier: TrustTier,
}

/// Record that the package at `package_dir`, under the project's
/// `packages_root` (its added skills or added connectors), came from a
/// registry. The record is written once per version, as the package is.
pub(crate) fn write_install_record(
    records: &Path,
    packages_root: &Path,
    package_dir: &Path,
    record: &InstallRecord,
) -> Result<(), String> {
    let Some(path) = record_path(records, packages_root, package_dir) else {
        return Err(format!(
            "{} is not under {}",
            package_dir.display(),
            packages_root.display()
        ));
    };
    write_json(&path, record)
}

/// The record of the package at `package_dir`, if a registry added it to
/// the project.
///
/// # Errors
///
/// Returns an error when a record exists and cannot be read.
pub fn read_install_record(
    paths: &RunxPaths,
    package_dir: &Path,
) -> Result<Option<InstallRecord>, String> {
    // The roots and the package as the filesystem names them, so a link in
    // either path does not hide the record.
    let resolved = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let records = paths.install_records();
    let package_dir = resolved(package_dir);
    for root in [paths.added_skills(), paths.added_connectors()] {
        if let Some(path) = record_path(&records, &resolved(&root), &package_dir) {
            return read_json(&path);
        }
    }
    Ok(None)
}

/// Where an installed package came from: the registry identity and tier the
/// project recorded when a registry added it, and the digests of the files
/// the install verified against the registry's signed envelope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InstalledOrigin {
    pub(crate) skill_id: String,
    pub(crate) version: String,
    pub(crate) trust_tier: TrustTier,
    /// The manual's digest.
    pub(crate) digest: String,
    /// The registry's digest of the package files beside the manual and the
    /// profile, when it has any.
    pub(crate) bundle_digest: Option<String>,
}

/// The registry origin of the installed skill `package`, rebuilt from its
/// install record; none when no registry added it.
///
/// # Errors
///
/// Returns an error when a record exists and cannot be read, or names no
/// `<skill>@<version>`.
pub(crate) fn installed_origin(
    paths: &RunxPaths,
    package: &crate::LoadedSkillPackage,
) -> Result<Option<InstalledOrigin>, String> {
    let Some(record) = read_install_record(paths, &package.package_root)? else {
        return Ok(None);
    };
    let (skill_id, version) = record.package.rsplit_once('@').ok_or_else(|| {
        format!(
            "the install record of {} names no version: {}",
            package.package_root.display(),
            record.package
        )
    })?;
    let files = package
        .package
        .source
        .files
        .iter()
        .filter(|(path, _)| {
            path.as_str() != super::package_files::MANUAL_FILE
                && path.as_str() != runx_parser::PROFILE_FILE
        })
        .map(|(path, content)| super::RegistryPackageFile {
            path: path.clone(),
            content: String::from_utf8_lossy(content).into_owned(),
        })
        .collect::<Vec<_>>();
    Ok(Some(InstalledOrigin {
        skill_id: skill_id.to_owned(),
        version: version.to_owned(),
        trust_tier: record.trust_tier,
        digest: package.package.manual_digest.clone(),
        bundle_digest: super::package_files::registry_bundle_digest(&files),
    }))
}

/// `<records>/<skills|connectors>/<path under the root>.json`.
fn record_path(records: &Path, packages_root: &Path, package_dir: &Path) -> Option<PathBuf> {
    let kind = packages_root.file_name()?;
    let relative = package_dir.strip_prefix(packages_root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    let mut path = records.join(kind).join(relative);
    path.as_mut_os_string().push(".json");
    Some(path)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no directory", path.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let encoded = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    let staged = parent.join(format!(
        ".{}.{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        super::install::unique_suffix()
    ));
    fs::write(&staged, encoded).map_err(|error| format!("{}: {error}", staged.display()))?;
    fs::rename(&staged, path).map_err(|error| {
        let _ignored = fs::remove_file(&staged);
        format!("{}: {error}", path.display())
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_keyed_by_the_packages_path_under_its_root() {
        let records = Path::new("/w/.runx/installs");
        assert_eq!(
            record_path(
                records,
                Path::new("/w/skills"),
                Path::new("/w/skills/acme/echo/1.0.0")
            ),
            Some(PathBuf::from(
                "/w/.runx/installs/skills/acme/echo/1.0.0.json"
            ))
        );
        assert_eq!(
            record_path(records, Path::new("/w/skills"), Path::new("/w/skills")),
            None
        );
        assert_eq!(
            record_path(
                records,
                Path::new("/w/skills"),
                Path::new("/elsewhere/echo")
            ),
            None
        );
    }
}
