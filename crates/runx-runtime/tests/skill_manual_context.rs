use std::fs;

use runx_contracts::{
    ExecutionEvent, JsonValue, ResolutionRequest, canonical_stable_json, sha256_prefixed,
};
#[cfg(feature = "agent")]
use runx_runtime::adapters::agent::{AgentExecutionTelemetry, ManagedAgentError};
use runx_runtime::{Host, InvocationOutput, Runtime, RuntimeError, SkillAdapter, SkillInvocation};

#[derive(Default)]
struct RecordingHost {
    requests: Vec<ResolutionRequest>,
}

impl Host for RecordingHost {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<runx_contracts::ResolutionResponse>, RuntimeError> {
        self.requests.push(request);
        Ok(None)
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

struct UnusedAdapter;

impl SkillAdapter for UnusedAdapter {
    fn invoke(&self, _request: SkillInvocation) -> Result<InvocationOutput, RuntimeError> {
        Ok(InvocationOutput::runtime_failure(
            JsonValue::Null,
            "agent context test unexpectedly invoked the adapter",
            0,
        ))
    }
}

#[cfg(feature = "agent")]
struct MisidentifiedManagedFailureHost;

#[cfg(feature = "agent")]
impl Host for MisidentifiedManagedFailureHost {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        request: ResolutionRequest,
    ) -> Result<Option<runx_contracts::ResolutionResponse>, RuntimeError> {
        let request_id = match request {
            ResolutionRequest::AgentAct { id, .. } => id.as_str().to_owned(),
            _ => "managed-agent".to_owned(),
        };
        Err(RuntimeError::ManagedAgentResolution {
            step_id: "referenced-agent-runner".to_owned(),
            request_id,
            source: Box::new(ManagedAgentError::bounded_failure(
                "round_budget_exhausted",
                "Managed agent exhausted its bounded run.",
                AgentExecutionTelemetry::default(),
            )),
        })
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}

#[cfg(feature = "agent")]
#[test]
fn graph_execution_binds_managed_failure_to_the_owning_step()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    fs::write(
        temp.path().join("SKILL.md"),
        "---\nname: owner\ndescription: Failure identity fixture.\n---\n\n# Owner\n",
    )?;
    fs::write(
        temp.path().join("graph.yaml"),
        "name: owner\nsteps:\n  - id: owning-step\n    agent: { role: test, task: work }\n    outputs:\n      result: object\n",
    )?;
    let runtime = Runtime::new(
        UnusedAdapter,
        crate::support::fixture_runtime_options(
            std::env::vars().collect(),
            crate::support::run_in(temp.path(), "run_manual_context")?,
        )?,
    );

    let error = match runtime.run_graph_file_with_host(
        &temp.path().join("graph.yaml"),
        &mut MisidentifiedManagedFailureHost,
    ) {
        Ok(_) => return Err("managed failure must propagate".into()),
        Err(error) => error,
    };

