import assert from "node:assert/strict";
import { test } from "node:test";
import finalize from "./team-delegate.mjs";
const job = {
  objective: "Review the reported setup-guide drift before preparing a documentation draft.",
  roster: [
    {
      member: "researcher",
      skill: "runx/prior-art",
      method: "prior-art-agent",
      scope_ceiling: ["fs.read"],
      capabilities:
        "Inspect the operator-selected local source paths and return cited findings or missing evidence. No code edits or provider calls.",
    },
    {
      member: "writer",
      skill: "runx/ghostwrite",
      method: "draft",
      scope_ceiling: [],
      capabilities: "Draft only from actual review findings when those are available.",
    },
  ],
  evidence: [
    {
      source_id: "issue:docs-drift",
      text: "The operator reports possible setup-guide drift and selects README.md for inspection. Treat the report as a claim; verify the document before asserting an omission.",
    },
  ],
};
const ready = {
  decision: "propose",
  rationale: "The source review is ready; documentation writing waits for its actual findings.",
  assignments: [
    {
      member: "researcher",
      task: "Inspect README.md against the supplied report about explicit connection selection. Return source-bound findings or identify missing evidence; do not edit documentation.",
      input_context: {
        objective:
          "Inspect README.md against the supplied report about explicit connection selection. Return source-bound findings or identify missing evidence; do not edit documentation.",
        repo_root: ".",
        source_paths: ["README.md"],
        workflow_purpose: "Documentation source review",
        audience: "Local operator",
      },
      source_ids: ["issue:docs-drift"],
      needed_scope: [],
      acceptance: [
        "Return findings cited to the selected document, or identify missing source evidence.",
        "Do not edit the document or claim the writer has run.",
      ],
      verification:
        "Check the receiver receipt and source-bound review; keep unavailable evidence explicit.",
    },
  ],
  missing_context: ["The writer needs the source-review findings before drafting."],
};
const digest = "sha256:" + "a".repeat(64);
test("ready work survives an explicit missing downstream prerequisite", () => {
  const result = finalize({ ...job, draft: ready, input_digest: digest }).delegation_packet;
  assert.equal(result.decision, "proposed");
  assert.equal(result.assignments.length, 1);
  assert.deepEqual(result.missing_context, ready.missing_context);
  assert.deepEqual(result.assignments[0].evidence, job.evidence);
  assert.equal(result.assignments[0].authority_status, "not_granted");
  assert.equal(result.execution_status, "not_started");
});
test("needs_input cannot expose runnable assignments", () => {
  const result = finalize({
    ...job,
    draft: { ...ready, decision: "needs_input" },
    input_digest: digest,
  }).delegation_packet;
  assert.equal(result.decision, "held");
  assert.deepEqual(result.assignments, []);
});
