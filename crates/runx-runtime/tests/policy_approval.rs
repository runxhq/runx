use std::collections::BTreeMap;

use runx_contracts::{
    ExecutionEvent, JsonValue, ReferenceType, ResolutionRequest, ResolutionResponse,
    ResolutionResponseActor,
};
use runx_parser::{parse_graph_yaml, validate_graph};
use runx_runtime::adapters::process_tool::ProcessToolAdapter;
use runx_runtime::{Host, Runtime, RuntimeError};

const CREATED_AT: &str = "2026-08-27T00:00:00Z";

#[test]
fn explicit_approval_guard_binds_its_receipt_to_the_governed_step()
-> Result<(), Box<dyn std::error::Error>> {
    let workspace = tempfile::tempdir()?;
    let graph = policy_write_graph(workspace.path(), true)?;
    let runtime = runtime(workspace.path(), "run_explicit_guard")?;
    let mut host = ApprovalHost::new(true);

    let run = runtime.run_graph_with_host(workspace.path(), graph, &mut host)?;
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
    let routine_graph = policy_write_graph(routine_workspace.path(), false)?;
    runtime(routine_workspace.path(), "run_routine_write")?.run_graph_with_host(
        routine_workspace.path(),
        routine_graph,
        &mut ApprovalHost::new(false),
    )?;
    assert_eq!(
        std::fs::read_to_string(routine_workspace.path().join("approved.txt"))?,
        "approved"
    );

    let guarded_workspace = tempfile::tempdir()?;
    let guarded_graph = policy_write_graph(guarded_workspace.path(), true)?;
    let error = match runtime(guarded_workspace.path(), "run_guarded_write")?.run_graph_with_host(
        guarded_workspace.path(),
        guarded_graph,
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
    let child_graph = child_policy_graph(workspace.path());
    let indented_graph = child_graph
        .lines()
        .filter(|line| !line.starts_with("name:"))
        .map(|line| format!("    {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(
        child.join("run.yaml"),
        format!(
            r#"skill: child-policy-write
runners:
  write:
    default: true
{indented_graph}
"#,
        ),
    )?;
    let parent = validate_graph(parse_graph_yaml(
        r#"name: parent-policy-write
result_from: [child]
steps:
  - id: approve
    ask: { gate: parent.child.approval, reason: Approve the nested write. }
  - id: child
    skill: ./child
    runner: write
policy:
  guards:
    - step: child
      value: $steps.approve.approval_decision.approved
      equals: true
"#,
    )?)?;

    let mut host = ApprovalHost::new(true);
    let run = runtime(workspace.path(), "run_nested_parent")?.run_graph_with_host(
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

    let child_graph = validate_graph(parse_graph_yaml(&child_graph)?)?;
    runtime(workspace.path(), "run_nested_child")?.run_graph_with_host(
        &child,
        child_graph,
        &mut ApprovalHost::new(true),
    )?;
    Ok(())
}

fn child_policy_graph(workspace: &std::path::Path) -> String {
    format!(
        r#"name: child-policy-write
result_from: [write]
steps:
  - id: write
    tool: fs.write
    scopes: [fs.write]
    inputs:
      repo_root: {root:?}
      path: nested.txt
      contents: nested"#,
        root = workspace.to_string_lossy()
    )
}

fn policy_write_graph(
    workspace: &std::path::Path,
    with_guard: bool,
) -> Result<runx_parser::ExecutionGraph, Box<dyn std::error::Error>> {
    let policy = if with_guard {
        r#"policy:
  guards:
    - step: write
      value: $steps.approve.approval_decision.approved
      equals: true
"#
    } else {
        ""
    };
    Ok(validate_graph(parse_graph_yaml(&format!(
        r#"name: policy-write
result_from: [write]
steps:
  - id: approve
    ask: {{ gate: policy.write.approval, reason: Approve the exact fixture write. }}
  - id: write
    tool: fs.write
    scopes: [fs.write]
    inputs:
      repo_root: {root:?}
      path: approved.txt
      contents: approved
{policy}"#,
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
    options.created_at = CREATED_AT.to_owned();
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
