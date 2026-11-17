/** What came back that ends a cold sequence, and why. */
const ENDS = {
  reply: "a reply ends the cold sequence",
  bounce: "an invalid address stops the sequence",
  complaint: "a complaint stops the sequence",
  unsubscribed: "an opt-out stops the sequence",
};

/**
 * Why a follow-up to `follows` can't be sent: past the user's limit, or a
 * reply, opt-out, complaint or bounce since the first email. An away message
 * is no answer, so it neither ends nor stops the sequence.
 */
export function sequenceBlockers(follows) {
  if (!follows) return [];
  const blockers = [];
  if (follows.number > follows.limit) {
    blockers.push(`follow-up ${follows.number} is past the ${follows.limit} the user allows`);
  }
  const first = Date.parse(follows.earlier[0].sent_at);
  for (const event of follows.since.filter((answer) => Date.parse(answer.received_at) >= first)) {
    if (ENDS[event.kind]) blockers.push(ENDS[event.kind]);
  }
  return [...new Set(blockers)];
}

/** The fewest words a sentence has to carry something a reader would notice repeated. */
const SUBSTANTIVE_WORDS = 8;

/** A body's sentences, the footer left out, each with its words as compared. */
function sentences(body, footer) {
  return body
    .replace(footer, "")
    .split(/(?<=[.!?])\s+|\n+/u)
    .map((sentence) => sentence.trim())
    .filter((sentence) => sentence.split(/\s+/u).length >= SUBSTANTIVE_WORDS)
    .map((sentence) => ({ sentence, compared: sentence.toLowerCase().replace(/\s+/gu, " ") }));
}

/**
 * The first sentence of `body` an earlier email of the sequence already
 * said, word for word: a follow-up says something the sequence hasn't.
 */
export function repeated(body, earlier, footer) {
  const said = earlier.map((email) => email.body.toLowerCase().replace(/\s+/gu, " "));
  return sentences(body, footer).find(({ compared }) => said.some((text) => text.includes(compared)))?.sentence ?? null;
}

export function admitContact({
  sender, prospect, purpose, offer, required_footer, contact_policy, qualification, evidence_context, memory, follows,
}) {
  const blockers = [];
  if (qualification) {
    if (qualification.decision !== "qualified") blockers.push("lead qualification does not support outreach");
    if (!prospect.id || qualification.subject.id !== prospect.id) {
      blockers.push("lead qualification belongs to a different or unbound prospect");
    }
  }
  if (contact_policy.suppressed) blockers.push("recipient is suppressed");
  if (contact_policy.recipient !== prospect.email) blockers.push("contact policy belongs to a different recipient");
  if (contact_policy.status !== "eligible") blockers.push(`contact eligibility is ${contact_policy.status}`);
  if (!contact_policy.policy_id.trim() || contact_policy.evidence_ids.length === 0) {
    blockers.push("contact eligibility has no policy evidence");
  }
  if (!required_footer.trim()) blockers.push("required identity/opt-out footer is empty");
  blockers.push(...sequenceBlockers(follows));
  blockers.push(...evidence_context.blockers);
  const eligible = blockers.length === 0 && evidence_context.decision === "ready";
  return {
    // Memory is searched for the recipient only for a draft, and only at
    // the levels the caller names.
    memory_search: eligible && memory
      ? { path: "search", levels: memory, query: prospect.email }
      : { path: "none" },
    contact_brief: {
      decision: eligible ? "eligible_for_draft" : "hold",
      sender,
      prospect,
      purpose,
      offer,
      required_footer,
      policy_id: contact_policy.policy_id,
      policy_evidence_ids: contact_policy.evidence_ids,
      blockers,
      follows: follows ?? null,
    },
  };
}

export function finalizeReview({ contact_brief: brief, content_draft_packet: draft, draft_packet_digest }) {
  const blockers = [...brief.blockers];
  if (brief.decision === "eligible_for_draft") {
    if (draft?.decision !== "ready" || draft?.validation?.status !== "pass") {
      // The person is told why: the writer's and the verifier's own reasons.
      const why = [
        ...(Array.isArray(draft?.review_checklist) ? draft.review_checklist : []),
        ...(Array.isArray(draft?.validation?.findings) ? draft.validation.findings.map((finding) => finding?.message) : []),
      ].filter((reason) => typeof reason === "string" && reason.trim());
      blockers.push(why.length > 0 ? `the draft was withheld: ${why.join("; ")}` : "a validated, supported draft is required");
    } else if (typeof draft.draft?.title !== "string" || typeof draft.draft?.body !== "string"
        || /[\r\n]/u.test(draft.draft.title)) {
      blockers.push("a single-line subject and plain-text body are required");
    } else if (!draft.draft.body.includes(brief.required_footer)) {
      blockers.push("the exact required footer is missing");
    } else if (brief.follows) {
      const again = repeated(draft.draft.body, brief.follows.earlier, brief.required_footer);
      if (again) blockers.push(`a follow-up says what the sequence hasn't, and this repeats: "${again}"`);
    }
  }
  const ready = blockers.length === 0;
  return {
    outreach_review: {
      decision: ready ? "ready_for_review" : "hold",
      sender: brief.sender,
      recipient: brief.prospect.email,
      policy_id: brief.policy_id,
      policy_evidence_ids: brief.policy_evidence_ids,
      draft_status: ready ? "prepared" : "withheld",
      draft: ready ? { subject: draft.draft.title, body: draft.draft.body } : null,
      draft_packet_digest: ready ? draft_packet_digest : null,
      delivery_status: "not_sent",
      blockers,
    },
  };
}
