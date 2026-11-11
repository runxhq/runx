//! Charge eval 9 needs a real paused run, not a JSON claim that one exists.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use runx_contracts::ResumeDocument;
use serde_json::{Value, json};

use super::provider_responses::TestTransport;
use crate::effects::RuntimeEffectRegistry;
use crate::http::{
    RuntimeHttpError, RuntimeHttpRequest, RuntimeHttpResponse, RuntimeHttpTransport,
};
use crate::kernel::operation::OperationEffect;
use crate::{LocalOrchestrator, ManagedAgentPolicy, ResumeRequest, SkillRunRequest};

/// Observe only provider-operation bodies; all responses still come from the
/// existing isolated provider fixture, which refuses undeclared operations.
#[derive(Clone)]
struct ObservedProvider {
    inner: TestTransport,
    calls: Arc<Mutex<Vec<String>>>,
}

impl RuntimeHttpTransport for ObservedProvider {
    fn send(&self, request: RuntimeHttpRequest) -> Result<RuntimeHttpResponse, RuntimeHttpError> {
        if request.url.ends_with("/v1/provider-operations") {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(request.body.clone().unwrap_or_default());
        }
        self.inner.send(request)
    }
}

#[test]
fn an_admitted_charge_resumes_its_original_run_and_submits_once()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let skill = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/charge");
    let fixture = super::load_test_fixture(skill.join("tests/hosted-contract.yaml"))?;
    let mut given: Value = serde_json::from_str(&fs::read_to_string(
        skill.join("evals/files/admitted-charge.json"),
    )?)?;
    given
        .as_object_mut()
        .ok_or("charge context is an object")?
        .remove("about");
    let mut env = BTreeMap::new();
    crate::paths::isolate(&mut env, workspace.path());
    env.extend(super::transport::provider_env(&fixture.providers));
    let paths = crate::RunxPaths::resolve(&env, &skill);
    let scope = crate::ConnectorScope::in_project(&skill, &paths)?;
    super::transport::seed_provider_grant(&fixture.providers, &scope, &[], &env, &skill)?;
    let calls = Arc::new(Mutex::new(Vec::new()));
    let transport = ObservedProvider {
        inner: super::transport::test_transport(&fixture.providers, &scope, &[], &skill)?,
        calls: Arc::clone(&calls),
    };
    let mut effects = RuntimeEffectRegistry::default();
    effects.replace_effect(OperationEffect::test(transport, &[], &[]))?;
    let request = SkillRunRequest {
        current_context: Vec::new(),
        historical_context: Vec::new(),
        run_authority: None,
        act: None,
        skill_path: skill,
        receipt_dir: None,
        run_id: Some("charge-case-5".to_owned()),
        answers: None,
        inputs: serde_json::from_value(given.clone())?,
        env: env.clone(),
        cwd: workspace.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
    };
    let start = LocalOrchestrator::with_effects(effects.clone()).run_skill_with_binding(
        &request,
        Some("charge"),
        None,
        None,
    )?;
    assert!(!start.succeeded(), "the native approval must remain open");
    let read = crate::run_record::read_run(&env, workspace.path(), "charge-case-5", None)?;
    let [waiting] = read.waiting.as_slice() else {
        return Err(format!("expected one native payment approval: {:?}", read.waiting).into());
    };
    assert_eq!(waiting.address.as_str(), "settle");
    let address = waiting.address.to_string();
    let digest = waiting.request_digest.to_string();
    let resume = |answers| ResumeRequest {
        run_id: "charge-case-5".to_owned(),
        answers,
        receipt_dir: None,
        env: env.clone(),
        cwd: workspace.path().to_path_buf(),
        managed_agent: ManagedAgentPolicy::HostDriven,
        package_digest: None,
        closure_digest: None,
    };
    let still_waiting = LocalOrchestrator::with_effects(effects.clone())
        .resume(&resume(ResumeDocument::default()))?;
    assert!(!still_waiting.succeeded());
    assert!(
        calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    );
    let approved = serde_json::from_value(json!({
        "approvals": {address: {"request_digest": digest, "approved": true}}
    }))?;
    let completed = LocalOrchestrator::with_effects(effects.clone()).resume(&resume(approved))?;
    assert!(completed.succeeded(), "{:?}", completed.output);
    let output = serde_json::to_value(&completed.output)?;
    assert_eq!(output["run_id"], "charge-case-5");
    let result = &output["result"]["provider_operation"];
    assert_eq!(result["operation"], "payment.x402.merchant.read");
    assert_eq!(result["finality"], "confirmed");
    assert_eq!(result["result"]["payment_id"], "x402pay_test_001");
    assert_eq!(result["result"]["resource"], given["request"]["resource"]);
    assert_eq!(
        result["result"]["amount_minor"],
        given["request"]["amount_minor"]
    );
    assert_eq!(result["result"]["currency"], given["request"]["currency"]);
    let again =
        LocalOrchestrator::with_effects(effects).resume(&resume(ResumeDocument::default()))?;
    assert_eq!(
        completed.output, again.output,
        "a sealed resume returns the original seal"
    );
    let observed = calls
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let observed: Vec<Value> = observed
        .iter()
        .map(|body| serde_json::from_str(body))
        .collect::<Result<_, _>>()?;
    assert_eq!(
        observed
            .iter()
            .map(|call| call["operation"].as_str())
            .collect::<Vec<_>>(),
        [
            Some("payment.x402.merchant.settle"),
            Some("payment.x402.merchant.read")
        ]
    );
    let original = given["request"]
        .as_object()
        .ok_or("original charge request is an object")?;
    for (field, value) in original {
        assert_eq!(
            &observed[0]["input"][field], value,
            "settlement preserves {field}"
        );
    }
    assert_eq!(observed[1]["input"]["payment_id"], "x402pay_test_001");
    assert_eq!(
        observed[1]["input"]["resource"],
        given["request"]["resource"]
    );
    let key = observed[0]["input"]["idempotency_key"]
        .as_str()
        .ok_or("native provider retry key")?;
    assert!(!key.is_empty());
    assert_eq!(result["result"]["idempotency_key"], key);
    assert_eq!(
        result["result"]["request_binding_digest"],
        given["request"]["request_binding_digest"]
    );
    Ok(())
}
