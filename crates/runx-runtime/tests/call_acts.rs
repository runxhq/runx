#![allow(clippy::expect_used, clippy::unwrap_used)]

//! A governed call's act, end to end: the invoker states `$act`, an operation
//! step binds it into its call's `act:`, and the call's own receipt seals the
//! act with the authority admission derived and the effect its operation
//! declares. A call in a child run is decided at its address.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use runx_contracts::{ActForm, DecisionChoice, Receipt, ReferenceType};
use runx_runtime::{HarnessReplayError, HarnessReplayOutput, run_harness_fixture};

const SKILL: &str = r#"
skill: call-acts
connectors:
  slack:
    operations: [channel.post]
runners:
  stated:
    default: true
    inputs:
      channel_locator: {type: string, required: true}
      text: {type: string, required: true}
      thread: {type: string, required: true}
      previous: {type: string, required: false}
      form: {type: string, required: false}
    steps:
      - id: post
        operation: slack/channel.post
        idempotency_key: post-1
        approval: {reason: Approve this exact post., type: slack_message}
        input: {channel_locator: $input.channel_locator, text: $input.text}
        act:
          form: $input.form
          purpose: $act.purpose
          reason: $act.reason
          target: $input.thread
          decision: close
          previous: $input.previous
  forwarded:
    inputs:
      channel_locator: {type: string, required: true}
      text: {type: string, required: true}
    steps:
      - id: post
        operation: slack/channel.post
        idempotency_key: post-1
        approval: {reason: Approve this exact post., type: slack_message}
        input: {channel_locator: $input.channel_locator, text: $input.text}
        act: $act
  turn:
    outputs:
      answer: string
"#;

const PREVIOUS: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";
const EFFECT: &str = "slack://T123/C456/1750000000.000001";

struct Case<'a> {
    name: &'a str,
    runner: &'a str,
    inputs: &'a str,
    act: &'a str,
    status: &'a str,
}

fn run(root: &Path, case: &Case<'_>) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let skill = root.join("call-acts");
    std::fs::create_dir_all(skill.join("fixtures")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: call-acts\ndescription: call-acts fixture.\n---\n# Call acts\n",
    )
    .unwrap();
    std::fs::write(skill.join("run.yaml"), SKILL).unwrap();
    let path: PathBuf = skill.join("fixtures").join(format!("{}.yaml", case.name));
    std::fs::write(
        &path,
        format!(
            r#"name: {name}
kind: skill
target: ..
runner: {runner}
inputs:
  channel_locator: slack://T123/C456
  text: Deploy finished.
{inputs}
caller:
{act}
  approvals:
    post: true
  provider_responses:
    principal_id: operator-fixture
    grant:
      grant_id: call-acts-fixture
      provider: slack
      scopes: [channel.post]
    operations:
      - operation: channel.post
        target: slack://T123/C456
        access: mutate
        result:
          message_locator: {EFFECT}
          conversation_locator: slack://T123/C456
expect:
  status: {status}
"#,
            name = case.name,
            runner = case.runner,
            inputs = case.inputs,
            act = case.act,
            status = case.status,
        ),
    )
    .unwrap();
    let env = std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default();
    run_harness_fixture(path, env)
}

fn post(output: &HarnessReplayOutput) -> &Receipt {
    &output
        .steps
        .iter()
        .find(|step| step.step_id == "post")
        .expect("the post step ran")
        .receipt
}

#[test]
fn the_invokers_act_is_sealed_in_the_call_receipt_with_admitted_authority_and_effect() {
    let root = tempfile::tempdir().unwrap();
    let output = run(
        root.path(),
        &Case {
            name: "stated",
            runner: "stated",
            inputs: &format!(
                "  thread: github://acme/app/issues/7\n  form: reply\n  previous: \"{PREVIOUS}\""
            ),
            act: "  act:\n    purpose: Tell the channel the deploy finished\n    reason: The release went out cleanly.",
            status: "sealed",
        },
    )
    .expect("the case seals");
    let receipt = post(&output);
    assert_eq!(receipt.acts.len(), 1, "one act per call receipt");
    let act = &receipt.acts[0];
    assert_eq!(act.form, ActForm::Reply);
    assert_eq!(
        act.intent.purpose.as_str(),
        "Tell the channel the deploy finished"
    );
    assert_eq!(act.reason.as_deref(), Some("The release went out cleanly."));
    // Legitimacy is the execution term admission derived, never prose.
    let term = receipt.authority.terms.first().expect("the admitted term");
    assert_eq!(
        act.intent.legitimacy.as_str(),
        format!("runx:authority_term:{}", term.term_id)
    );
    // Who acted is admission's: the term's principal, under its grant.
    assert_eq!(
        Some(&term.principal_ref),
        receipt.authority.actor_ref.as_ref()
    );
    assert!(
        receipt
            .authority
            .grant_refs
            .iter()
            .any(|grant| grant.reference_type == ReferenceType::Grant)
    );
    // The target is classified by the connectors' declared references.
    let target = act.target_refs.first().expect("the act's target");
    assert_eq!(target.uri.as_str(), "github://acme/app/issues/7");
    assert_eq!(target.reference_type, ReferenceType::Thread);
    // The effect is the one the operation declares, of its declared kind.
    let effect = act
        .artifact_refs
        .iter()
        .find(|reference| reference.uri.as_str() == EFFECT)
        .expect("the declared effect");
    assert_eq!(effect.reference_type, ReferenceType::Thread);
    assert_eq!(effect.provider.as_deref(), Some("slack"));
    // The decision and lineage are the act's.
    assert_eq!(receipt.decisions[0].choice, DecisionChoice::Close);
    assert_eq!(receipt.decisions[0].selected_act_id, act.id);
    assert_eq!(
        receipt
            .lineage
            .previous
            .as_ref()
            .map(|previous| previous.uri.to_string()),
        Some(format!("runx:receipt:{PREVIOUS}"))
    );
}

