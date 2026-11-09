#![allow(clippy::expect_used, clippy::unwrap_used)]

//! A skill that stores an artifact, end to end through the test: the
//! kernel reads a ledger entry and sends its allocation to the test's runx, which keeps the case
//! principal's artifact space, and confirms it by reading the reference back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_runtime::{TestExpectedStatus, TestReplayError, TestReplayOutput, run_test_fixture};

const CONNECTOR: &str = r#"schema: runx.connector.v1
name: ledger
title: Acme Ledger
description: Read one ledger entry.
category: agent-tools
auth:
  credentials:
    service_token:
      kind: api_key
      fields:
        - {name: token, label: Acme service token}
      delivery: {env: ACME_SERVICE_TOKEN}
      audience: [api.example.com]
placements:
  hosted: {auth: credential, credential: service_token}
operations:
  entry.read:
    description: Read one ledger entry.
    verb: read
    scopes: [entry.read]
    credentials: [service_token]
    target:
      template: "acme:ledger"
    result_fields: [entry]
"#;

const SKILL: &str = r#"
minimum_runx: 0.12.0
methods:
  keep:
    steps:
      - id: read
        operation: ledger/entry.read
        result:
          required: [entry]
        inputs: {}
      - id: keep
        operation: artifact/artifact.allocate
        idempotency_key: $run.id
        inputs:
          purpose: notes.kept.v1
          media_type: application/json
          value: $steps.read.provider_operation.result.entry
"#;

fn run(root: &Path, name: &str) -> Result<TestReplayOutput, TestReplayError> {
    std::fs::write(root.join("plugin.json"), r#"{"namespace":"acme"}"#).unwrap();
    std::fs::create_dir_all(root.join("connectors/ledger")).unwrap();
    std::fs::write(root.join("connectors/ledger/connector.yaml"), CONNECTOR).unwrap();
    // The plugin is the workspace, as a publisher lays one out: its skills/ beside
    // its connectors/.
    let skill = root.join("skills/notes");
    std::fs::create_dir_all(skill.join("tests")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: notes\ndescription: notes fixture.\n---\n# Notes\n",
    )
    .unwrap();
    std::fs::write(skill.join("run.yaml"), SKILL).unwrap();
    let providers = "providers:\n  hosted:\n    principal_id: operator-fixture\n    grant: {grant_id: ledger-fixture, provider: ledger, scopes: [entry.read]}\n    operations:\n      - {operation: entry.read, target: \"acme:ledger\", access: read, result: {entry: {amount: 7}}}\n";
    let path: PathBuf = skill.join("tests").join(format!("{name}.yaml"));
    std::fs::write(
        &path,
        format!("name: {name}\nmethod: keep\n{providers}expect:\n  status: sealed\n"),
    )
    .unwrap();
    let env = std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default();
    run_test_fixture(path, env)
}

#[test]
fn a_case_stores_and_reads_back_an_artifact_in_its_principals_space() {
    let root = tempfile::tempdir().unwrap();
    let output = run(root.path(), "keeps").expect("the allocation seals");

    assert_eq!(output.status, TestExpectedStatus::Sealed);
    let keep = output
        .steps
        .iter()
        .find(|step| step.step_id == "keep")
        .expect("the allocation ran");
    let contract = serde_json::to_value(&keep.contract).unwrap();
    let result = &contract["provider_operation"]["data"]["result"];
    assert!(
        result["artifact_uri"]
            .as_str()
            .is_some_and(|reference| reference.starts_with("runx:artifact:sha256:")),
        "{contract:#}"
    );
    assert_eq!(result["media_type"], "application/json", "{contract:#}");
    assert_eq!(
        result["principal_uri"], "runx:principal:operator-fixture",
        "{contract:#}"
    );
}
