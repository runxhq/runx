import assert from "node:assert/strict";
import test from "node:test";
import {
  prepareContent,
  prepareEvidence,
  prepareHandoff,
  inventedNumbers,
  reviewContextUse,
} from "./ghostwrite.mjs";
const digest = "sha256:" + "a".repeat(64);
const tasteDigest = "sha256:" + "b".repeat(64);
const evidence_pack = {
  decision: "ready",
  validation: { status: "pass" },
  source_evidence: [{ evidence_digest: digest }],
  evidence_log: [{ source_digest: digest }],
};
const brandContext = (applicability) => ({
  decision: "ready",
  validation: { status: "pass" },
  applicability,
  brand_voice: {
    vocabulary: { use: ["proof"], avoid: ["magic"] },
    cadence: ["short"],
    claim_rules: { forbidden: ["guaranteed outcomes"] },
  },
  evidence: {
    bindings: [{ rule: "one ask" }],
    sources: [{ content: "raw evidence stays upstream" }],
  },
  stop_conditions: ["unsupported claims"],
});
const tasteContext = (applicability) => ({
  decision: "ready",
  validation: { status: "pass" },
  applicability,
  taste_profile: { dislikes: ["generic compliments"] },
  evidence: { bindings: [{ rule: "specific useful advice" }] },
});
const prepare = (contexts) =>
  prepareEvidence({
    objective: "Offer a useful review",
    audience: "operators",
    channel: "email",
    evidence_pack,
    evidence_kind: "research_packet",
    evidence_packet_digest: digest,
    brand_context_digest: digest,
    taste_context_digest: tasteDigest,
    ...contexts,
  }).evidence_context;
const binding = (
  packet_digest,
  decision = "applies",
  reason = "The selected audience and surface are the same operator email task described by this profile.",
) => ({
  packet_digest,
  applied_rules: ["one ask"],
  scope_review: { decision, reason },
});

test("admission preserves source applicability and target scope without treating prose as identity", () => {
  const brand = brandContext({
    channels: ["Email"],
    audience: "Operators",
    boundaries: ["public facts only"],
  });
  const ctx = prepare({ brand_context: brand });
  assert.equal(ctx.decision, "ready");
  assert.deepEqual(ctx.writing_contexts[0].profile, brand.brand_voice);
  assert.deepEqual(
    ctx.context_requirements[0].applicability,
    brand.applicability,
  );
  assert.deepEqual(ctx.context_requirements[0].scope, {
    audience: "operators",
    channel: "email",
  });
  assert.equal(ctx.writing_contexts[0].sources, undefined);
  const result = reviewContextUse({
    evidence_context: ctx,
    content_draft: {
      decision: "ready",
      context_binding: { applied: [binding(digest)] },
    },
  });
  assert.equal(result.reviewed_draft.decision, "ready");
});

test("an out-of-scope or uncertain writer judgment withholds and preserves the actual reason", () => {
  const ctx = prepare({
    brand_context: brandContext({
      channels: ["reddit"],
      audience: "engineers",
      boundaries: [],
    }),
  });
  assert.equal(ctx.decision, "ready");
  for (const decision of ["out_of_scope", "uncertain"]) {
    const result = reviewContextUse({
      evidence_context: ctx,
      content_draft: {
        decision: "ready",
        context_binding: {
          applied: [
            binding(
              digest,
              decision,
              "This source is scoped to an engineering Reddit discussion, not this buyer email.",
            ),
          ],
        },
      },
    });
    assert.equal(result.reviewed_draft.decision, "needs_more_evidence");
    assert.match(result.review_context.blockers[0], /engineering Reddit/);
  }
  // A writer that withheld wrote no draft: verification is given an empty
  // one, whole, and the writer's decision as why.
  const withheld = reviewContextUse({ evidence_context: ctx, content_draft: { decision: "needs_more_evidence" } });
  assert.deepEqual(withheld.reviewed_draft.draft, { title: "", body: "", claims: [] });
  assert.deepEqual(withheld.reviewed_draft.context_binding, { applied: [] });
  assert.match(withheld.review_context.blockers[0], /writer returned needs_more_evidence/);
});

test("every admitted context needs one explicit review, even when labels happen to match", () => {
  const ctx = prepare({
    brand_context: brandContext({
      channels: ["email"],
      audience: "operators",
      boundaries: [],
    }),
    taste_context: tasteContext({
      surfaces: ["email"],
      audience: "operators",
      expires_when: [],
    }),
  });
  for (const applied of [
    [],
    [binding(digest)],
    [binding(digest), binding(digest), binding(tasteDigest)],
  ]) {
    assert.equal(
      reviewContextUse({
        evidence_context: ctx,
        content_draft: { decision: "ready", context_binding: { applied } },
      }).reviewed_draft.decision,
      "needs_more_evidence",
    );
  }
  assert.equal(
    reviewContextUse({
      evidence_context: ctx,
      content_draft: {
        decision: "ready",
        context_binding: { applied: [binding(digest), binding(tasteDigest)] },
      },
    }).reviewed_draft.decision,
    "ready",
  );
});

