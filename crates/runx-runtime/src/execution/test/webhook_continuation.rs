//! A stable webhook receipt resumes without delivering another request.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use runx_contracts::ResumeDocument;
use serde_json::{Value, json};

use crate::effects::RuntimeEffectRegistry;
use crate::kernel::operation::OperationEffect;
use crate::{LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, SkillRunRequest};

#[test]
fn an_acknowledged_webhook_resumes_its_original_run_without_redelivery()
-> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/webhook-handoff");
    let fixture = super::load_test_fixture(skill.join("tests/default-delivers-n8n.yaml"))?;
    let mut given: Value = serde_json::from_str(&fs::read_to_string(
        skill.join("evals/files/n8n-handoff.json"),
    )?)?;
    given
        .as_object_mut()
        .ok_or("handoff input")?
        .remove("about");
    let mut env = BTreeMap::new();
    crate::paths::isolate(&mut env, root.path());
    env.extend(fixture.env.clone());
    let paths = crate::RunxPaths::resolve(&env, &skill);
    let scope = crate::ConnectorScope::in_project(&skill, &paths)?;
    let transport = super::transport::test_transport(&fixture.providers, &scope, &[], &skill)?;
    super::replay::seed_test_instances(
        &fixture,
        &skill,
        &skill,
        &super::replay::TestOptions::new(env.clone())?,
        &transport,
        &[],
    )?;
    let exchanges = transport.exchanges.clone();
    let mut effects = RuntimeEffectRegistry::default();
    effects.replace_effect(OperationEffect::test(transport, &[], &[]))?;
    let start = LocalOrchestrator::with_effects(effects.clone()).run_skill_with_binding(
        &SkillRunRequest {
            current_context: Vec::new(),
            historical_context: Vec::new(),
            run_authority: None,
            act: None,
            skill_path: skill,
            receipt_dir: None,
            run_id: Some("webhook-case-109".to_owned()),
            answers: None,
            inputs: serde_json::from_value(given)?,
            env: env.clone(),
            cwd: root.path().to_path_buf(),
            managed_agent: ManagedAgentPolicy::HostDriven,
        },
        Some("send"),
        None,
        None,
    )?;
    assert!(!start.succeeded());
    let read = crate::run_record::read_run(&env, root.path(), "webhook-case-109", None)?;
    let [held] = read.waiting.as_slice() else {
        return Err("expected one exact webhook approval".into());
    };
    assert_eq!(held.address.as_str(), "deliver");
    let approved = serde_json::from_value(json!({"approvals": {
        held.address.as_str(): {"request_digest": held.request_digest.as_str(), "approved": true}
    }}))?;
    let resume = |answers| ResumeRequest {
        run_id: "webhook-case-109".to_owned(),
        answers,
        receipt_dir: None,
        env: env.clone(),
        cwd: root.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
        package_digest: None,
        closure_digest: None,
    };
    let unanswered = LocalOrchestrator::with_effects(effects.clone())
        .resume(&resume(ResumeDocument::default()))?;
    assert!(!unanswered.succeeded());
    assert_eq!(exchanges.request_counts()?, [0]);
    let completed = LocalOrchestrator::with_effects(effects.clone()).resume(&resume(approved))?;
    assert!(completed.succeeded(), "{:?}", completed.output);
    let output = serde_json::to_value(&completed.output)?;
    assert_eq!(
        output["result"]["provider_operation"]["finality"],
        "acknowledged"
    );
    assert_eq!(
        output["result"]["provider_operation"]["result"]["acknowledgement"]["message"],
        "Workflow was started"
    );
    assert_eq!(exchanges.request_counts()?, [1]);
    let sealed =
        LocalOrchestrator::with_effects(effects).resume(&resume(ResumeDocument::default()))?;
    assert_eq!(completed.output, sealed.output);
    assert_eq!(exchanges.request_counts()?, [1]);
    Ok(())
}