    assert!(matches!(
        error,
        RuntimeError::ManagedAgentResolution { step_id, .. } if step_id == "owning-step"
    ));
    Ok(())
}

#[test]
fn skill_manual_context_is_exact_digest_bound_and_progressive()
-> Result<(), Box<dyn std::error::Error>> {
    let temp = tempfile::tempdir()?;
    let adjacent_dir = temp.path().join("context/adjacent");
    fs::create_dir_all(&adjacent_dir)?;
    let root_manual = "---\nname: root-operator\ndescription: Root operating model\n---\n\n# Root operator\n\nKeep this exact line and trailing blank.\n\n";
    let adjacent_manual = "---\nname: adjacent\ndescription: Adjacent specialist\n---\n\n# Adjacent specialist\n\nLoad this only when invoked.\n";
    fs::write(temp.path().join("SKILL.md"), root_manual)?;
    fs::write(adjacent_dir.join("SKILL.md"), adjacent_manual)?;
    fs::write(
        adjacent_dir.join("run.yaml"),
        "skill: adjacent\ncatalog:\n  kind: skill\n  audience: operator\n  visibility: internal\n  role: context\n  execution: read\n  completion: plan\n  requires_adapter: false\n  approval: none\nrunners:\n  operate:\n    default: true\n    steps:\n      - id: operate\n        agent: { role: operator, task: operate }\n        outputs:\n          result: object\n",
    )?;
    fs::write(
        temp.path().join("context.yaml"),
        "name: context-summary\nsteps:\n  - id: judge\n    agent: { role: operator, task: judge }\n    outputs:\n      result: object\n    context_skills:\n      - ./context/adjacent\n",
    )?;
    fs::write(
        temp.path().join("invoke.yaml"),
        "name: invoke-adjacent\nsteps:\n  - id: adjacent\n    skill: ./context/adjacent\n    runner: operate\n",
    )?;
    let runtime = Runtime::new(
        UnusedAdapter,
        crate::support::fixture_runtime_options(
            std::env::vars().collect(),
            crate::support::run_in(temp.path(), "run_manual_context")?,
        )?,
    );

    let mut summary_host = RecordingHost::default();
    let result =
        runtime.run_graph_file_with_host(&temp.path().join("context.yaml"), &mut summary_host);
    assert!(
        matches!(
            result,
            Err(RuntimeError::ResolutionPending { ref step_id, .. }) if step_id == "judge"
        ),
        "unexpected context graph result: {result:?}"
    );
    let summary_request = summary_host
        .requests
        .first()
        .ok_or("agent request missing")?;
    let envelope = agent_envelope(summary_request)?;
    assert_eq!(envelope.instructions.as_ref(), root_manual);
    assert_eq!(
        envelope.instructions_sha256.as_ref(),
        sha256_prefixed(root_manual.as_bytes())
    );
    let adjacent_context = envelope
        .current_context
        .first()
        .ok_or("adjacent skill context missing")?;
    assert_eq!(
        adjacent_context.data.get("content_kind"),
        Some(&JsonValue::String("skill-manual".to_owned()))
    );
    assert_eq!(
        adjacent_context.data.get("manual_sha256"),
        Some(&JsonValue::String(sha256_prefixed(
            adjacent_manual.as_bytes()
        )))
    );
    assert_eq!(
        adjacent_context.data.get("content"),
        Some(&JsonValue::String(adjacent_manual.to_owned()))
    );
    let catalog = adjacent_context
        .data
        .get("catalog")
        .and_then(JsonValue::as_object)
        .ok_or("adjacent catalog summary missing")?;
    assert_eq!(
        catalog.get("role"),
        Some(&JsonValue::String("context".to_owned()))
    );
    assert_eq!(
        catalog.get("approval"),
        Some(&JsonValue::String("none".to_owned()))
    );
    let canonical = canonical_stable_json(&JsonValue::Object(adjacent_context.data.clone()))?;
    assert_eq!(
        adjacent_context.meta.hash.as_ref(),
        sha256_prefixed(canonical.as_bytes())
    );
    assert_eq!(adjacent_context.meta.size_bytes, canonical.len() as u64);
    assert!(!adjacent_context.data.contains_key("sha256"));

    let wire = serde_json::to_vec(summary_request)?;
    let resumed: ResolutionRequest = serde_json::from_slice(&wire)?;
    assert_eq!(agent_envelope(&resumed)?.instructions.as_ref(), root_manual);

    let invoking = Runtime::new(
        UnusedAdapter,
        crate::support::fixture_runtime_options(
            std::env::vars().collect(),
            crate::support::run_in(temp.path(), "run_manual_invoke")?,
        )?,
    );
    let mut invoked_host = RecordingHost::default();
    let result =
        invoking.run_graph_file_with_host(&temp.path().join("invoke.yaml"), &mut invoked_host);
    assert!(
        matches!(
            result,
            Err(RuntimeError::ResolutionPending { ref step_id, .. }) if step_id == "adjacent"
        ),
        "unexpected adjacent graph result: {result:?}"
    );
    let invoked = invoked_host
        .requests
        .first()
        .ok_or("adjacent request missing")?;
    assert_eq!(
        agent_envelope(invoked)?.instructions.as_ref(),
        adjacent_manual
    );
    Ok(())
}

fn agent_envelope(
    request: &ResolutionRequest,
) -> Result<&runx_contracts::AgentContextEnvelope, Box<dyn std::error::Error>> {
    match request {
        ResolutionRequest::AgentAct { invocation, .. } => Ok(&invocation.envelope),
        _ => Err("expected agent resolution request".into()),
    }
}
