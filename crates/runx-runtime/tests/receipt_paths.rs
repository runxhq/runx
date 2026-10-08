use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_runtime::{
    ReceiptPathInputs, ReceiptPathSource, RuntimeReceiptConfig, RunxPaths, safe_receipt_store_label,
};

#[test]
fn explicit_receipt_dir_wins_over_config_env_and_default() {
    let workspace = workspace();
    let mut env = env_with("RUNX_RECEIPT_DIR", "env-receipts");
    env.insert("RUNX_PROJECT_DIR".to_owned(), "project-state".to_owned());
    let config = RuntimeReceiptConfig {
        dir: Some(PathBuf::from("config-receipts")),
    };

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: Some(Path::new("explicit-receipts")),
        runtime_config: Some(&config),
        paths: &RunxPaths::resolve(&env, &workspace),
    });

    assert_eq!(resolved.source, ReceiptPathSource::ExplicitInput);
    assert_eq!(resolved.path, workspace.join("explicit-receipts"));
}

#[test]
fn runtime_config_wins_over_env_and_default() {
    let workspace = workspace();
    let env = env_with("RUNX_RECEIPT_DIR", "env-receipts");
    let config = RuntimeReceiptConfig {
        dir: Some(PathBuf::from("config-receipts")),
    };

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: None,
        runtime_config: Some(&config),
        paths: &RunxPaths::resolve(&env, &workspace),
    });

    assert_eq!(resolved.source, ReceiptPathSource::RuntimeConfig);
    assert_eq!(resolved.path, workspace.join("config-receipts"));
}

#[test]
fn env_receipt_dir_wins_over_project_default() {
    let workspace = workspace();
    let env = env_with("RUNX_RECEIPT_DIR", "env-receipts");

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: None,
        runtime_config: None,
        paths: &RunxPaths::resolve(&env, &workspace),
    });

    assert_eq!(resolved.source, ReceiptPathSource::Environment);
    assert_eq!(resolved.path, workspace.join("env-receipts"));
}

#[test]
fn project_default_uses_project_run_state_receipts() {
    let workspace = workspace();
    let env = BTreeMap::new();

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: None,
        runtime_config: None,
        paths: &RunxPaths::resolve(&env, &workspace),
    });

    assert_eq!(resolved.source, ReceiptPathSource::ProjectDefault);
    assert_eq!(resolved.path, workspace.join(".runx").join("receipts"));
}

#[test]
fn relative_receipt_paths_resolve_from_workspace_without_existing() {
    let workspace = workspace();
    let env = BTreeMap::new();

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: Some(Path::new("missing-parent/../receipts/new-store")),
        runtime_config: None,
        paths: &RunxPaths::resolve(&env, &workspace),
    });

    assert_eq!(resolved.path, workspace.join("receipts").join("new-store"));
}

#[test]
fn runx_project_dir_env_controls_project_default() {
    let workspace = workspace();
    let env = env_with("RUNX_PROJECT_DIR", "state/runx");

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: None,
        runtime_config: None,
        paths: &RunxPaths::resolve(&env, &workspace),
    });

    assert_eq!(resolved.source, ReceiptPathSource::ProjectDefault);
    assert_eq!(
        RunxPaths::resolve(&env, &workspace).project(),
        workspace.join("state").join("runx")
    );
    assert_eq!(
        resolved.path,
        workspace.join("state").join("runx").join("receipts")
    );
}

#[test]
fn safe_label_is_project_relative_under_project_run_state() {
    let workspace = workspace();
    let project_runx_dir = workspace.join(".runx");
    let receipt_dir = project_runx_dir.join("receipts");

    let label = safe_receipt_store_label(&receipt_dir, &project_at(&workspace, &project_runx_dir));

    assert_eq!(label.as_str(), ".runx/receipts");
}

#[test]
fn safe_label_is_project_scoped_when_project_state_is_outside_workspace() {
    let workspace = workspace();
    let project_runx_dir = PathBuf::from("/tmp/runx-project-state");
    let receipt_dir = project_runx_dir.join("receipts");

    let label = safe_receipt_store_label(&receipt_dir, &project_at(&workspace, &project_runx_dir));

    assert_eq!(label.as_str(), "runx-project:receipts");
}

#[test]
fn external_safe_label_is_stable_and_redacted() {
    let workspace = workspace();
    let project_runx_dir = workspace.join(".runx");
    let external = PathBuf::from("/tmp/operator/private/receipts");

    let first = safe_receipt_store_label(&external, &project_at(&workspace, &project_runx_dir));
    let second = safe_receipt_store_label(&external, &project_at(&workspace, &project_runx_dir));

    assert_eq!(first, second);
    assert!(first.as_str().starts_with("external-receipt-store:"));
    assert!(!first.as_str().contains("/tmp"));
    assert!(!first.as_str().contains("operator"));
    assert!(!first.as_str().contains("private"));
}

#[test]
fn public_projection_redacts_absolute_external_receipt_path_input() {
    let workspace = workspace();
    let project_runx_dir = workspace.join(".runx");
    let env = BTreeMap::new();
    let external = PathBuf::from("/Users/kam/private/runx-receipts");

    let resolved = runx_runtime::resolve_receipt_path(ReceiptPathInputs {
        explicit_dir: Some(&external),
        runtime_config: None,
        paths: &RunxPaths::resolve(&env, &workspace),
    });
    let projection = resolved.public_projection();
    let summary = projection.summary();
    let label = projection.label().as_str();
    let label_parts = label.split(':').collect::<Vec<_>>();

    assert_eq!(resolved.path, external);
    assert_eq!(label_parts.len(), 2);
    assert_eq!(label_parts[0], "external-receipt-store");
    assert_eq!(label_parts[1].len(), 16);
    assert!(
        label_parts[1]
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    assert!(summary.contains(label));
    assert_redacts_external_path(&summary);

    let direct_projection = runx_runtime::receipts::paths::safe_receipt_store_projection(
        &resolved.path,
        &project_at(&workspace, &project_runx_dir),
    );
    assert_eq!(direct_projection.summary(), summary);
}

#[test]
fn public_projection_uses_project_relative_label_for_run_state() {
    let workspace = workspace();
    let project_runx_dir = workspace.join(".runx");
    let receipt_dir = project_runx_dir.join("receipts");

    let projection = runx_runtime::receipts::paths::safe_receipt_store_projection(
        &receipt_dir,
        &project_at(&workspace, &project_runx_dir),
    );
    let summary = projection.summary();

    assert_eq!(projection.label().as_str(), ".runx/receipts");
    assert_eq!(summary, "receipt store: .runx/receipts");
    assert!(!summary.contains(workspace.to_string_lossy().as_ref()));
}

fn workspace() -> PathBuf {
    PathBuf::from("/workspace/runx")
}

fn project_at(workspace: &Path, project: &Path) -> RunxPaths {
    let mut env = env_with("RUNX_CWD", &workspace.to_string_lossy());
    env.insert(
        "RUNX_PROJECT_DIR".to_owned(),
        project.to_string_lossy().into_owned(),
    );
    RunxPaths::resolve(&env, workspace)
}

fn env_with(key: &str, value: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    env.insert(key.to_owned(), value.to_owned());
    env
}

fn assert_redacts_external_path(summary: &str) {
    assert!(!summary.contains("/Users"));
    assert!(!summary.contains("kam"));
    assert!(!summary.contains("private"));
    assert!(!summary.contains("runx-receipts"));
}
