import assert from "node:assert/strict";
import test from "node:test";
import { admitContact, finalizeReview } from "./cold-outreach.mjs";

function inputs() {
  return {
    sender: { name: "Example team", from_address: "hello@example.com", reply_to: "hello@example.com" },
    prospect: { email: "operator@example.org", company: "Example" },
    purpose: "Offer a useful documentation review", offer: "A bounded review of public docs",
    required_footer: "Example team. Reply to opt out.",
    contact_policy: { recipient: "operator@example.org", status: "eligible", suppressed: false, policy_id: "policy:outreach", evidence_ids: ["decision:recipient-1"] },
    evidence_context: { decision: "ready", blockers: [] },
  };
}

test("admission returns the structured brief the draft is written from, with no assembled prose", () => {
  const value = inputs();
  const { contact_brief } = admitContact(value);
  assert.equal(contact_brief.decision, "eligible_for_draft");
  assert.deepEqual(
    { sender: contact_brief.sender, prospect: contact_brief.prospect, purpose: contact_brief.purpose, offer: contact_brief.offer, required_footer: contact_brief.required_footer },
    { sender: value.sender, prospect: value.prospect, purpose: value.purpose, offer: value.offer, required_footer: value.required_footer },
  );
  assert.deepEqual(Object.keys(contact_brief).sort(), [
    "blockers", "decision", "offer", "policy_evidence_ids", "policy_id", "prospect", "purpose", "required_footer", "sender",
  ]);
});

test("holds suppression, unknown eligibility, unbound policy and a different policy recipient", () => {
  const changes = [
    (value) => { value.contact_policy.suppressed = true; },
    (value) => { value.contact_policy.status = "unknown"; },
    (value) => { value.contact_policy.evidence_ids = []; },
    (value) => { value.contact_policy.recipient = "other@example.org"; },
  ];
  for (const change of changes) {
    const value = inputs(); change(value);
    const { contact_brief } = admitContact(value);
    const { outreach_review } = finalizeReview({ contact_brief });
    assert.equal(outreach_review.decision, "hold");
    assert.equal(outreach_review.draft_status, "withheld");
    assert.equal(outreach_review.delivery_status, "not_sent");
  }
});

test("withheld evidence or writing context holds an otherwise eligible contact with its reasons", () => {
  const value = inputs();
  value.evidence_context = { decision: "needs_more_evidence", blockers: ["brand_voice context does not apply to the requested audience"] };
  const { contact_brief } = admitContact(value);
  assert.equal(contact_brief.decision, "hold");
  const { outreach_review } = finalizeReview({ contact_brief });
  assert.equal(outreach_review.decision, "hold");
  assert.equal(outreach_review.draft, null);
  assert.deepEqual(outreach_review.blockers, ["brand_voice context does not apply to the requested audience"]);
});

test("requires a supported draft and the exact footer before review readiness", () => {
  const { contact_brief } = admitContact(inputs());
  const valid = { decision: "ready", validation: { status: "pass" }, draft: { title: "A useful review", body: "Useful offer.\nExample team. Reply to opt out." } };
  const draft_packet_digest = `sha256:${"a".repeat(64)}`;
  const review = (content_draft_packet) => finalizeReview({ contact_brief, content_draft_packet, draft_packet_digest }).outreach_review;
  assert.equal(review(valid).decision, "ready_for_review");
  assert.equal(review({ ...valid, validation: { status: "fail" } }).decision, "hold");
  assert.equal(review({ ...valid, draft: { body: "Useful offer." } }).decision, "hold");
  assert.equal(review(valid).delivery_status, "not_sent");
  assert.deepEqual(review(valid).sender, inputs().sender);
  assert.equal(review(valid).draft.subject, valid.draft.title);
  assert.equal(review({ ...valid, draft: { body: "Useful offer." } }).draft, null);
  assert.equal(review({ ...valid, draft: { title: "Subject\nBcc: other@example.org", body: valid.draft.body } }).draft, null);
  assert.deepEqual(review({ ...valid, draft: { title: "A useful review", body: "Useful offer." } }).blockers, ["the exact required footer is missing"]);
});
