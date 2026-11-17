import assert from "node:assert/strict";
import test from "node:test";
import { admitContact, finalizeReview, sequenceBlockers } from "./cold-outreach.mjs";

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
    "blockers", "decision", "follows", "offer", "policy_evidence_ids", "policy_id", "prospect", "purpose", "required_footer", "sender",
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

test("memory is searched for the recipient only for a draft, and only at the levels the caller names", () => {
  const levels = [{ level: "personal" }];
  assert.deepEqual(admitContact({ ...inputs(), memory: levels }).memory_search, {
    path: "search",
    levels,
    query: "operator@example.org",
  });
  assert.deepEqual(admitContact(inputs()).memory_search, { path: "none" });
  const held = { ...inputs(), memory: levels };
  held.contact_policy = { ...held.contact_policy, suppressed: true };
  assert.deepEqual(admitContact(held).memory_search, { path: "none" });
});

test("a follow-up holds past the limit or after a reply, opt-out, complaint or bounce, and never repeats", () => {
  const earlier = [{ subject: "A useful review", body: "Useful offer.\nExample team. Reply to opt out.", sent_at: "2026-11-17T00:00:00Z" }];
  const follows = (since, number = 1, limit = 2) => ({ earlier, since, number, limit });
  const at = (kind) => ({ kind, received_at: "2026-11-18T00:00:00Z" });
  assert.deepEqual(sequenceBlockers(follows([at("auto_reply")])), [], "an away message is no answer");
  assert.deepEqual(sequenceBlockers(follows([], 3)), ["follow-up 3 is past the 2 the user allows"]);
  assert.deepEqual(sequenceBlockers(follows([at("reply")])), ["a reply ends the cold sequence"]);
  assert.deepEqual(sequenceBlockers(follows([at("unsubscribed"), at("bounce")])), ["an opt-out stops the sequence", "an invalid address stops the sequence"]);
  const value = { ...inputs(), follows: follows([]) };
  const { contact_brief } = admitContact(value);
  assert.equal(contact_brief.decision, "eligible_for_draft");
  // A sentence the sequence already said, word for word, withholds it; a
  // greeting or the footer doesn't.
  const said = "We will review one release pipeline for free and show you where the risk hides.";
  const first = { subject: "A useful review", body: `Hi Ana,\n\n${said}\n\nExample team. Reply to opt out.`, sent_at: "2026-11-17T00:00:00Z" };
  const brief = admitContact({ ...inputs(), follows: { earlier: [first], since: [], number: 1, limit: 2 } }).contact_brief;
  const review = (body) => finalizeReview({ contact_brief: brief, content_draft_packet: { decision: "ready", validation: { status: "pass" }, draft: { title: "A useful review", body } }, draft_packet_digest: `sha256:${"a".repeat(64)}` }).outreach_review;
  assert.deepEqual(review(`Hi Ana,\n\nOne more thing.  ${said}\n\nExample team. Reply to opt out.`).blockers, [`a follow-up says what the sequence hasn't, and this repeats: "${said}"`]);
  assert.equal(review("Hi Ana,\n\nTeams that ship weekly usually find their rollback steps were never rehearsed.\n\nExample team. Reply to opt out.").decision, "ready_for_review");
});
