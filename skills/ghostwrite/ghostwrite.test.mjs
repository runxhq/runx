import assert from "node:assert/strict";
import test from "node:test";
import { prepareContent, prepareEvidence, prepareHandoff } from "./ghostwrite.mjs";

const digest = `sha256:${"a".repeat(64)}`;
const evidence_pack = { decision: "ready", validation: { status: "pass" }, source_evidence: [{ evidence_digest: digest }], evidence_log: [{ source_digest: digest }] };
const brandContext = (applicability) => ({
  decision: "ready", validation: { status: "pass" }, applicability,
  brand_voice: { vocabulary: { use: ["proof"], avoid: ["magic"] }, cadence: ["short"], claim_rules: { forbidden: ["guaranteed outcomes"] }, channel_adjustments: [{ channel: "email", rules: ["one ask"] }] },
  evidence: { bindings: [{ rule: "one ask", confidence: "explicit" }], sources: [{ content: "source material must not be copied into the admitted profile" }] },
  stop_conditions: ["unsupported claims"],
});
const tasteContext = (applicability) => ({
  decision: "ready", validation: { status: "pass" }, applicability,
  taste_profile: { dislikes: ["generic compliments"], examples_to_emulate: ["specific useful advice"] },
  evidence: { bindings: [{ rule: "specific useful advice" }] },
});
const prepare = (contexts) => prepareEvidence({
  objective: "Offer a useful review", audience: "operators", channel: "email", evidence_pack,
  evidence_packet_digest: digest, brand_context_digest: digest, taste_context_digest: digest, ...contexts,
}).evidence_context;

test("writing admission preserves structured brand and taste guidance and its scope", () => {
  const brand_context = brandContext({ channels: ["email"], audience: "operators", boundaries: ["public facts only"] });
  const taste_context = tasteContext({ surfaces: ["email"], audience: "operators", expires_when: ["preferences change"] });
  const result = prepare({ brand_context, taste_context });
  assert.equal(result.decision, "ready");
  assert.deepEqual(result.writing_contexts[0].profile, brand_context.brand_voice);
  assert.deepEqual(result.writing_contexts[0].applicability, brand_context.applicability);
  assert.deepEqual(result.writing_contexts[0].evidence_bindings, brand_context.evidence.bindings);
  assert.deepEqual(result.writing_contexts[1].profile, taste_context.taste_profile);
  assert.equal(result.writing_contexts[0].sources, undefined);
  assert.equal(result.context_requirements[0].profile, undefined);
  assert.equal(result.context_requirements[0].applicability, undefined);
  assert.equal(result.context_requirements[0].packet_digest, digest);
  assert.deepEqual(result.context_requirements.map(({ type, scope }) => ({ type, scope })), [
    { type: "brand_voice", scope: { audience: "operators", channel: "email" } },
    { type: "taste_profile", scope: { audience: "operators", channel: "email" } },
  ]);
});

test("a writing context scoped to another audience and channel is not admitted", () => {
  const result = prepare({ brand_context: brandContext({ channels: ["reddit"], audience: "engineers", boundaries: [] }) });
  assert.equal(result.decision, "needs_more_evidence");
  assert.deepEqual(result.blockers, [
    "brand_voice context does not apply to the requested audience",
    "brand_voice context does not apply to the requested channel",
  ]);
  assert.deepEqual(result.writing_contexts, []);
  assert.deepEqual(result.context_requirements, []);
});

test("only the context whose declared scope matches is admitted and bound", () => {
  const result = prepare({
    brand_context: brandContext({ channels: ["email"], audience: "operators", boundaries: [] }),
    taste_context: tasteContext({ surfaces: ["reddit"], audience: "operators", expires_when: [] }),
  });
  assert.equal(result.decision, "needs_more_evidence");
  assert.deepEqual(result.blockers, ["taste_profile context does not apply to the requested channel"]);
  assert.deepEqual(result.context_requirements.map(({ type, scope }) => ({ type, scope })), [
    { type: "brand_voice", scope: { audience: "operators", channel: "email" } },
  ]);
});

test("scope is compared exactly and an undeclared scope applies nowhere", () => {
  for (const applicability of [undefined, { channels: [], audience: "", boundaries: [] }, { channels: ["Email"], audience: "Operators", boundaries: [] }]) {
    const result = prepare({ brand_context: brandContext(applicability) });
    assert.equal(result.decision, "needs_more_evidence");
    assert.deepEqual(result.blockers, [
      "brand_voice context does not apply to the requested audience",
      "brand_voice context does not apply to the requested channel",
    ]);
    assert.deepEqual(result.context_requirements, []);
  }
  const taste = prepare({ taste_context: tasteContext({ channels: ["email"], audience: "operators", expires_when: [] }) });
  assert.deepEqual(taste.blockers, ["taste_profile context does not apply to the requested channel"]);
});

const boundDraft = (overrides = {}) => ({
  decision: "ready",
  validation: { status: "pass", findings: [] },
  draft: { title: "Evidence-bound execution", body: "Receipts bind actions.", claims: [{ claim: "Receipts bind actions.", source_digests: [digest] }] },
  evidence_binding: { evidence_packet_digest: digest, source_digests: [digest] },
  ...overrides,
});

test("packaging holds a draft that is ready only by its own say", () => {
  assert.equal(prepareContent({ channel: "blog", draft: boundDraft() }).publish_draft.decision, "ready_for_handoff");
  // Self-declared ready and passing, with no evidence binding.
  const unbound = prepareContent({ channel: "blog", draft: boundDraft({ evidence_binding: {} }) });
  assert.equal(unbound.publish_draft.decision, "needs_input");
  assert.equal(unbound.digest_input, "");
  // A claim citing a source the draft is not bound to.
  const stray = prepareContent({
    channel: "blog",
    draft: boundDraft({ draft: { title: "t", body: "b", claims: [{ claim: "c", source_digests: [`sha256:${"b".repeat(64)}`] }] } }),
  });
  assert.equal(stray.publish_draft.decision, "needs_input");
});

test("a handoff binds its packet's digest and channel, and states only what it knows", () => {
  const packet = { decision: "ready_for_handoff", channel: "email", payload_digest: digest, delivery_status: "not_sent" };
  const target = { thread_locator: "github://acme/app/issues/7" };
  const ready = prepareHandoff({ channel: "email", boundary_kind: "external", packet, target });
  assert.equal(ready.handoff_packet.decision, "handoff_ready");
  assert.deepEqual(ready.boundary_state, {
    boundary_kind: "external",
    completion_state: "handoff_ready",
    next_actor: "governed_provider_adapter",
    approval_required_for_delivery: true,
    ack_expected: true,
  });
  for (const unbound of [
    { ...packet, payload_digest: "" },
    { ...packet, channel: "blog" },
  ]) {
    assert.equal(prepareHandoff({ channel: "email", boundary_kind: "external", packet: unbound, target }).handoff_packet.decision, "needs_input");
  }
  assert.equal(prepareHandoff({ channel: "email", boundary_kind: "internal", packet, target }).boundary_state.approval_required_for_delivery, false);
});
