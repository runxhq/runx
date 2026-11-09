use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use super::RegistryPublishPackageError;
use crate::registry::{RegistryPackageFile, RegistryPublishTestReport};
use crate::{LoadedSkillPackage, LocalOrchestrator, PackageTestRequest};

pub(super) struct PublishTestPackage {
    root: TempDirectory,
}

impl PublishTestPackage {
    pub(super) fn path(&self) -> &Path {
        self.root.path()
    }
}

pub(super) fn stage_publish_test(
    loaded: &LoadedSkillPackage,
    profile_document: Option<&str>,
    package_files: &[RegistryPackageFile],
) -> Result<Option<PublishTestPackage>, RegistryPublishPackageError> {
    let Some(profile_document) = profile_document else {
        return Ok(None);
    };
    let root = TempDirectory::create("runx-publish-profile-test")?;
    write_text(
        root.path().join("SKILL.md"),
        &loaded.package.manual_markdown,
    )?;
    write_text(
        root.path().join(runx_parser::PROFILE_FILE),
        profile_document,
    )?;
    for file in package_files {
        write_text(root.path().join(&file.path), &file.content)?;
    }
    Ok(Some(PublishTestPackage { root }))
}

pub(super) fn run_publish_test(
    orchestrator: &LocalOrchestrator,
    test_path: &Path,
    workspace_env: &BTreeMap<String, String>,
) -> Result<RegistryPublishTestReport, RegistryPublishPackageError> {
    let receipt_root = TempDirectory::create("runx-publish-test")?;
    let orchestrator = orchestrator.with_environment(publish_test_env(
        workspace_env,
        &receipt_root.path().join("home"),
        test_path,
    ));
    let request = PackageTestRequest {
        skill_path: test_path.to_path_buf(),
        receipt_dir: Some(receipt_root.path().to_path_buf()),
    };
    let report = orchestrator.run_package_test(&request).map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "package test failed for {}: {error}",
            test_path.display()
        ))
    })?;
    let report = RegistryPublishTestReport {
        status: report.status.to_owned(),
        case_count: report.case_count,
        assertion_error_count: report.assertion_error_count,
        assertion_errors: report.assertion_errors,
        case_names: report.case_names,
        receipt_ids: report.receipt_ids,
        workflow_case_count: report.workflow_case_count,
    };
    if report.case_count == 0 {
        return Err(RegistryPublishPackageError::invalid(format!(
            "test failed for {}: no cases were discovered",
            test_path.display()
        )));
    }
    if report.failed() {
        return Err(RegistryPublishPackageError::invalid(format!(
            "test failed for {}: {}",
            test_path.display(),
            report.assertion_errors.join("; ")
        )));
    }
    Ok(report)
}

fn write_text(path: PathBuf, content: &str) -> Result<(), RegistryPublishPackageError> {
    write_bytes(path.clone(), content.as_bytes())?;
    crate::registry::package_files::restore_script_permissions(&path, content)
        .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))
}

fn write_bytes(path: PathBuf, content: &[u8]) -> Result<(), RegistryPublishPackageError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            RegistryPublishPackageError::invalid(format!(
                "failed to create publish test directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(&path, content).map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "failed to write publish test file {}: {error}",
            path.display()
        ))
    })
}

fn publish_test_env(
    workspace_env: &BTreeMap<String, String>,
    runx_home: &Path,
    test_path: &Path,
) -> BTreeMap<String, String> {
    let mut env = workspace_env.clone();
    env.retain(|key, _| !key.starts_with("RUNX_HOSTED_"));
    env.remove("RUNX_AGENT_PROVIDER");
    env.remove("RUNX_AGENT_MODEL");
    env.remove("RUNX_AGENT_API_KEY");
    env.remove("RUNX_AGENT_ENDPOINT_URL");
    env.remove("RUNX_AGENT_AUTH_MODE");
    env.insert(
        "RUNX_HOME".to_owned(),
        runx_home.to_string_lossy().into_owned(),
    );
    env.insert(
        crate::RUNX_CWD_ENV.to_owned(),
        test_path.to_string_lossy().into_owned(),
    );
    env
}

/// A directory this process alone owns, removed when it drops.
struct TempDirectory(PathBuf);

/// Each temporary directory this process names takes the next number, so two
/// made at the same instant on two threads never share a name.
static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

impl TempDirectory {
    fn create(prefix: &str) -> Result<Self, RegistryPublishPackageError> {
        loop {
            let number = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = env::temp_dir().join(format!("{prefix}-{}-{number}", process::id()));
            match Self::claim(path.clone()) {
                Ok(directory) => return Ok(directory),
                // Left by an earlier process with this id: take the next name.
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(RegistryPublishPackageError::invalid(format!(
                        "failed to create temporary directory {}: {error}",
                        path.display()
                    )));
                }
            }
        }
    }

    /// Take `path` as this directory's own, refusing one that already
    /// exists: a directory another owner holds is never shared, so its
    /// cleanup can't remove what this one holds.
    fn claim(path: PathBuf) -> std::io::Result<Self> {
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ignored = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A staging directory is its own: another claim of its name is refused,
    /// so no other staging's cleanup can remove what it holds. Two stagings
    /// on threads of one process used to take the same name when the clock
    /// gave them the same instant, and the first one dropped removed the
    /// other's files under it.
    #[test]
    fn a_staging_directory_is_never_shared() -> Result<(), Box<dyn std::error::Error>> {
        let first = TempDirectory::create("runx-publish-profile-test")?;
        let same_name = TempDirectory::claim(first.path().to_path_buf());
        assert!(same_name.is_err(), "a second claim of one name is refused");
        drop(same_name);
        fs::write(first.path().join("SKILL.md"), "# staged")?;

        let second = TempDirectory::create("runx-publish-profile-test")?;
        assert_ne!(first.path(), second.path());
        Ok(())
    }

    #[test]
    fn test_environment_removes_hosted_and_agent_credentials() {
        let workspace_env = BTreeMap::from([
            ("RUNX_HOSTED_API_KEY".to_owned(), "hosted-secret".to_owned()),
            ("RUNX_AGENT_API_KEY".to_owned(), "agent-secret".to_owned()),
            (
                "RUNX_AGENT_ENDPOINT_URL".to_owned(),
                "http://127.0.0.1:18081/v1/chat/completions".to_owned(),
            ),
            ("RUNX_AGENT_AUTH_MODE".to_owned(), "local_none".to_owned()),
            ("HTTP_PROXY".to_owned(), "http://proxy.test".to_owned()),
        ]);
        let env = publish_test_env(
            &workspace_env,
            Path::new("/tmp/runx-publish-home"),
            Path::new("/tmp/runx-publish-workspace"),
        );
        assert!(env.keys().all(|key| !key.starts_with("RUNX_HOSTED_")));
        assert!(!env.contains_key("RUNX_AGENT_API_KEY"));
        assert!(!env.contains_key("RUNX_AGENT_ENDPOINT_URL"));
        assert!(!env.contains_key("RUNX_AGENT_AUTH_MODE"));
        assert_eq!(
            env.get("HTTP_PROXY").map(String::as_str),
            Some("http://proxy.test")
        );
        assert_eq!(
            env.get(crate::RUNX_CWD_ENV).map(String::as_str),
            Some("/tmp/runx-publish-workspace")
        );
        // The test seals under its own home's key: no identity is
        // invented for it.
        assert!(!env.contains_key(crate::RUNX_RECEIPT_SIGN_KID_ENV));
    }
}
