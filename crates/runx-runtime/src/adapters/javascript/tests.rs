use std::collections::BTreeMap;
use std::fs;

use runx_parser::WorkerTool;

use super::*;
use crate::adapter::Executable;
use crate::credentials::CredentialDelivery;
use runx_parser::{ArtifactPageFraming, ArtifactPageInput, ArtifactPageSource};

fn page_package(
    root: &std::path::Path,
) -> Result<crate::LoadedSkillPackage, Box<dyn std::error::Error>> {
    fs::write(
        root.join("SKILL.md"),
        "---\nname: javascript-test\ndescription: Count admitted synthetic records.\n---\n# Page proof\n\nRead only the native snapshot.\n",
    )?;
    fs::write(
        root.join("run.yaml"),
        "minimum_runx: 0.12.0\nmethods:\n  count:\n    steps:\n    - id: count\n      tool: {module: domain.mjs, requires: [parseData]}\n",
    )?;
    fs::write(
        root.join("domain.mjs"),
        r#"export default function (inputs) {
      const page = inputs.runx_page;
      const state = {...(page.state ?? {count:0,sum:0})};
      for (const encoded of page.records) {
        const record = Runx.parseData(encoded, "json");
        state.count++; state.sum += record.id;
      }
      return {runx_page:{state,done:page.eof},...(page.eof ? {
        count:state.count,sum:state.sum,source_digest:page.whole_digest,
        leaked_source:Object.hasOwn(inputs,"source_uri") || Object.hasOwn(inputs,"source_path") || Object.hasOwn(inputs,"source_scope")
      } : {})};
    }"#,
    )?;
    Ok(crate::load_validated_skill_package(root)?)
}

fn page_invocation(
    root: &std::path::Path,
    source: ArtifactPageInput,
    page_bytes: u64,
) -> SkillInvocation {
    let mut request = invocation(root);
    request.requirements.scopes.push("fs.read".to_owned());
    if let Executable::Worker(worker) = &mut request.executable {
        worker
            .requires
            .push(runx_contracts::javascript_worker::WorkerApi::ParseData);
        worker.paged_input = Some(ArtifactPageSource {
            source,
            framing: ArtifactPageFraming::JsonArray,
            page_bytes,
        });
    }
    request
}

#[test]
fn admitted_artifact_pages_match_path_results_at_two_scales_and_reuse_one_worker()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let package = page_package(root)?;
    let adapter = JavaScriptAdapter::new_session();
    let artifacts = crate::services::LocalArtifactService::default();
    for count in [1_500_u64, 12_000] {
        let records = (0..count)
            .map(|id| serde_json::json!({"id":id,"text":"日本語 \\\" 😀"}))
            .collect::<Vec<_>>();
        fs::write(root.join("records.json"), serde_json::to_vec(&records)?)?;
        let snapshot = artifacts.admit(
            root,
            std::path::Path::new("records.json"),
            "application/json",
            crate::WorkspaceBounds::Operator,
        )?;
        for page_bytes in [65_536, 524_288] {
            let mut by_path = page_invocation(
                root,
                ArtifactPageInput::Path {
                    path: "source_path".to_owned(),
                    path_scope: Some("source_scope".to_owned()),
                    media_type: "application/json".to_owned(),
                },
                page_bytes,
            );
            by_path.workspace_bounds = crate::WorkspaceBounds::Agent;
            by_path.inputs = JsonObject::from([
                (
                    "source_path".to_owned(),
                    JsonValue::String("records.json".to_owned()),
                ),
                (
                    "source_scope".to_owned(),
                    JsonValue::String("skill".to_owned()),
                ),
            ]);
            let mut by_artifact = page_invocation(
                root,
                ArtifactPageInput::Artifact {
                    artifact: "source_uri".to_owned(),
                },
                page_bytes,
            );
            by_artifact.inputs.insert(
                "source_uri".to_owned(),
                JsonValue::String(snapshot.reference.clone()),
            );
            let from_path = adapter.invoke_with_artifacts(by_path, &package, &artifacts)?;
            let from_artifact = adapter.invoke_with_artifacts(by_artifact, &package, &artifacts)?;
            assert_eq!(from_path.status, InvocationStatus::Success, "{from_path:?}");
            assert_eq!(
                from_artifact.status,
                InvocationStatus::Success,
                "{from_artifact:?}"
            );
            assert_eq!(from_path.value, from_artifact.value);
            assert_eq!(
                serde_json::to_value(from_artifact.value)?,
                serde_json::json!({
                    "count":count,"sum":count*(count-1)/2,"source_digest":snapshot.whole_digest,"leaked_source":false
                })
            );
        }
    }
    assert_eq!(adapter.session_stats().spawned_process_count, 1);
    Ok(())
}

