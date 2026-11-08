use std::collections::BTreeMap;

use runx_contracts::{
    ExecutionEvent, JsonValue, ReferenceType, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_parser::{parse_workflow_yaml, validate_workflow};
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::{Host, Runtime, RuntimeError};

const CREATED_AT: &str = "2026-08-27T00:00:00Z";

#[test]
fn explicit_approval_guard_binds_its_receipt_to_the_governed_step()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let graph = policy_write_workflow(workspace.path(), true)?;
    let runtime = runtime(workspace.path(), "run_explicit_guard")?;
    let mut host = ApprovalHost::new(true);

    let run = runtime.run_workflow_with_host(workspace.path(), graph, &mut host)?;
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
fn scoped_write_does_not_invent_approval_and_an_explicit_denied_guard_still_blocks()
-> Result<(), Box<dyn std::error::Error>> {
    let routine_workspace = tempfile::tempdir()?;
    let routine_workflow = policy_write_workflow(routine_workspace.path(), false)?;
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
    let guarded_workflow = policy_write_workflow(guarded_workspace.path(), true)?;
    let error = match runtime(guarded_workspace.path(), "run_guarded_write")?
        .run_workflow_with_host(
            guarded_workspace.path(),
            guarded_workflow,
            &mut ApprovalHost::new(false),
        ) {
        Err(error) => error,
        Ok(_) => return Err("an explicit denied approval guard did not fail closed".into()),
    };
    assert!(matches!(error, RuntimeError::AuthorityDenied { .. }));
    assert!(error.to_string().contains("approval guard"));
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
    let child_workflow = child_policy_workflow(workspace.path());
    let indented_workflow = child_workflow
        .lines()
        .filter(|line| !line.starts_with("name:"))
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        child.join("run.yaml"),
        format!(
            r#"minimum_runx: 0.12.0
methods:
  write:
{indented_workflow}
"#,
        ),
    )?;
    let parent = validate_workflow(parse_workflow_yaml(
        r#"name: parent-policy-write
result_from: [child]
steps:
  - id: approve
    ask: {gate: parent.child.approval, reason: Approve the nested write.}
  - id: child
    guard:
      - {value: $steps.approve.approval_decision.approved, equals: true}
    skill: ./child#write
"#,
    )?)?;

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

    let child_workflow = validate_workflow(parse_workflow_yaml(&child_workflow)?)?;
    runtime(workspace.path(), "run_nested_child")?.run_workflow_with_host(
        &child,
        child_workflow,
        &mut ApprovalHost::new(true),
    )?;
    Ok(())
}

fn child_policy_workflow(workspace: &std::path::Path) -> String {
    format!(
        r#"name: child-policy-write
result_from: [write]
steps:
  - id: write
    tool: fs.write
    inputs:
      repo_root: {root:?}
      path: nested.txt
      contents: nested"#,
        root = workspace.to_string_lossy()
    )
}

fn policy_write_workflow(
    workspace: &std::path::Path,
    with_guard: bool,
) -> Result<runx_parser::Workflow, Box<dyn std::error::Error>> {
    let guard = if with_guard {
        "    guard:\n      - {value: $steps.approve.approval_decision.approved, equals: true}\n"
    } else {
        ""
    };
    Ok(validate_workflow(parse_workflow_yaml(&format!(
        r#"name: policy-write
result_from: [write]
steps:
  - id: approve
    ask: {{ gate: policy.write.approval, reason: Approve the exact fixture write. }}
  - id: write
{guard}    tool: fs.write
    inputs:
      repo_root: {root:?}
      path: approved.txt
      contents: approved
"#,
        root = workspace.to_string_lossy()
    ))?)?)
}

fn runtime(
    workspace: &std::path::Path,
    run_id: &str,
) -> Result<Runtime<ProcessToolAdapter>, Box<dyn std::error::Error>> {
    let mut options = crate::support::fixture_runtime_options(
        BTreeMap::new(),
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
