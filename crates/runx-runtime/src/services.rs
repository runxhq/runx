mod env;
mod receipt_proof;
mod receipt_query;
mod receipts;
mod skill_packages;
mod tool_roots;
mod verified_receipts;
mod workspace_files;

pub use env::{WorkspaceEnv, WorkspaceEnvError};
pub(crate) use env::{env_file_differs, merge_inferred_tool_roots};
pub(crate) use receipt_proof::prove_receipts;
pub(crate) use receipt_query::{ReceiptQueryInput, query_receipts};
pub(crate) use receipts::{ReceiptReadContext, ReceiptServices};
pub(crate) use skill_packages::{
    apply_skill_change, bind_skill_change, inspect_skill_workspace, plan_skill_architecture,
    skill_change_effect, validate_skill_package,
};
pub use verified_receipts::VerifiedReceiptStore;
pub(crate) use workspace_files::{
    ArtifactPageEncoding, ArtifactRecordPage, DEFAULT_ARTIFACT_PAGE_BYTES, LocalArtifact,
    LocalArtifactService, WorkspaceBounds, WorkspaceFile, is_secret_path,
    persist_canonical_json_artifact, resolve_scoped_root,
};
pub use workspace_files::{
    WorkspaceFileError, persist_json_artifact, read_json_artifact, read_workspace_text,
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use crate::paths::{RUNX_CWD_ENV, RUNX_HOME_ENV, RUNX_PROJECT_DIR_ENV};
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

    #[cfg(unix)]
    #[test]
    fn receipt_services_seal_under_the_workspace_home_key() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let env = BTreeMap::from([(
            RUNX_HOME_ENV.to_owned(),
            temp.path().join("home").display().to_string(),
        )]);
        let workspace = WorkspaceEnv::new(env.clone(), temp.path().to_path_buf())?;

        let receipts = ReceiptServices::resolve(&workspace)?;

        let issuer = receipts.signature_config().issuer();
        assert_eq!(issuer.issuer_type, runx_contracts::ReceiptIssuerType::Local);
        assert!(workspace.paths().local_receipt_key().is_file());
        assert_eq!(
            ReceiptServices::resolve(&workspace)?
                .signature_config()
                .issuer(),
            issuer
        );

        let mut partial_env = env;
        partial_env.insert(
            RUNX_RECEIPT_SIGN_KID_ENV.to_owned(),
            "partial-explicit-key".to_owned(),
        );
        let partial = WorkspaceEnv::new(partial_env, temp.path().to_path_buf())?;
        let Err(error) = ReceiptServices::resolve(&partial) else {
            return Err("partial signer env must fail closed".into());
        };
        assert!(error.to_string().contains("set together"));
        Ok(())
    }
}
