// For each lead: enrich its supplied sources, qualify it against the
// criteria, and prepare its first contact. Every lead's outcome is
// collected, and a lead's failure or hold stays that lead's.
export default async function batch(ctx, inputs) {
  const { campaign, leads } = inputs;
  const settled = await Promise.allSettled(leads.map((lead) => outreach(ctx, lead, inputs)));
  return {
    outcomes: settled.map((outcome, index) => ({
      lead: leads[index].id,
      idempotency_key: sendKey(campaign, leads[index].prospect.email),
      ...(outcome.status === "fulfilled"
        ? { status: "fulfilled", outreach_review: outcome.value }
        : { status: "rejected", rejection: outcome.reason }),
    })),
  };
}

// The key a lead's send is made under: its campaign and its recipient,
// never its draft, so a revised batch's new draft is the same intent and a
// lead already sent is never sent again. An address is compared without
// case, since a mailbox written two ways is one recipient.
function sendKey(campaign, recipient) {
  return `cold-outreach/${campaign}/${recipient.toLowerCase()}`;
}

// A packet a call returns arrives in its envelope, and the next call takes
// its data.
async function outreach(ctx, lead, inputs) {
  let stage = "enrich";
  try {
    const { lead_enrichment_packet } = await ctx.skill("../lead-enrichment#enrich", {
      subject: lead.subject,
      sources: lead.sources,
      requested_fields: inputs.requested_fields,
      as_of: inputs.as_of,
    });
    stage = "qualify";
    const { lead_qualification } = await ctx.skill("../lead-qualify#qualify", {
      enrichment_packet: lead_enrichment_packet.data,
      criteria: inputs.criteria,
      ...eligibility(lead),
    });
    stage = "prepare";
    const { outreach_review } = await ctx.skill(".#prepare", {
      sender: inputs.sender,
      prospect: { ...lead.prospect, id: lead.subject.id },
      audience: inputs.audience,
      purpose: inputs.purpose,
      offer: inputs.offer,
      required_footer: inputs.required_footer,
      contact_policy: lead.contact_policy,
      qualification: lead_qualification.data,
      ...(lead.facts === undefined ? {} : { facts: lead.facts }),
    });
    return outreach_review;
  } catch (rejected) {
    throw { stage, reason: rejected.reason, code: rejected.code, message: rejected.message };
  }
}

// The lead's contact policy as qualification takes it, when the policy
// names its evidence; with none, the lead is qualified on fit alone and
// holds for its contact policy.
function eligibility({ subject, contact_policy: policy }) {
  if (!policy.policy_id.trim() || policy.evidence_ids.length === 0) return {};
  return {
    contact_eligibility: {
      subject,
      status: policy.status,
      policy_id: policy.policy_id,
      evidence_ids: policy.evidence_ids,
    },
  };
}
