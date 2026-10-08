mod env;
mod receipt_proof;
mod receipt_query;
mod receipts;
#[cfg(feature = "catalog")]
mod skill_packages;
mod tool_roots;
mod verified_receipts;
mod workspace_files;

pub use env::{WorkspaceEnv, WorkspaceEnvError};
pub(crate) use env::{merge_inferred_tool_roots, process_env_value};
pub(crate) use receipt_proof::prove_receipts;
pub(crate) use receipt_query::{ReceiptQueryInput, query_receipts};
pub(crate) use receipts::{ReceiptReadContext, ReceiptServices};
#[cfg(feature = "catalog")]
pub(crate) use skill_packages::{
    apply_skill_change, bind_skill_change, inspect_skill_workspace, plan_skill_architecture,
    validate_skill_package,
};
pub use verified_receipts::VerifiedReceiptStore;
pub(crate) use workspace_files::{
    ArtifactPageEncoding, ArtifactRecordPage, DEFAULT_ARTIFACT_PAGE_BYTES, LocalArtifact,
    LocalArtifactService, WorkspaceFile, resolve_scoped_root,
};
pub use workspace_files::{
    WorkspaceFileError, persist_json_artifact, read_json_artifact, read_workspace_text,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use crate::paths::{RUNX_CWD_ENV, RUNX_PROJECT_DIR_ENV};
    use crate::receipts::RUNX_RECEIPT_SIGN_KID_ENV;
    use crate::services::{ReceiptServices, WorkspaceEnv};

    #[test]
    fn skill_env_injects_workspace_and_project_paths() -> Result<(), Box<dyn std::error::Error>> {
        let workspace = WorkspaceEnv::new(BTreeMap::new(), PathBuf::from("/tmp/runx-work"))?;

        let env = workspace.skill_env_for_skill(Path::new("/tmp/runx-work/skills/demo"));

        assert_eq!(env.get(RUNX_CWD_ENV), Some(&"/tmp/runx-work".to_owned()));
        assert_eq!(
            env.get(RUNX_PROJECT_DIR_ENV),
            Some(&"/tmp/runx-work/.runx".to_owned())
        );
        Ok(())
    }

    #[test]
    fn skill_env_infers_bundled_skill_tools() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let skill_dir = temp.path().join("skills/demo");
        let tools_dir = skill_dir.join("tools");
        std::fs::create_dir_all(&tools_dir)?;
        let workspace = WorkspaceEnv::new(BTreeMap::new(), temp.path().to_path_buf())?;

        let env = workspace.skill_env_for_skill(&skill_dir);

        let value = env
            .get(crate::services::tool_roots::RUNX_TOOL_ROOTS_ENV)
            .ok_or("RUNX_TOOL_ROOTS was not inferred")?;
        let paths = std::env::split_paths(value).collect::<Vec<_>>();
        assert_eq!(paths.first(), Some(&tools_dir));
        Ok(())
    }

    #[test]
    fn a_workspace_resolves_its_receipt_root() -> Result<(), Box<dyn std::error::Error>> {
        let env = BTreeMap::from([(RUNX_PROJECT_DIR_ENV.to_owned(), ".runx-custom".to_owned())]);
        let workspace = WorkspaceEnv::new(env, PathBuf::from("/tmp/runx-work"))?;

        let resolved = workspace.receipt_path(None);

        assert_eq!(
            resolved.path,
            PathBuf::from("/tmp/runx-work/.runx-custom/receipts")
        );
        Ok(())
    }

    #[test]
    fn receipt_services_local_development_fallback_only_handles_absent_signer_env()
    -> Result<(), Box<dyn std::error::Error>> {
        let receipts = ReceiptServices::from_env_or_local_development(&BTreeMap::new())?;

        assert!(
            receipts
                .signature_config()
                .production_key_for_kid("any-production-key")
                .is_none()
        );

        let partial_env = BTreeMap::from([(
            RUNX_RECEIPT_SIGN_KID_ENV.to_owned(),
            "partial-explicit-key".to_owned(),
        )]);
        let Err(error) = ReceiptServices::from_env_or_local_development(&partial_env) else {
            return Err("partial signer env must still fail closed".into());
        };
        assert!(error.to_string().contains("set together"));
        Ok(())
    }
}
