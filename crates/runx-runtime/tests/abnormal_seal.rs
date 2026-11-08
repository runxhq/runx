use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::RunOutcome;
use runx_runtime::{
    InvocationOutput, SkillAdapter, SkillInvocation, TestExpectedStatus, TestOptions,
    run_test_fixture_with_adapter,
};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[test]
fn test_blocks_ending_and_seals_workflow_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let case = TempCase::new("abnormal-seal")?;
    case.write_skill("act", "act", "approved: {type: boolean}")?;
    case.write_skill("closure", "closure", "closed: {type: boolean}")?;
    case.write_workflow()?;
    case.write_test()?;

    let adapter = RecordingAdapter::default();
    let output =
        run_test_fixture_with_adapter(case.test_path(), adapter.clone(), runtime_options()?)?;

    assert_eq!(output.status, TestExpectedStatus::PolicyDenied);
    assert_eq!(output.receipt.seal.outcome, RunOutcome::Blocked);
    assert_eq!(
        output.receipt.seal.reason,
        Some(runx_contracts::SealReason::WorkflowBlocked)
    );
    assert_eq!(output.step_receipts.len(), 1);
    assert_eq!(output.step_receipts[0].seal.outcome, RunOutcome::Completed);
    assert_eq!(adapter.calls()?, vec!["act".to_owned()]);

    // Each step ran its child skill as a nested graph; the tree holds the
    // step receipts and every receipt beneath them.
    let children = output
        .steps
        .iter()
        .flat_map(|step| {
            step.nested_receipts
                .iter()
                .chain(std::iter::once(&step.receipt))
                .cloned()
        })
        .collect::<Vec<_>>();
    assert!(
        runx_runtime::validate_runtime_receipt_tree(
            &output.receipt,
            children,
            runx_receipts::ReceiptTreeConfig::default(),
            crate::support::fixture_policy()?,
        )
        .is_ok()
    );
    assert!(!output.receipt.lineage.children.is_empty());
    Ok(())
}

#[derive(Clone, Default)]
struct RecordingAdapter {
    calls: Arc<Mutex<Vec<String>>>,
}

impl RecordingAdapter {
    fn calls(&self) -> Result<Vec<String>, std::io::Error> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .map_err(|_| std::io::Error::other("adapter calls lock poisoned"))
    }
}

impl SkillAdapter for RecordingAdapter {
    fn invoke(
        &self,
        request: SkillInvocation,
    ) -> Result<InvocationOutput, runx_runtime::RuntimeError> {
        self.calls
            .lock()
            .map_err(|_| runx_runtime::RuntimeError::ReceiptInvalid {
                message: "adapter calls lock poisoned".to_owned(),
            })?
            .push(request.skill_name.clone());
        let value = match request.skill_name.as_str() {
            "act" => serde_json::json!({ "approved": false }),
            _ => serde_json::json!({ "closed": true }),
        };
        Ok(InvocationOutput::runtime_success(
            serde_json::from_value(value).map_err(|source| {
                runx_runtime::RuntimeError::ReceiptInvalid {
                    message: format!("building adapter output: {source}"),
                }
            })?,
            0,
        ))
    }
}

struct TempCase {
    root: PathBuf,
}

impl TempCase {
    fn new(name: &str) -> Result<Self, std::io::Error> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_nanos());
        let root = std::env::temp_dir().join(format!("runx-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn test_path(&self) -> PathBuf {
        self.root.join("tests/abnormal-seal.yaml")
    }

    fn write_skill(&self, directory: &str, name: &str, output: &str) -> Result<(), std::io::Error> {
        let skill_dir = self.root.join("fixtures").join(directory);
        crate::support::write_test_skill_package(
            &skill_dir,
            format!(
                r#"---
name: {name}
description: Abnormal seal test {name} skill.
---

Emits structured test output through the harness adapter.
"#
            )
            .as_str(),
            format!(
                r#"minimum_runx: 0.12.0
methods:
  {name}:
    steps:
      - id: {name}
        tool: {{command: runx-test-adapter, args: []}}
        outputs:
          {output}
"#
            )
            .as_str(),
        )
    }

    /// The workflow under test, a fixture package beside the skills it
    /// composes.
    fn write_workflow(&self) -> Result<(), std::io::Error> {
        crate::support::write_test_skill_package(
            &self.root.join("fixtures/abnormal-seal-gate"),
            "---\nname: abnormal-seal-gate\ndescription: A test fixture that composes abnormal-seal with the packages it calls.\n---\n\nUsed only by abnormal-seal's tests.\n",
            r#"minimum_runx: 0.12.0
catalog:
  visibility: internal
methods:
  run:
    steps:
      - id: act
        skill: ../act
      - id: closure
        guard:
          - {value: $steps.act.approved, equals: true}
        skill: ../closure
"#,
        )
    }

    fn write_test(&self) -> Result<(), std::io::Error> {
        fs::create_dir_all(self.root.join("tests"))?;
        fs::write(
            self.test_path(),
            r#"name: abnormal-seal
package: fixtures/abnormal-seal-gate
expect:
  status: policy_denied
  receipt:
    schema: runx.receipt.v1
    state: sealed
    outcome: blocked
    reason: workflow_blocked
  steps:
    - act
"#,
        )
    }
}

impl Drop for TempCase {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn runtime_options() -> Result<TestOptions, runx_runtime::RuntimeError> {
    let mut options = TestOptions::new(std::env::vars().collect())?;
    options.clock = runx_runtime::RunClock::Fixed(FIXTURE_CREATED_AT.to_owned());
    Ok(options)
}