#[test]
fn a_whole_act_forwards_the_invokers_and_an_absent_one_leaves_the_operations_observation() {
    let root = tempfile::tempdir().unwrap();
    let forwarded = run(
        root.path(),
        &Case {
            name: "forwarded",
            runner: "forwarded",
            inputs: "",
            act: "  act:\n    purpose: Forward the invoker's act\n    decision: monitor",
            status: "sealed",
        },
    )
    .expect("the case seals");
    let receipt = post(&forwarded);
    assert_eq!(
        receipt.acts[0].intent.purpose.as_str(),
        "Forward the invoker's act"
    );
    assert_eq!(receipt.decisions[0].choice, DecisionChoice::Monitor);

    let absent = run(
        root.path(),
        &Case {
            name: "absent",
            runner: "forwarded",
            inputs: "",
            act: "",
            status: "sealed",
        },
    )
    .expect("the case seals");
    let receipt = post(&absent);
    let act = &receipt.acts[0];
    assert_eq!(act.form, ActForm::Observation);
    // Unstated, its purpose is what the operation declares it does.
    assert_eq!(act.intent.purpose.as_str(), "Post a message to a channel.");
    let term = receipt.authority.terms.first().expect("the admitted term");
    assert_eq!(
        act.intent.legitimacy.as_str(),
        format!("runx:authority_term:{}", term.term_id),
        "a governed call is legitimate by its term, stated act or not"
    );
    assert!(act.target_refs.is_empty());
    assert!(
        act.artifact_refs
            .iter()
            .any(|reference| reference.uri.as_str() == EFFECT),
        "an effect is the call's, whether or not its act is stated"
    );
    assert!(
        receipt.decisions.is_empty(),
        "no decision is sealed when none is stated"
    );
}

#[test]
fn an_act_outside_its_shape_refuses_the_call_before_admission() {
    let root = tempfile::tempdir().unwrap();
    // Nothing is admitted, so nothing reaches the provider and no call
    // receipt is sealed.
    for (name, inputs, act, refused) in [
        (
            "revision",
            "  thread: github://acme/app/issues/7\n  form: revision",
            "  act:\n    purpose: Revise the channel",
            "revision",
        ),
        (
            "no-purpose",
            "  thread: github://acme/app/issues/7",
            "",
            "purpose",
        ),
    ] {
        let error = run(
            root.path(),
            &Case {
                name,
                runner: "stated",
                inputs,
                act,
                status: "sealed",
            },
        )
        .expect_err("the call is refused")
        .to_string();
        assert!(
            error.contains("operation step inputs are invalid") && error.contains(refused),
            "{error}"
        );
    }
}

