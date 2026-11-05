import assert from "node:assert/strict";
import { test } from "node:test";
import { finalizeAttention } from "./attention-review.mjs";
const base = {
  objective: "Check new personal observations.",
  as_of: "2026-10-02T10:00:00Z",
  evidence: [
    {
      source_id: "chat-1",
      source_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      thread_locator: "https://github.com/example/project/pull/12",
      source_kind: "chat",
      observed_at: "2026-10-02T09:55:00Z",
      summary: "A colleague requests review of a pull request.",
    },
  ],
  current_actions: [{ source_id: "chat-1", state: "open" }],
  allowed_action_ids: [],
  work_routes: [
    {
      source_id: "chat-1",
      source_digest: "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
      route_id: "pr_status",
      target_locator: "https://github.com/example/project/pull/12",
    },
  ],
  min_check_minutes: 15,
  max_check_minutes: 60,
};
const draft = {
  decision: "brief",
  items: [{ source_id: "chat-1", priority: "medium" }],
  recommended_action_ids: [],
  work_proposals: [
    {
      source_id: "chat-1",
      route_id: "pr_status",
      target_locator: "https://github.com/example/project/pull/12",
      inputs: {},
      due_at_unix_seconds: 0,
    },
  ],
  next_check_minutes: 20,
};
const run = (input = base, answer = draft) =>
  finalizeAttention({ ...input, draft: answer }).attention_packet;
test("declared route selectors bind the exact source occurrence and target", () => {
  assert.equal(run().decision, "ready");
  for (const field of ["source_id", "source_digest", "target_locator"]) {
    const input = structuredClone(base);
    input.work_routes[0][field] =
      field === "source_digest" ? "sha256:" + "c".repeat(64) : "different";
    const result = run(input);
    assert.equal(result.decision, "held");
    assert.deepEqual(result.work_proposals, []);
  }
});
test("a generic route remains usable and protects host-owned inputs", () => {
  const input = structuredClone(base);
  input.work_routes = [
    {
      route_id: "pr_status",
      source_kinds: ["chat"],
      fixed_inputs: { account: "held-account" },
      source_context_input: "source_context",
    },
  ];
  const answer = structuredClone(draft);
  answer.work_proposals[0].inputs = {
    account: "forged",
    source_context: { forged: true },
    query: "status",
  };
  const result = run(input, answer);
  assert.equal(result.decision, "ready");
  assert.deepEqual(result.work_proposals[0].inputs, { query: "status" });
  assert.equal(result.delivery_status, "not_sent");
});
test("resolved and contradictory recorded state cannot become ready work", () => {
  const input = structuredClone(base);
  input.current_actions[0].state = "resolved";
  assert.equal(run(input).decision, "held");
  input.current_actions.push({ source_id: input.evidence[0].source_id, state: "open" });
  assert.equal(run(input).decision, "held");
});
test("an empty selection stays idle and the host bounds its check delay", () => {
  const answer = {
    decision: "brief",
    items: [],
    recommended_action_ids: [],
    work_proposals: [],
    next_check_minutes: 0,
  };
  const result = run(base, answer);
  assert.equal(result.decision, "idle");
  assert.equal(result.brief, "");
  assert.equal(result.next_check_minutes, base.min_check_minutes);
});
