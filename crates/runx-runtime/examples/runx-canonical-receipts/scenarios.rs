//! The scenarios behind the canonical receipts Cloud's spec shows: real test
//! replays, sealed as `runx test` seals them, under the public fixture key at
//! the fixed clock. The generator writes what they seal, and the integration
//! test holds the fixtures to it.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::Path;

use runx_contracts::Receipt;
use runx_runtime::{TestReplayOutput, run_test_fixture};

/// Where the canonical receipts are kept, from the repository root.
pub(crate) const FIXTURE_DIR: &str = "fixtures/contracts/canonical-receipts";

/// One canonical receipt and the fixture file that holds it.
pub(crate) struct CanonicalReceipt {
    pub(crate) file: &'static str,
    pub(crate) receipt: Receipt,
}

/// The three canonical receipts: the hello-workflow example's first step, as
/// its child run seals it; a governed call whose caller states a reply that
/// closes a thread; and the hello-workflow run itself.
pub(crate) fn canonical_receipts(repo: &Path) -> Result<Vec<CanonicalReceipt>, Box<dyn Error>> {
    let workflow = run_test_fixture(
        repo.join("examples/hello-workflow/tests/hello-workflow.yaml"),
        replay_env(),
    )?;
    let first = workflow
        .steps
        .iter()
        .find(|step| step.step_id == "first")
        .ok_or("the hello-workflow run has no first step")?
        .nested_receipts
        .iter()
        .find(|receipt| {
            receipt
                .subject
                .reference
                .uri
                .as_str()
                .ends_with("/first#default")
        })
        .ok_or("the first step's child run sealed no step receipt")?
        .clone();
    let call = governed_call(repo)?;
    let post = call
        .steps
        .iter()
        .find(|step| step.step_id == "post")
        .ok_or("the governed call case has no post step")?
        .receipt
        .clone();
    Ok(vec![
        CanonicalReceipt {
            file: "workflow-step.receipt.json",
            receipt: first,
        },
        CanonicalReceipt {
            file: "governed-call.receipt.json",
            receipt: post,
        },
        CanonicalReceipt {
            file: "workflow.receipt.json",
            receipt: workflow.receipt,
        },
    ])
}

/// A fixture file's bytes for `receipt`: its canonical JSON, as every
/// contract fixture and test oracle keeps a receipt.
pub(crate) fn render(receipt: &Receipt) -> Result<String, runx_receipts::ReceiptError> {
    Ok(format!(
        "{}\n",
        runx_receipts::canonical_receipt_json(receipt)?
    ))
}

/// The environment a replay sees: only where its tools are found.
fn replay_env() -> BTreeMap<String, String> {
    std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default()
}

const CALL_SKILL: &str = r#"minimum_runx: 0.12.0
methods:
  stated:
    inputs:
      channel_locator: {type: string}
      text: {type: string}
    steps:
      - id: post
        operation: fixture/chat/channel.post
        idempotency_key: post-1
        approval: {reason: Approve this exact post., type: chat_message}
        inputs: {channel_locator: $input.channel_locator, text: $input.text}
        act: $run.act
"#;

const CALL_MANUAL: &str =
    "---\nname: call-acts\ndescription: call-acts fixture.\n---\n# Call acts\n";

/// The caller states a reply that closes the thread; the operation posts it,
/// and the hosted provider acknowledges the post and reads it back.
const CALL_CASE: &str = r#"name: stated
method: stated
inputs:
  channel_locator: chat://T123/C456
  text: Deploy finished.
host:
  act:
    form: reply
    purpose: Tell the channel the deploy finished
    reason: The release went out cleanly.
    target: forge://acme/app/issues/7
    decision: close
  approvals:
    post: true
providers:
  hosted:
    principal_id: operator-fixture
    grant:
      grant_id: call-acts-fixture
      provider: fixture/chat
      scopes: [channel.post]
    operations:
      - operation: channel.post
        target: chat://T123/C456
        access: mutate
        result:
          message_locator: chat://T123/C456/1750000000.000001
          conversation_locator: chat://T123/C456
expect:
  status: sealed
"#;

/// The governed call, run in a plugin of its own that holds the kernel's
/// fixture chat and forge connectors, as `runx test` runs a project's case.
fn governed_call(repo: &Path) -> Result<TestReplayOutput, Box<dyn Error>> {
    let root = tempfile::tempdir()?;
    let connectors = repo.join("fixtures/plugins/kernel/connectors");
    for id in ["fixture/chat", "fixture/forge"] {
        copy_dir(
            &connectors.join(id),
            &root.path().join("connectors").join(id),
        )?;
    }
    let skill = root.path().join("call-acts");
    fs::create_dir_all(skill.join("tests"))?;
    fs::write(skill.join("SKILL.md"), CALL_MANUAL)?;
    fs::write(skill.join("run.yaml"), CALL_SKILL)?;
    let case = skill.join("tests/stated.yaml");
    fs::write(&case, CALL_CASE)?;
    Ok(run_test_fixture(case, replay_env())?)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
