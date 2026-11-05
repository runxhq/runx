import assert from "node:assert/strict";
import { test } from "node:test";
import { indexEvidence } from "./taste-profile.mjs";
test("the declared character budget admits Unicode examples without treating each surrogate as a character", () => {
  const content = "A clear source example. " + "🟠".repeat(11000);
  const result = indexEvidence({
    subject: "runx product copy",
    evidence: [
      { kind: "liked", content },
      { kind: "instruction", content: "Use exact action names." },
    ],
  }).evidence_index;
  assert.equal(result.decision, "ready");
  assert.equal(result.sources[0].content, content);
});
test("a context remains held when its distinguishing source requirement is missing", () => {
  const result = indexEvidence({
    subject: "runx product copy",
    evidence: [{ kind: "liked", content: "One isolated example." }],
  }).evidence_index;
  assert.equal(result.decision, "needs_more_evidence");
});
