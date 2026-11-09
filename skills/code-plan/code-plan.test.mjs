import test from "node:test";
import assert from "node:assert/strict";
import validate from "./code-plan.mjs";
import fs from "node:fs";
import YAML from "../../packages/contracts/node_modules/yaml/dist/index.js";
const fixture = YAML.parse(
  fs.readFileSync(new URL("./run.yaml", import.meta.url), "utf8"),
).test.cases.find((c) => c.name === "plans-intake-without-model-approval");
const inputs = () => ({
  ...structuredClone(fixture.inputs),
  catalog_skills: [{ name: "github-issue-to-pr" }],
  work_plan_draft: structuredClone(
    fixture.caller.answers.draft.work_plan_draft,
  ),
});
test("planning preserves an upstream source without invented authority", () => {
  const actual = validate(inputs());
  assert.equal(actual.decision, "ready", JSON.stringify(actual.validation));
  assert.equal(actual.evidence.source_change_set_status, "preserved");
});
test("removing false authority checks retains source conservation", () => {
  const i = inputs();
  i.work_plan_draft.change_set.shared_invariants.push("invented");
  const actual = validate(i);
  assert.equal(actual.decision, "blocked");
  assert.ok(
    actual.validation.findings.some((f) => f.code === "change_set.drift"),
  );
});
test("legacy approval-like source fields never grant or deny planning authority", () => {
  const i = inputs();
  for (const v of [i.change_set, i.work_plan_draft.change_set]) {
    v.commence_decision = "hold";
    v.action_decision = "request_review";
  }
  const actual = validate(i);
  assert.equal(actual.decision, "ready", JSON.stringify(actual.validation));
  assert.deepEqual(actual.change_set, i.change_set);
});