#[test]
fn admitted_artifact_pages_use_the_snapshot_and_require_native_scope_and_lifetime()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let package = page_package(root)?;
    fs::write(root.join("records.json"), "[{\"id\":7}]")?;
    let artifacts = crate::services::LocalArtifactService::default();
    let snapshot = artifacts.admit(
        root,
        std::path::Path::new("records.json"),
        "application/json",
        crate::WorkspaceBounds::Operator,
    )?;
    fs::write(root.join("records.json"), "[]")?;
    let adapter = JavaScriptAdapter::new_session();
    let mut request = page_invocation(
        root,
        ArtifactPageInput::Artifact {
            artifact: "source_uri".to_owned(),
        },
        65_536,
    );
    request.inputs.insert(
        "source_uri".to_owned(),
        JsonValue::String(snapshot.reference),
    );
    for invalid in ["records.json", "not-a-native-reference"] {
        let mut malformed = request.clone();
        malformed.inputs.insert(
            "source_uri".to_owned(),
            JsonValue::String(invalid.to_owned()),
        );
        let error = adapter
            .invoke_with_artifacts(malformed, &package, &artifacts)
            .err()
            .ok_or("a path-shaped or malformed artifact reference was admitted")?;
        assert!(
            error.to_string().contains(
                &crate::services::WorkspaceFileError::InvalidArtifactReference.to_string()
            ),
            "{error}"
        );
        assert_eq!(adapter.session_stats().spawned_process_count, 0);
    }
    let output = adapter.invoke_with_artifacts(request.clone(), &package, &artifacts)?;
    assert_eq!(output.status, InvocationStatus::Success, "{output:?}");
    assert_eq!(serde_json::to_value(output.value)?["sum"], 7);
    let error = adapter
        .invoke_with_artifacts(
            request.clone(),
            &package,
            &crate::services::LocalArtifactService::default(),
        )
        .err()
        .ok_or("cross-runtime artifact was admitted")?;
    assert!(
        error
            .to_string()
            .contains(&crate::services::WorkspaceFileError::UnknownArtifactReference.to_string()),
        "{error}"
    );
    request.requirements.scopes.clear();
    request.inputs.insert(
        "source_uri".to_owned(),
        JsonValue::String("not-a-native-reference".to_owned()),
    );
    let error = adapter
        .invoke_with_artifacts(request, &package, &artifacts)
        .err()
        .ok_or("missing page scope was admitted")?;
    assert!(error.to_string().contains("fs.read"), "{error}");
    assert_eq!(adapter.session_stats().spawned_process_count, 1);
    Ok(())
}

#[test]
fn rejects_credentials_before_loading_a_module() -> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let mut request = invocation(directory.path());
    request.credential_delivery = CredentialDelivery::process_env(
        "example",
        "token",
        "EXAMPLE_TOKEN",
        "local:test",
        "secret",
    )?;
    let error = JavaScriptAdapter::default()
        .invoke(request)
        .err()
        .map(|error| error.to_string());
    assert!(error.is_some_and(|message| message.contains("cannot receive credentials")));
    Ok(())
}

fn invocation(skill_directory: &std::path::Path) -> SkillInvocation {
    let _ = fs::create_dir_all(skill_directory);
    SkillInvocation {
        workspace_bounds: crate::WorkspaceBounds::Operator,
        historical_context: Vec::new(),
        skill_name: "javascript-test".to_owned(),
        step_id: None,
        artifacts: None,
        requirements: Default::default(),
        executable: crate::adapter::Executable::Worker(WorkerTool {
            requires: Vec::new(),
            module: "domain.mjs".to_owned(),
            export: None,
            timeout_seconds: None,
            paged_input: None,
            environment: Default::default(),
        }),
        outputs: None,
        inputs: JsonObject::new(),
        resolved_inputs: JsonObject::new(),
        current_context: Vec::new(),
        provenance: Vec::new(),
        skill_directory: skill_directory.to_path_buf(),
        env: BTreeMap::new(),
        credential_delivery: CredentialDelivery::none(),
    }
}
