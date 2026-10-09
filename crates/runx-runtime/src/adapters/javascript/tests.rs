use std::collections::BTreeMap;
use std::fs;

use runx_parser::WorkerTool;

use super::*;
use crate::credentials::CredentialDelivery;

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

#[test]
fn wall_limit_failure_records_exact_receipt_metadata() -> Result<(), Box<dyn std::error::Error>> {
    let limits = runx_contracts::javascript_worker::InvocationLimits {
        wall_milliseconds: 7_000,
        ..runx_contracts::javascript_worker::InvocationLimits::default()
    };
    let output = project_worker_outcome(
        Instant::now(),
        supervisor::WorkerInvocationOutcome {
            result: supervisor::WorkerInvocationResult::Failure {
                code: runx_contracts::javascript_worker::WorkerFailureCode::ResourceLimit,
                limit: Some(runx_contracts::javascript_worker::WorkerLimit::WallMilliseconds),
                message: "wall limit reached".to_owned(),
                disposition: runx_contracts::javascript_worker::WorkerDisposition::Discard,
            },
            execution_boundary: crate::process_invocation::boundary_metadata(
                runx_contracts::ExecutionBoundaryKind::DeterministicWorker,
            )?,
        },
        limits,
    )?;

    let hit = output
        .metadata
        .get(crate::adapter::EXECUTION_LIMITS_METADATA)
        .and_then(JsonValue::as_object)
        .and_then(|limits| limits.get("hit"))
        .and_then(JsonValue::as_object)
        .ok_or("structured limit hit is missing")?;
    assert_eq!(
        hit.get("id"),
        Some(&JsonValue::String(
            "javascript.wall_milliseconds".to_owned()
        ))
    );
    // Compare canonical JSON, not the JsonNumber variant: the serde bridge
    // stores in-range integers as I64 and the wire encoding is identical.
    assert_eq!(
        hit.get("configured")
            .map(serde_json::to_string)
            .transpose()?,
        Some("7000".to_owned())
    );
    assert_eq!(
        hit.get("manifest_field"),
        Some(&JsonValue::String("tool.timeout_seconds".to_owned()))
    );
    Ok(())
}

fn invocation(skill_directory: &std::path::Path) -> SkillInvocation {
    let _ = fs::create_dir_all(skill_directory);
    SkillInvocation {
        historical_context: Vec::new(),
        skill_name: "javascript-test".to_owned(),
        step_id: None,
        artifacts: None,
        requirements: Default::default(),
        executable: crate::adapter::Executable::Worker(WorkerTool {
            module: "domain.mjs".to_owned(),
            export: None,
            timeout_seconds: None,
            pages: None,
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
