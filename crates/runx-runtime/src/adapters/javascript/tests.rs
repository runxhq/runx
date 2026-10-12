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