#[test]
fn a_turns_operation_call_seals_the_act_it_carries() {
    let root = tempfile::tempdir().unwrap();
    let skill = root.path().join("call-acts");
    std::fs::create_dir_all(skill.join("fixtures")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: call-acts\ndescription: call-acts fixture.\n---\n# Call acts\n",
    )
    .unwrap();
    std::fs::write(skill.join("run.yaml"), SKILL).unwrap();
    let path = skill.join("fixtures").join("turn.yaml");
    std::fs::write(
        &path,
        format!(
            r#"name: turn
kind: skill
target: ..
runner: turn
caller:
  turns:
    agent.turn.output:
      calls:
        - tool: slack__channel_post
          input:
            input: {{channel_locator: slack://T123/C456, text: Deploy finished.}}
            idempotency_key: post-1
            act: {{form: reply, purpose: Tell the channel, reason: In the agent's own words.}}
          expect:
            subset:
              outcome: {{status: completed}}
      output: {{answer: posted}}
  approvals:
    turn/0: true
  provider_responses:
    principal_id: operator-fixture
    grant:
      grant_id: call-acts-fixture
      provider: slack
      scopes: [channel.post]
    operations:
      - operation: channel.post
        target: slack://T123/C456
        access: mutate
        result:
          message_locator: {EFFECT}
          conversation_locator: slack://T123/C456
expect:
  status: sealed
"#
        ),
    )
    .unwrap();
    let env = std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default();
    let output = run_harness_fixture(path, env).expect("the turn seals");
    let call = output
        .steps
        .iter()
        .flat_map(|step| step.nested_receipts.iter())
        .find(|receipt| receipt.subject.reference.uri.as_str().ends_with("#turn/0"))
        .expect("the call's own receipt");
    let act = &call.acts[0];
    assert_eq!(act.form, ActForm::Reply);
    assert_eq!(act.intent.purpose.as_str(), "Tell the channel");
    assert_eq!(act.reason.as_deref(), Some("In the agent's own words."));
    assert!(
        act.intent
            .legitimacy
            .as_str()
            .starts_with("runx:authority_term:")
    );
    assert!(
        act.artifact_refs
            .iter()
            .any(|reference| reference.uri.as_str() == EFFECT)
    );
}

/// A parent whose skill step hands its child run a post, decided by
/// `approvals`.
fn delegate(root: &Path, approvals: &str) -> Result<HarnessReplayOutput, HarnessReplayError> {
    let parent = root.join("delegating");
    let child = parent.join("child");
    std::fs::create_dir_all(parent.join("fixtures")).unwrap();
    std::fs::create_dir_all(&child).unwrap();
    std::fs::write(
        child.join("SKILL.md"),
        "---\nname: posting\ndescription: Posts what it is given.\n---\n# Posting\n",
    )
    .unwrap();
    std::fs::write(
        child.join("run.yaml"),
        r#"skill: posting
connectors:
  slack:
    operations: [channel.post]
runners:
  post:
    default: true
    inputs:
      text: {type: string, required: true}
    steps:
      - id: post
        operation: slack/channel.post
        idempotency_key: post-1
        approval: {reason: Approve this exact post., type: slack_message}
        input: {channel_locator: slack://T123/C456, text: $input.text}
        act: $act
"#,
    )
    .unwrap();
    std::fs::write(
        parent.join("SKILL.md"),
        "---\nname: delegating\ndescription: Hands a post to its child.\n---\n# Delegating\n",
    )
    .unwrap();
    std::fs::write(
        parent.join("run.yaml"),
        r#"skill: delegating
runners:
  delegate:
    default: true
    steps:
      - id: send
        skill: ./child
        inputs: {text: Deploy finished.}
        act:
          purpose: Announce the deploy
          reason: $act.reason
"#,
    )
    .unwrap();
    let path = parent.join("fixtures").join("delegate.yaml");
    std::fs::write(
        &path,
        format!(
            r#"name: delegate
kind: skill
target: ..
caller:
  act: {{purpose: Run the release, reason: Release 0.8.2 shipped.}}
  approvals: {approvals}
  provider_responses:
    principal_id: operator-fixture
    grant:
      grant_id: call-acts-fixture
      provider: slack
      scopes: [channel.post]
    operations:
      - operation: channel.post
        target: slack://T123/C456
        access: mutate
        result:
          message_locator: {EFFECT}
          conversation_locator: slack://T123/C456
expect:
  status: sealed
"#
        ),
    )
    .unwrap();
    let env = std::env::var("PATH")
        .ok()
        .map(|path| BTreeMap::from([("PATH".to_owned(), path)]))
        .unwrap_or_default();
    run_harness_fixture(path, env)
}

#[test]
fn a_skill_step_gives_its_child_run_the_act_its_calls_read_as_dollar_act() {
    let root = tempfile::tempdir().unwrap();
    let output = delegate(root.path(), r#"{"send#post": true}"#).expect("the delegated post seals");
    let call = output
        .steps
        .iter()
        .flat_map(|step| step.nested_receipts.iter())
        .find(|receipt| receipt.subject.reference.uri.as_str().ends_with("#post"))
        .expect("the child call's receipt");
    let act = &call.acts[0];
    // The parent's step states the purpose; the invoker's reason reaches the
    // child's call through the act the step gives it, never through inputs.
    assert_eq!(act.intent.purpose.as_str(), "Announce the deploy");
    assert_eq!(act.reason.as_deref(), Some("Release 0.8.2 shipped."));
}

#[test]
fn a_child_runs_approval_is_decided_only_by_its_address() {
    let root = tempfile::tempdir().unwrap();
    // The child's `post` is `send#post` to the case; its id alone decides
    // nothing, so the post waits for a decision and nothing is sent.
    let error = delegate(root.path(), "{post: true}")
        .expect_err("the child's post is not approved")
        .to_string();
    assert!(error.contains("unanswered: send#post"), "{error}");
}
