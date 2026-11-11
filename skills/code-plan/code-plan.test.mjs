import test from "node:test";
import assert from "node:assert/strict";
import validate from "./code-plan.mjs";
import fs from "node:fs";
import YAML from "../../packages/contracts/node_modules/yaml/dist/index.js";
const fixture = YAML.parse(
  fs.readFileSync(
    new URL("./tests/plans-intake-without-model-approval.yaml", import.meta.url),
    "utf8",
  ),
);
const inputs = () => ({
  ...structuredClone(fixture.inputs),
  catalog_skills: [{ name: "github-issue-to-pr" }],
  work_plan_draft: structuredClone(fixture.host.answers.draft.work_plan_draft),
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
  assert.deepEqual(actual.change_set, i.change_set);
  assert.equal(actual.evidence.source_change_set_status, "drifted");
});
test("a malformed draft retains the supplied intake and its evidence", () => {
  const i = inputs();
  i.work_plan_draft.workspace_change_plan.phases[0].id = "invalid_id";
  const actual = validate(i);
  assert.equal(actual.decision, "blocked");
  assert.deepEqual(actual.workspace_change_plan, {});
  assert.deepEqual(actual.change_set, i.change_set);
  assert.equal(actual.evidence.source_context_available, true);
  assert.equal(actual.evidence.source_change_set_status, "preserved");
  assert.equal(actual.evidence.source_thread_locator_preserved, true);
  assert.ok(actual.validation.findings.some((f) => f.code === "draft.invalid_shape"));
});
test("catalog identity retains the publisher in matching and output", () => {
  const i = inputs();
  i.catalog_skills = [{ name: "runx/github-issue-to-pr" }];
  for (const skill of i.work_plan_draft.required_skills) skill.name = "runx/github-issue-to-pr";
  for (const step of i.work_plan_draft.orchestration_steps) step.skill = "runx/github-issue-to-pr@1.2.3#work";
  const actual = validate(i);
  assert.equal(actual.decision, "ready", JSON.stringify(actual.validation));
  assert.equal(actual.required_skills[0].name, "runx/github-issue-to-pr");
  i.catalog_skills = [{ name: "other/github-issue-to-pr" }];
  const mismatched = validate(i);
  assert.equal(mismatched.decision, "blocked");
  assert.ok(mismatched.validation.findings.some((f) => f.code === "catalog.unknown_skill"));
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
