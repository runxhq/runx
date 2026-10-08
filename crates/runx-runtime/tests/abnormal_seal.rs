#![cfg(feature = "process-tool")]

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use runx_contracts::{ClosureDisposition, JsonObject};
use runx_runtime::{
    HarnessExpectedStatus, HarnessOptions, InvocationOutput, SkillAdapter, SkillInvocation,
    run_harness_fixture_with_adapter,
};

const FIXTURE_CREATED_AT: &str = "2026-05-18T00:00:00Z";

#[test]
fn harness_blocks_closure_and_seals_graph_receipt() -> Result<(), Box<dyn std::error::Error>> {
    let case = TempCase::new("abnormal-seal")?;
    case.write_skill("act", "act", "approved: boolean")?;
    case.write_skill("closure", "closure", "closed: boolean")?;
    case.write_graph()?;
    case.write_harness()?;

    let adapter = RecordingAdapter::default();
    let output =
        run_harness_fixture_with_adapter(case.harness_path(), adapter.clone(), runtime_options())?;

    assert_eq!(output.status, HarnessExpectedStatus::PolicyDenied);
    assert_eq!(output.receipt.seal.disposition, ClosureDisposition::Blocked);
    assert_eq!(output.receipt.seal.reason_code, "graph_blocked");
    assert_eq!(output.step_receipts.len(), 1);
    assert_eq!(
        output.step_receipts[0].seal.disposition,
        ClosureDisposition::Closed
    );
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
            runx_receipts::ReceiptTreeConfig::default()
        )
        .is_ok()
    );
    assert!(
        output
            .receipt
            .lineage
            .as_ref()
            .is_some_and(|lineage| !lineage.children.is_empty())
    );
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
            JsonObject::default(),
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

    fn harness_path(&self) -> PathBuf {
        self.root.join("harness.yaml")
    }

    fn write_skill(&self, directory: &str, name: &str, output: &str) -> Result<(), std::io::Error> {
        let skill_dir = self.root.join(directory);
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
                r#"skill: {name}
runners:
  {name}:
    default: true
    steps:
      - id: {name}
        tool: {{ command: runx-test-adapter, args: [] }}
        outputs:
          {output}
"#
            )
            .as_str(),
        )
    }

    fn write_graph(&self) -> Result<(), std::io::Error> {
        fs::write(
            self.root.join("graph.yaml"),
            r#"name: abnormal-seal-gate
owner: runx
steps:
  - id: act
    skill: ./act
  - id: closure
    skill: ./closure
policy:
  guards:
    - step: closure
      field: act.approved
      equals: true
"#,
        )
    }

    fn write_harness(&self) -> Result<(), std::io::Error> {
        fs::write(
            self.harness_path(),
            r#"name: abnormal-seal
kind: graph
target: graph.yaml
expect:
  status: policy_denied
  receipt:
    schema: runx.receipt.v1
    state: sealed
    disposition: blocked
    reason_code: graph_blocked
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

fn runtime_options() -> HarnessOptions {
    let mut options = HarnessOptions::local_development(std::env::vars().collect());
    options.created_at = FIXTURE_CREATED_AT.to_owned();
    options
}
