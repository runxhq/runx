export function admitContact({
  sender, prospect, purpose, offer, required_footer, contact_policy, qualification, evidence_context,
}) {
  const blockers = [];
  if (qualification) {
    if (qualification.decision !== "qualified") blockers.push("lead qualification does not support outreach");
    if (!prospect.ref || qualification.subject.ref !== prospect.ref) {
      blockers.push("lead qualification belongs to a different or unbound prospect");
    }
  }
  if (contact_policy.suppressed) blockers.push("recipient is suppressed");
  if (contact_policy.recipient !== prospect.email) blockers.push("contact policy belongs to a different recipient");
  if (contact_policy.status !== "eligible") blockers.push(`contact eligibility is ${contact_policy.status}`);
  if (!contact_policy.policy_ref.trim() || contact_policy.evidence_refs.length === 0) {
    blockers.push("contact eligibility has no policy evidence");
  }
  if (!required_footer.trim()) blockers.push("required identity/opt-out footer is empty");
  blockers.push(...evidence_context.blockers);
  const eligible = blockers.length === 0 && evidence_context.decision === "ready";
  return {
    contact_brief: {
      decision: eligible ? "eligible_for_draft" : "hold",
      sender,
      prospect,
      purpose,
      offer,
      required_footer,
      policy_ref: contact_policy.policy_ref,
      policy_evidence_refs: contact_policy.evidence_refs,
      blockers,
    },
  };
}

export function finalizeReview({ contact_brief: brief, content_draft_packet: draft, draft_packet_digest }) {
  const blockers = [...brief.blockers];
  if (brief.decision === "eligible_for_draft") {
    if (draft?.decision !== "ready" || draft?.validation?.status !== "pass") {
      blockers.push("a validated, supported draft is required");
    } else if (typeof draft.draft?.title !== "string" || typeof draft.draft?.body !== "string"
        || /[\r\n]/u.test(draft.draft.title)) {
      blockers.push("a single-line subject and plain-text body are required");
    } else if (!draft.draft.body.includes(brief.required_footer)) {
      blockers.push("the exact required footer is missing");
    }
  }
  const ready = blockers.length === 0;
  return {
    outreach_review: {
      decision: ready ? "ready_for_review" : "hold",
      sender: brief.sender,
      recipient: brief.prospect.email,
      policy_ref: brief.policy_ref,
      policy_evidence_refs: brief.policy_evidence_refs,
      draft_status: ready ? "prepared" : "withheld",
      draft: ready ? { subject: draft.draft.title, body: draft.draft.body } : null,
      draft_packet_digest: ready ? draft_packet_digest : null,
      delivery_status: "not_sent",
      blockers,
    },
  };
}
