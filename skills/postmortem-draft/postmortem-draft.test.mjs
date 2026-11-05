import assert from "node:assert/strict";
import { test } from "node:test";
import { finalizePostmortem } from "./postmortem-draft.mjs";
const fragments = [
  {
    id: "log-1",
    source: "incident log",
    text: "10:00 errors rose. 10:05 rollback restored service. Investigation confirmed the pool setting caused timeouts.",
  },
];
const draft = {
  summary: "A pool setting caused timeouts until rollback.",
  timeline: [{ entry: "Errors rose at 10:00.", fragment_id: "log-1", quote: "10:00 errors rose." }],
  root_cause: {
    status: "known",
    statement: "The pool setting caused timeouts.",
    fragment_id: "log-1",
    quote: "Investigation confirmed the pool setting caused timeouts.",
  },
  unknowns: [],
  action_items: [{ action: "Test the pool setting under load.", owner: "platform" }],
};
const run = (change = {}) =>
  finalizePostmortem({
    incident_id: "incident-1",
    incident_fragments: fragments,
    postmortem_draft: draft,
    fragments_digest: "sha256:" + "a".repeat(64),
    ...change,
  });
test("only known complete cause can propose publication; a hypothesis stays explicitly incomplete", () => {
  assert.equal(run().postmortem.decision, "publishable");
  const result = run({
    postmortem_draft: { ...draft, root_cause: { ...draft.root_cause, status: "suspected" } },
  });
  assert.equal(result.postmortem.decision, "needs_more_evidence");
  assert.equal(result.postmortem.publish_proposal, null);
  assert.equal(result.postmortem.publish_performed, false);
  assert.ok(result.document.includes("## Root cause (suspected)"));
});
test("wrong quotes and ambiguous source ids withhold the draft", () => {
  for (const change of [
    { incident_fragments: [...fragments, { ...fragments[0], text: "different source" }] },
    { postmortem_draft: { ...draft, timeline: [{ ...draft.timeline[0], quote: "invented" }] } },
  ]) {
    const result = run(change);
    assert.equal(result.postmortem.decision, "refused");
    assert.deepEqual(result.postmortem.timeline, []);
    assert.ok(result.document.includes("Draft withheld"));
  }
});
test("the document preserves exact findings, source attribution and useful owned actions", () => {
  const result = run();
  assert.ok(result.document.includes("\n\n## Summary\n\n"));
  assert.ok(result.document.includes("> 10:00 errors rose."));
  assert.ok(result.document.includes("platform: Test the pool setting under load."));
  assert.ok(result.document.includes("Publication: not performed."));
  const unknown = run({
    postmortem_draft: {
      ...draft,
      root_cause: { status: "unknown" },
      unknowns: ["Why did the setting change?"],
    },
  });
  assert.equal(unknown.postmortem.decision, "needs_more_evidence");
  assert.ok(unknown.document.includes("Not established."));
});
