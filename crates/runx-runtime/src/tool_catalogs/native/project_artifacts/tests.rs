use super::super::{NativeToolInvocation, invoke};
use crate::credentials::CredentialDelivery;
use crate::effects::RuntimeEffectRegistry;
use crate::services::{ArtifactPageEncoding, LocalArtifactService};
use crate::tool_catalogs::ToolCall;
use runx_contracts::Reach;
use runx_contracts::{Artifact, ArtifactLabels, JsonObject, JsonValue};
use std::collections::BTreeMap;

fn call(
    tool: &str,
    inputs: serde_json::Value,
    env: &BTreeMap<String, String>,
    artifacts: &LocalArtifactService,
    reach: Reach,
    identity: Option<&ToolCall>,
) -> Result<JsonValue, Box<dyn std::error::Error>> {
    let root = std::path::Path::new(env.get("RUNX_CWD").ok_or("missing workspace")?);
    let result = invoke(NativeToolInvocation {
        tool,
        observed_at: "2026-11-03T10:00:00Z",
        inputs: serde_json::from_value::<JsonObject>(inputs)?,
        env,
        skill_directory: root,
        credential_delivery: &CredentialDelivery::none(),
        local_artifacts: artifacts,
        effects: &RuntimeEffectRegistry::default(),
        reach,
        call: identity,
        effect_admission: None,
    })
    .ok_or("capability is not registered")?;
    Ok(result.result?)
}
fn environment(root: &std::path::Path, project: &std::path::Path) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("RUNX_CWD".to_owned(), root.to_string_lossy().into_owned()),
        (
            "RUNX_PROJECT_DIR".to_owned(),
            project.to_string_lossy().into_owned(),
        ),
        (
            "RUNX_HOME".to_owned(),
            root.join("home").to_string_lossy().into_owned(),
        ),
    ])
}
fn store(
    root: &std::path::Path,
    project: &std::path::Path,
    value: serde_json::Value,
) -> Result<Artifact, Box<dyn std::error::Error>> {
    store_inputs(root, project, serde_json::json!({"value":value}))
}
fn store_inputs(
    root: &std::path::Path,
    project: &std::path::Path,
    inputs: serde_json::Value,
) -> Result<Artifact, Box<dyn std::error::Error>> {
    let env = environment(root, project);
    call(
        "artifact.store",
        inputs,
        &env,
        &LocalArtifactService::default(),
        Reach::Inputs,
        Some(&ToolCall::new("artifact-run", "store", 1, None)),
    )?
    .deserialize_into()
    .map_err(Into::into)
}
fn source_path(artifact: &Artifact) -> Result<&std::path::Path, Box<dyn std::error::Error>> {
    artifact
        .path
        .as_deref()
        .map(std::path::Path::new)
        .ok_or_else(|| "the artifact names no path".into())
}

