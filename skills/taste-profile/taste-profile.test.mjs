import assert from "node:assert/strict";
import { test } from "node:test";
import { finalizeProfile, indexEvidence } from "./taste-profile.mjs";

const collections = ["principles", "likes", "dislikes", "decision_rules", "examples_to_emulate", "examples_to_avoid"];
function inputs() {
  return {
    subject: "maintainer",
    ...indexEvidence({ subject: "maintainer", evidence: [
      { kind: "liked", content: "Show the workflow." },
      { kind: "disliked", content: "Hide the workflow." },
    ] }),
    evidence_digest: `sha256:${"1".repeat(64)}`,
    taste_profile_draft: {
      decision: "ready", subject: "maintainer",
      applicability: { surfaces: [], audience: "", expires_when: [] },
      taste_profile: Object.fromEntries(collections.map((name) => [name, []])),
      redactions: [], stop_conditions: [],
    },
  };
}
const rule = (value, sourceIds = ["source:1"], confidence = "inferred") => ({ rule: value, source_ids: sourceIds, confidence });
const finish = (value) => finalizeProfile(value).taste_profile_packet;

test("all rule collections preserve exact text, sources and confidence in public context", () => {
  const value = inputs();
  for (const name of collections) value.taste_profile_draft.taste_profile[name] = [rule(` ${name} `, ["source:2", "source:1"])];
  const packet = finish(value);
  assert.equal(packet.decision, "ready");
  for (const name of collections) assert.deepEqual(packet.taste_profile[name], [` ${name} `]);
  assert.deepEqual(packet.evidence.bindings, collections.map((name) => rule(` ${name} `, ["source:2", "source:1"])));
});

test("unknown or missing evidence on examples holds the whole profile", () => {
  for (const bad of [rule("Hide context", ["source:unknown"]), { rule: "Hide context" }, "Hide context"]) {
    const value = inputs();
    value.taste_profile_draft.taste_profile.principles = [rule("Show the workflow")];
    value.taste_profile_draft.taste_profile.examples_to_avoid = [bad];
    const packet = finish(value);
    assert.equal(packet.decision, "needs_more_evidence");
    assert.deepEqual(packet.evidence.bindings, []);
  }
});

test("equivalent repeated bindings coalesce without losing either collection", () => {
  const value = inputs();
  value.taste_profile_draft.taste_profile.principles = [rule("Visible workflow", ["source:1", "source:2"])];
  value.taste_profile_draft.taste_profile.likes = [rule("Visible workflow", ["source:2", "source:1"])];
  const packet = finish(value);
  assert.equal(packet.decision, "ready");
  assert.equal(packet.evidence.bindings.length, 1);
  assert.deepEqual(packet.taste_profile.likes, ["Visible workflow"]);
});

test("conflicting repeated sources or confidence cannot overwrite an earlier binding", () => {
  for (const changed of [rule("Visible workflow", ["source:2"]), rule("Visible workflow", ["source:1"], "explicit")]) {
    const value = inputs();
    value.taste_profile_draft.taste_profile.principles = [rule("Visible workflow")];
    value.taste_profile_draft.taste_profile.likes = [changed];
    const packet = finish(value);
    assert.equal(packet.decision, "needs_more_evidence");
    assert.ok(packet.validation.findings.some((finding) => finding.code === "taste_profile.binding.conflict"));
  }
});

test("an empty ready profile cannot release useful-context status", () => {
  assert.equal(finish(inputs()).decision, "needs_more_evidence");
});

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
