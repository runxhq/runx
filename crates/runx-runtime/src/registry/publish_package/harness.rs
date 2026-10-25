use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::RegistryPublishPackageError;
use crate::registry::{RegistryPackageFile, RegistryPublishHarnessReport};
use crate::{LoadedSkillPackage, LocalOrchestrator, PackageHarnessRequest};

pub(super) struct PublishHarnessPackage {
    root: TempDirectory,
}

impl PublishHarnessPackage {
    pub(super) fn path(&self) -> &Path {
        self.root.path()
    }
}

pub(super) fn stage_publish_harness(
    loaded: &LoadedSkillPackage,
    profile_document: Option<&str>,
    package_files: &[RegistryPackageFile],
) -> Result<Option<PublishHarnessPackage>, RegistryPublishPackageError> {
    let Some(profile_document) = profile_document else {
        return Ok(None);
    };
    let root = TempDirectory::create("runx-publish-profile-harness")?;
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
    Ok(Some(PublishHarnessPackage { root }))
}

pub(super) fn run_publish_harness(
    orchestrator: &LocalOrchestrator,
    harness_path: &Path,
    workspace_env: &BTreeMap<String, String>,
) -> Result<RegistryPublishHarnessReport, RegistryPublishPackageError> {
    let receipt_root = TempDirectory::create("runx-publish-harness")?;
    let orchestrator = orchestrator.with_environment(publish_harness_env(
        workspace_env,
        &receipt_root.path().join("home"),
        harness_path,
    ));
    let request = PackageHarnessRequest {
        skill_path: harness_path.to_path_buf(),
        receipt_dir: Some(receipt_root.path().to_path_buf()),
    };
    let report = orchestrator
        .run_package_harness(&request)
        .map_err(|error| {
            RegistryPublishPackageError::invalid(format!(
                "package harness failed for {}: {error}",
                harness_path.display()
            ))
        })?;
    let report = RegistryPublishHarnessReport {
        status: report.status.to_owned(),
        case_count: report.case_count,
        assertion_error_count: report.assertion_error_count,
        assertion_errors: report.assertion_errors,
        case_names: report.case_names,
        receipt_ids: report.receipt_ids,
        graph_case_count: report.graph_case_count,
    };
    if report.case_count == 0 {
        return Err(RegistryPublishPackageError::invalid(format!(
            "harness failed for {}: no cases were discovered",
            harness_path.display()
        )));
    }
    if report.failed() {
        return Err(RegistryPublishPackageError::invalid(format!(
            "harness failed for {}: {}",
            harness_path.display(),
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
                "failed to create publish harness directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    fs::write(&path, content).map_err(|error| {
        RegistryPublishPackageError::invalid(format!(
            "failed to write publish harness file {}: {error}",
            path.display()
        ))
    })
}

fn publish_harness_env(
    workspace_env: &BTreeMap<String, String>,
    runx_home: &Path,
    harness_path: &Path,
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
        harness_path.to_string_lossy().into_owned(),
    );
    env
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn create(prefix: &str) -> Result<Self, RegistryPublishPackageError> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| RegistryPublishPackageError::invalid(error.to_string()))?
            .as_nanos();
        let path = env::temp_dir().join(format!("{prefix}-{}-{nanos}", process::id()));
        fs::create_dir_all(&path).map_err(|error| {
            RegistryPublishPackageError::invalid(format!(
                "failed to create temporary directory {}: {error}",
                path.display()
            ))
        })?;
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

    #[test]
    fn harness_environment_removes_hosted_and_agent_credentials() {
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
        let env = publish_harness_env(
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
        // The harness seals under its own home's key: no identity is
        // invented for it.
        assert!(!env.contains_key(crate::RUNX_RECEIPT_SIGN_KID_ENV));
    }
}