#[test]
fn project_artifact_store_reopens_in_a_fresh_snapshot_service_and_pages_exact_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let custom = tempfile::tempdir()?;
    let value = serde_json::json!([{"id":1,"name":"東京"},{"id":2,"name":"🙂"}]);
    let artifact = store(workspace.path(), custom.path(), value.clone())?;
    assert!(
        source_path(&artifact)?.starts_with(
            custom
                .path()
                .join("artifacts/skill-runs/artifact-run/values")
        )
    );
    assert_eq!(
        artifact,
        store(workspace.path(), custom.path(), value.clone())?
    );
    let env = environment(workspace.path(), custom.path());
    let snapshots = LocalArtifactService::default();
    let opened: Artifact = call(
        "artifact.open",
        serde_json::json!({"artifact":artifact}),
        &env,
        &snapshots,
        Reach::Inputs,
        None,
    )?
    .deserialize_into()?;
    assert_eq!(opened.digest, artifact.digest);
    let first = snapshots.read_json_array_page(&opened.reference, 0, 40)?;
    let second = snapshots.read_json_array_page(&opened.reference, first.next_offset, 40)?;
    let records = first
        .records
        .into_iter()
        .chain(second.records)
        .map(|s| serde_json::from_str::<serde_json::Value>(&s))
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(records, value.as_array().ok_or("array")?.clone());
    assert!(second.eof);
    assert_eq!(
        crate::services::read_json_artifact(&custom.path().join("artifacts"), &artifact)?,
        serde_json::from_value::<JsonValue>(value)?
    );
    std::fs::write(
        source_path(&artifact)?,
        "changed after the immutable snapshot",
    )?;
    assert!(
        snapshots
            .read_page(&opened.reference, 0, 1024, ArtifactPageEncoding::Utf8)?
            .eof
    );
    assert!(
        call(
            "artifact.open",
            serde_json::json!({"artifact":artifact}),
            &env,
            &LocalArtifactService::default(),
            Reach::Inputs,
            None,
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn project_artifact_ports_require_identity_and_reach_inputs_before_io()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let project = workspace.path().join("not-created");
    let env = environment(workspace.path(), &project);
    let snapshots = LocalArtifactService::default();
    let identity = ToolCall::new("run", "store", 1, None);
    assert!(
        call(
            "artifact.store",
            serde_json::json!({"value":1}),
            &env,
            &snapshots,
            Reach::Inputs,
            None,
        )
        .is_err()
    );
    assert!(
        call(
            "artifact.store",
            serde_json::json!({"value":1}),
            &env,
            &snapshots,
            Reach::Workspace,
            Some(&identity),
        )
        .is_err()
    );
    let fake = serde_json::json!({"schema":"runx.artifact.v1","ref":format!("runx:project-artifact:sha256:{}","a".repeat(64)),"digest":format!("sha256:{}","a".repeat(64)),"size_bytes":8,"media_type":"application/json","path":"/unavailable/secret","labels":{"sharing":"configured_model"}});
    let result = call(
        "artifact.open",
        serde_json::json!({"artifact":fake}),
        &env,
        &snapshots,
        Reach::Workspace,
        None,
    );
    let error = match result {
        Err(error) => error,
        Ok(_) => return Err("agent boundary admitted protected state".into()),
    };
    assert!(error.to_string().contains("protected operator state"));
    assert!(!project.exists());
    Ok(())
}

#[test]
fn project_artifact_open_refuses_tampering_foreign_roots_and_oversized_descriptors()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    let other = tempfile::tempdir()?;
    let artifact = store(workspace.path(), project.path(), serde_json::json!({"n":1}))?;
    let env = environment(workspace.path(), project.path());
    let fresh = LocalArtifactService::default();
    let foreign = environment(workspace.path(), other.path());
    assert!(
        call(
            "artifact.open",
            serde_json::json!({"artifact":artifact}),
            &foreign,
            &fresh,
            Reach::Inputs,
            None,
        )
        .is_err()
    );
    for field in ["size_bytes", "digest", "ref", "media_type"] {
        let mut value = serde_json::to_value(&artifact)?;
        value[field] = match field {
            "size_bytes" => serde_json::json!(crate::services::MAX_ARTIFACT_BYTES + 1),
            "digest" => serde_json::json!(format!("sha256:{}", "b".repeat(64))),
            "ref" => serde_json::json!("runx:project-artifact:wrong"),
            _ => serde_json::json!("text/plain"),
        };
        assert!(
            call(
                "artifact.open",
                serde_json::json!({"artifact":value}),
                &env,
                &fresh,
                Reach::Inputs,
                None,
            )
            .is_err()
        );
    }
    std::fs::write(
        source_path(&artifact)?,
        serde_json::to_vec(&serde_json::json!({"n":2}))?,
    )?;
    assert!(
        call(
            "artifact.open",
            serde_json::json!({"artifact":artifact}),
            &env,
            &fresh,
            Reach::Inputs,
            None,
        )
        .is_err()
    );
    assert!(
        crate::services::read_json_artifact(&project.path().join("artifacts"), &artifact).is_err()
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn project_artifact_open_refuses_file_symlinks_and_nonregular_sources()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    let artifact = store(workspace.path(), project.path(), serde_json::json!({"n":1}))?;
    let env = environment(workspace.path(), project.path());
    let source = source_path(&artifact)?;
    let moved = source.with_extension("retained");
    std::fs::rename(source, &moved)?;
    std::os::unix::fs::symlink(&moved, source)?;
    assert!(
        call(
            "artifact.open",
            serde_json::json!({"artifact":artifact}),
            &env,
            &LocalArtifactService::default(),
            Reach::Inputs,
            None,
        )
        .is_err()
    );
    std::fs::remove_file(source)?;
    std::fs::create_dir(source)?;
    assert!(
        call(
            "artifact.open",
            serde_json::json!({"artifact":artifact}),
            &env,
            &LocalArtifactService::default(),
            Reach::Inputs,
            None,
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn project_artifact_open_preserves_large_record_paging_without_a_control_blob()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    let count = 20_000_u64;
    let records = (0..count)
        .map(|id| serde_json::json!({"id":id,"label":"source record"}))
        .collect::<Vec<_>>();
    let artifact = store(
        workspace.path(),
        project.path(),
        serde_json::Value::Array(records),
    )?;
    let env = environment(workspace.path(), project.path());
    let snapshots = LocalArtifactService::default();
    let opened: Artifact = call(
        "artifact.open",
        serde_json::json!({"artifact":artifact}),
        &env,
        &snapshots,
        Reach::Inputs,
        None,
    )?
    .deserialize_into()?;
    let mut offset = 0;
    let mut next_id = 0;
    loop {
        let page = snapshots.read_json_array_page(&opened.reference, offset, 4096)?;
        for record in page.records {
            let value: serde_json::Value = serde_json::from_str(&record)?;
            assert_eq!(value["id"], next_id);
            next_id += 1;
        }
        if page.eof {
            break;
        }
        assert!(page.next_offset > offset);
        offset = page.next_offset;
    }
    assert_eq!(next_id, count);
    Ok(())
}

#[test]
fn what_is_stored_from_a_local_only_artifact_is_local_only_and_opens_so()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let project = tempfile::tempdir()?;
    let env = environment(workspace.path(), project.path());
    let source = store_inputs(
        workspace.path(),
        project.path(),
        serde_json::json!({"value":{"note":"kept here"},"labels":{"sharing":"local_only"}}),
    )?;
    assert_eq!(source.labels, ArtifactLabels::LOCAL_ONLY);
    // A value made from the local_only artifact is local_only by default.
    let derived = store(
        workspace.path(),
        project.path(),
        serde_json::json!({"summary":"made from it","from":source}),
    )?;
    assert_eq!(derived.labels, ArtifactLabels::LOCAL_ONLY);
    // A value made from nothing local_only is the configured model's.
    let shared = store(workspace.path(), project.path(), serde_json::json!({"n":1}))?;
    assert_eq!(shared.labels, ArtifactLabels::CONFIGURED_MODEL);
    // A default, not a wall: a label the step gives is the one kept.
    let relabelled = store_inputs(
        workspace.path(),
        project.path(),
        serde_json::json!({"value":{"from":source},"labels":{"sharing":"configured_model"}}),
    )?;
    assert_eq!(relabelled.labels, ArtifactLabels::CONFIGURED_MODEL);
    // Opening keeps the label, and so does every later read of the snapshot,
    // in a fresh service too.
    let opened: Artifact = call(
        "artifact.open",
        serde_json::json!({"artifact":derived}),
        &env,
        &LocalArtifactService::for_run(project.path(), "artifact-run"),
        Reach::Inputs,
        None,
    )?
    .deserialize_into()?;
    assert_eq!(opened.labels, ArtifactLabels::LOCAL_ONLY);
    let restored = LocalArtifactService::for_run(project.path(), "artifact-run");
    assert_eq!(
        restored.descriptor(&opened.reference)?.labels,
        ArtifactLabels::LOCAL_ONLY
    );
    Ok(())
}
