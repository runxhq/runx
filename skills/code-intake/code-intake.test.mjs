import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import YAML from "../../packages/contracts/node_modules/yaml/dist/index.js";
import finalize from "./code-intake.mjs";
const cases = YAML.parse(
  fs.readFileSync(new URL("./run.yaml", import.meta.url), "utf8"),
).harness.cases;
const input = (name) => {
  const c = cases.find((c) => c.name === name);
  return {
    ...structuredClone(c.inputs),
    source_context: structuredClone(c.inputs.source_context || {}),
    intake_draft: structuredClone(c.caller.answers.intake.intake_draft),
    input_digest: "sha256:" + "a".repeat(64),
  };
};
test("plain request yields a bounded ordinary host recommendation", () => {
  const p = finalize(input("ordinary-docs-request")).request_packet;
  assert.equal(p.status, "ready");
  assert.equal(p.intake_report.recommended_lane, "host");
  assert.equal(p.execution.authority, "none_granted");
  assert.equal(p.change_set.change_set_id, p.input_digest);
  assert.equal(p.signal, undefined);
});
test("adapter evidence and provider identity stay outside model output", () => {
  const i = input("preserves-adapter-context");
  i.intake_draft.source_context = { issue_locator: "invented" };
  i.intake_draft.signal = { id: "invented" };
  const p = finalize(i).request_packet;
  assert.deepEqual(p.source_context, i.source_context);
  assert.equal(p.change_set.thread_locator, i.source_context.thread_locator);
  assert.equal(p.signal, undefined);
});
test("unknown issue and real questions retain work without an implementation route", () => {
  for (const name of [
    "does-not-invent-issue-identity",
    "retains-blocking-question",
  ]) {
    const p = finalize(input(name)).request_packet;
    assert.equal(p.status, "needs_context");
    assert.equal(p.intake_report.recommended_lane, "manual-review");
    assert.ok(p.intake_report.open_questions.length);
    assert.ok(p.intake_report.summary);
  }
});
test("invalid actionable drafts fail before a misleading ready packet", () => {
  const i = input("ordinary-docs-request");
  i.intake_draft.success_criteria = [];
  assert.throws(() => finalize(i), /success criteria/);
  i.intake_draft.success_criteria = ["actual criterion"];
  i.input_digest = "model identity";
  assert.throws(() => finalize(i), /Native input identity/);
});
test("intake does not mutate caller evidence or the agent draft", () => {
  const i = input("does-not-invent-issue-identity"),
    before = structuredClone(i);
  finalize(i);
  assert.deepEqual(i, before);
});
