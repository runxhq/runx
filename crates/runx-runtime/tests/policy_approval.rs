use std::collections::BTreeMap;

use runx_contracts::{
    ExecutionEvent, JsonValue, ReferenceType, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::{Host, Runtime, RuntimeError};

const CREATED_AT: &str = "2026-08-27T00:00:00Z";

#[test]
fn explicit_approval_guard_binds_its_receipt_to_the_governed_step()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let workflow = policy_write_workflow(true)?;
    let runtime = runtime(workspace.path(), "run_explicit_guard")?;
    let mut host = ApprovalHost::new(true);

    let run = runtime.run_workflow_with_host(workspace.path(), workflow, &mut host)?;
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("approved.txt"))?,
        "approved"
    );

    let approval = run
        .steps
        .iter()
        .find(|step| step.step_id == "approve")
        .ok_or("missing approval step")?;
    let write = run
        .steps
        .iter()
        .find(|step| step.step_id == "write")
        .ok_or("missing write step")?;
    let expected_uri = format!("runx:receipt:{}", approval.receipt.id);
    // The approval is the evidence the step's outcome was allowed.
    assert!(write.receipt.seal.criteria.iter().any(|criterion| {
        criterion.verification_refs.iter().any(|reference| {
            reference.reference_type == ReferenceType::Receipt
                && reference.uri.as_str() == expected_uri
                && reference.locator.as_ref() == Some(&approval.receipt.digest)
        })
    }));
    Ok(())
}

#[test]
fn scoped_write_does_not_invent_approval_and_an_explicit_declined_guard_still_blocks()
-> Result<(), Box<dyn std::error::Error>> {
    let routine_workspace = tempfile::tempdir()?;
    let routine_workflow = policy_write_workflow(false)?;
    runtime(routine_workspace.path(), "run_routine_write")?.run_workflow_with_host(
        routine_workspace.path(),
        routine_workflow,
        &mut ApprovalHost::new(false),
    )?;
    assert_eq!(
        std::fs::read_to_string(routine_workspace.path().join("approved.txt"))?,
        "approved"
    );

    let guarded_workspace = tempfile::tempdir()?;
    let guarded_workflow = policy_write_workflow(true)?;
    let error = match runtime(guarded_workspace.path(), "run_guarded_write")?
        .run_workflow_with_host(
            guarded_workspace.path(),
            guarded_workflow,
            &mut ApprovalHost::new(false),
        ) {
        Err(error) => error,
        Ok(_) => return Err("an explicit declined approval guard did not fail closed".into()),
    };
    // A person's no is declined, never a refusal (changed 10 November: it
    // was authority denied).
    assert!(matches!(error, RuntimeError::Declined { .. }));
    // With no reason given, the decline names the gate the person saw.
    assert!(
        error
            .to_string()
            .contains("the person declined the policy.write.approval gate, with no reason given"),
        "{error}"
    );
    assert!(!guarded_workspace.path().join("approved.txt").exists());
    Ok(())
}

#[test]
fn parent_approval_binds_the_composite_step_without_becoming_child_authority()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let child = workspace.path().join("child");
    std::fs::create_dir_all(&child)?;
    std::fs::write(
        child.join("SKILL.md"),
        "---\nname: child-policy-write\ndescription: nested policy fixture\n---\n# Child\n",
    )?;
    let child_run_yaml = child_policy_run_yaml();
    std::fs::write(child.join("run.yaml"), &child_run_yaml)?;
    let parent = crate::support::run_yaml_workflow(
        "parent-policy-write",
        r#"
minimum_runx: 0.12.0
methods:
  run:
    result_from: [child]
    steps:
      - id: approve
        ask: {gate: parent.child.approval, reason: Approve the nested write.}
      - id: child
        guard:
          - {value: $steps.approve.approval_decision.approved, equals: true}
        skill: ./child#write
"#,
    )?;

    let mut host = ApprovalHost::new(true);
    let run = runtime(workspace.path(), "run_nested_parent")?.run_workflow_with_host(
        workspace.path(),
        parent,
        &mut host,
    )?;
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("nested.txt"))?,
        "nested"
    );
    let approval = run
        .steps
        .iter()
        .find(|step| step.step_id == "approve")
        .ok_or("missing approval step")?;
    let child_step = run
        .steps
        .iter()
        .find(|step| step.step_id == "child")
        .ok_or("missing child step")?;
    let expected_uri = format!("runx:receipt:{}", approval.receipt.id);
    assert!(child_step.receipt.seal.criteria.iter().any(|criterion| {
        criterion.verification_refs.iter().any(|reference| {
            reference.reference_type == ReferenceType::Receipt
                && reference.uri.as_str() == expected_uri
        })
    }));

    // The child writes nested.txt as a new file, which never overwrites one
    // blind, so the file goes before the child runs alone.
    std::fs::remove_file(workspace.path().join("nested.txt"))?;
    let child_workflow = crate::support::run_yaml_workflow("child-policy-write", &child_run_yaml)?;
    runtime(workspace.path(), "run_nested_child")?.run_workflow_with_host(
        &child,
        child_workflow,
        &mut ApprovalHost::new(true),
    )?;
    Ok(())
}

/// The child package's `run.yaml`: one method, `write`, that writes
/// `nested.txt` into the run's workspace.
fn child_policy_run_yaml() -> String {
    r#"minimum_runx: 0.12.0
methods:
  write:
    result_from: [write]
    steps:
      - id: write
        tool: fs.write
        inputs:
          path: nested.txt
          contents: nested
"#
    .to_owned()
}

fn policy_write_workflow(
    with_guard: bool,
) -> Result<runx_parser::Workflow, Box<dyn std::error::Error>> {
    let guard = if with_guard {
        "        guard:\n          - {value: $steps.approve.approval_decision.approved, equals: true}\n"
    } else {
        ""
    };
    crate::support::run_yaml_workflow(
        "policy-write",
        &format!(
            r#"minimum_runx: 0.12.0
methods:
  run:
    result_from: [write]
    steps:
      - id: approve
        ask: {{ gate: policy.write.approval, reason: Approve the exact fixture write. }}
      - id: write
{guard}        tool: fs.write
        inputs:
          path: approved.txt
          contents: approved
"#
        ),
    )
}

fn runtime(
    workspace: &std::path::Path,
    run_id: &str,
) -> Result<Runtime<ProcessToolAdapter>, Box<dyn std::error::Error>> {
    // The workspace its writes are from, as every path is.
    let mut options = crate::support::fixture_runtime_options(
        BTreeMap::from([(
            "RUNX_CWD".to_owned(),
            workspace.to_string_lossy().into_owned(),
        )]),
        crate::support::run_in(workspace, run_id)?,
    )?;
    options.clock = runx_runtime::RunClock::Fixed(CREATED_AT.to_owned());
    Ok(Runtime::new(ProcessToolAdapter, options))
}

struct ApprovalHost {
    approved: bool,
}

impl ApprovalHost {
    fn new(approved: bool) -> Self {
        Self { approved }
    }
}

impl Host for ApprovalHost {
    fn report(&mut self, _event: ExecutionEvent) -> Result<(), RuntimeError> {
        Ok(())
    }

    fn resolve(
        &mut self,
        _request: ResolutionRequest,
    ) -> Result<Option<ResolutionResponse>, RuntimeError> {
        Ok(Some(ResolutionResponse {
            actor: ResolutionResponseActor::Human,
            payload: JsonValue::Bool(self.approved),
        }))
    }

    fn log(&mut self, _message: String) -> Result<(), RuntimeError> {
        Ok(())
    }
}