test("missing declared source scope still blocks structural admission", () => {
  for (const applicability of [
    undefined,
    { channels: [], audience: "", boundaries: [] },
  ]) {
    const ctx = prepare({ brand_context: brandContext(applicability) });
    assert.equal(ctx.decision, "needs_more_evidence");
    assert.equal(ctx.evidence_assurance, "not_admitted");
    assert.match(ctx.blockers.join(" "), /no declared/);
  }
  const wrongField = prepare({
    taste_context: tasteContext({
      channels: ["email"],
      audience: "operators",
      expires_when: [],
    }),
  });
  assert.equal(wrongField.decision, "needs_more_evidence");
});

const boundDraft = (overrides = {}) => ({
  decision: "ready",
  validation: { status: "pass", findings: [] },
  draft: {
    title: "Evidence-bound execution",
    body: "Receipts bind actions.",
    claims: [{ claim: "Receipts bind actions.", source_digests: [digest] }],
  },
  evidence_binding: {
    evidence_packet_digest: digest,
    source_digests: [digest],
  },
  ...overrides,
});
test("packaging retains source checks and does not bypass a missing context review", () => {
  assert.equal(
    prepareContent({ channel: "blog", draft: boundDraft() }).publish_draft
      .decision,
    "ready_for_handoff",
  );
  assert.equal(
    prepareContent({
      channel: "blog",
      draft: boundDraft({ evidence_binding: {} }),
    }).publish_draft.decision,
    "needs_input",
  );
  const unreviewed = boundDraft({
    context_binding: {
      required: [{ type: "brand_voice", packet_digest: digest }],
      applied: [{ packet_digest: digest, applied_rules: ["one ask"] }],
    },
  });
  assert.equal(
    prepareContent({ channel: "blog", draft: unreviewed }).publish_draft
      .decision,
    "needs_input",
  );
  const stray = boundDraft({
    draft: {
      title: "t",
      body: "b",
      claims: [{ claim: "c", source_digests: [tasteDigest] }],
    },
  });
  assert.equal(
    prepareContent({ channel: "blog", draft: stray }).publish_draft.decision,
    "needs_input",
  );
});
test("handoff still binds exact digest and channel without granting external authority", () => {
  const packet = {
    decision: "ready_for_handoff",
    channel: "email",
    payload_digest: digest,
    delivery_status: "not_sent",
  };
  const target = { thread_locator: "github://acme/app/issues/7" };
  const ready = prepareHandoff({
    channel: "email",
    boundary_kind: "external",
    packet,
    target,
  });
  assert.equal(ready.handoff_packet.decision, "handoff_ready");
  assert.equal(ready.boundary_state.approval_required_for_delivery, true);
  for (const bad of [
    { ...packet, payload_digest: "" },
    { ...packet, channel: "blog" },
  ])
    assert.equal(
      prepareHandoff({
        channel: "email",
        boundary_kind: "external",
        packet: bad,
        target,
      }).handoff_packet.decision,
      "needs_input",
    );
  assert.equal(
    prepareHandoff({
      channel: "email",
      boundary_kind: "internal",
      packet,
      target,
    }).boundary_state.approval_required_for_delivery,
    false,
  );
});

test("a draft states only numbers something admitted holds", () => {
  const context = {
    objective: "Reply to Priya",
    audience: "engineering leads",
    evidence_log: [{ claim: 'Priya wrote: "a 15-minute call next Tuesday"' }, { claim: "Shipwright has reviewed 40 pipelines since 2024." }],
  };
  const draft = (body) => ({ title: "Re: review", body });
  assert.deepEqual(inventedNumbers(context, draft("A 15-minute call works; we've reviewed forty pipelines since 2024.")), []);
  assert.deepEqual(inventedNumbers(context, draft("Tuesday at 2 PM works, for 15 minutes.")), ["2 pm"]);
  // Text the draft carries unchanged, as a footer, is the caller's, not invented.
  assert.deepEqual(inventedNumbers(context, draft("Thanks.\n12 Salamanca Place"), ["12 Salamanca Place"]), []);
  // A number is held counting what it counts: a currency, a unit, a scale.
  assert.deepEqual(inventedNumbers(context, draft("Reviews start at $40, or 40 dollars.")), ["$40", "40 dollars"]);
  assert.deepEqual(inventedNumbers(context, draft("It takes two weeks.")), ["two weeks"]);
  const scaled = { evidence_log: [{ claim: "Two engineers ship a million deploys over an hour of one engineer's time." }] };
  assert.deepEqual(inventedNumbers(scaled, draft("Two million deploys.")), ["two million deploys"]);
  // "One" is a number where it measures, which "an hour" holds.
  assert.deepEqual(inventedNumbers(scaled, draft("One engineer, one hour, in one day.")), ["one day"]);
});
