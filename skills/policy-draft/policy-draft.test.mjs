import assert from "node:assert/strict";
import { test } from "node:test";
import { preparePolicy, finalizePolicy } from "./policy-draft.mjs";
const policy = {
  schema: "runx.operational_policy.v1",
  schema_version: "runx.operational_policy.v1",
  policy_id: "test",
  sources: [
    {
      source_id: "inbound",
      provider: "github",
      allowed_locators: ["github://test/repo/issues"],
      allowed_actions: ["issue-intake"],
      source_thread: { required: true, publish_mode: "comment", missing_behavior: "fail_closed" },
      minimum_confidence: 0.7,
    },
  ],
  runners: [
    {
      method_id: "review",
      kind: "local",
      state: "available",
      allowed_actions: ["issue-intake"],
      target_repos: ["test/repo"],
      scafld_required: true,
    },
  ],
  owner_routes: [{ route_id: "maintainers", owners: ["one", "two"], target_repos: ["test/repo"] }],
  targets: [
    {
      repo: "test/repo",
      method_ids: ["review"],
      allowed_actions: ["issue-intake"],
      default_owner_route: "maintainers",
      scafld_required: true,
    },
  ],
  dedupe: { strategy: "source_digest", key_fields: ["signal.digest"], on_duplicate: "reuse" },
  outcomes: {
    observe_provider: true,
    verification_required: true,
    close_source_issue: "when_verified",
    publish_final_source_thread_update: false,
  },
  permissions: { auto_merge: false, mutate_target_repo: false, require_human_merge_gate: true },
};
const run = (next, existing = policy) =>
  preparePolicy({
    existing_policy: existing,
    policy_proposal: {
      decision: "ready",
      policy: next,
      rationale: "bounded test",
      blockers: [],
      needs_input: [],
      success_checkpoint: {},
    },
  }).policy_context;
test("known review, observation, runner and publication controls cannot weaken", () => {
  for (const mutate of [
    (p) => (p.sources[0].source_thread.required = false),
    (p) => (p.targets[0].scafld_required = false),
    (p) => (p.outcomes.verification_required = false),
    (p) => (p.outcomes.publish_final_source_thread_update = true),
    (p) => (p.outcomes.close_source_issue = "when_terminal"),
  ]) {
    const next = structuredClone(policy);
    mutate(next);
    assert.equal(run(next).decision, "reject");
  }
  for (const state of ["disabled", "maintenance"]) {
    const stopped = structuredClone(policy);
    stopped.runners[0].state = state;
    const context = run(policy, stopped);
    assert.equal(context.decision, "reject");
    assert.deepEqual(context.attenuation_findings.map((finding) => finding.path), ["runner.review.state"]);
    assert.equal(run(stopped, stopped).path, "lint");
  }
});
test("owner and opaque changes remain in the exact native comparison input", () => {
  const next = structuredClone(policy);
  next.owner_routes[0].owners.push("other");
  next.sources[0].adapter_policy = { github: { approval: "relaxed" } };
  const context = run(next);
  assert.notDeepEqual(context.fixed_existing, context.fixed_proposed);
  const final = finalizePolicy({
    policy_context: context,
    policy_lint: { status: "pass", findings: [], readback: {} },
    fixed_comparison: { equal: false },
  });
  assert.equal(final.policy_proposal.decision, "reject");
  assert.equal(final.policy_proposal.validation.status, "fail");
});
test("roster reordering and native empty defaults do not change fixed meaning", () => {
  const next = structuredClone(policy);
  next.owner_routes[0].owners.reverse();
  next.owner_routes[0].labels = [];
  next.sources[0].adapter_policy = {};
  const context = run(next);
  assert.deepEqual(context.fixed_existing, context.fixed_proposed);
  next.sources[0].minimum_confidence = 0.8;
  assert.equal(run(next).path, "lint");
});
