// How many leads the batch works at once, as many as the step's concurrency
// runs calls at once. Each lane takes the next lead when its last is done, so
// the worker holds at most this many leads however many the batch holds: a
// batch read all at once holds every lead through its three calls, and 300
// leads at their largest pass the worker's 64 MiB.
const LANES = 4;

// For each lead: enrich its supplied sources, qualify it against the
// criteria, and prepare its first contact. Every lead's outcome is
// collected in the order given, and a lead's failure or hold stays that
// lead's.
export default async function batch(ctx, inputs) {
  const { leads } = inputs;
  const outcomes = [];
  let next = 0;
  const lane = async () => {
    while (next < leads.length) {
      const index = next;
      next += 1;
      outcomes[index] = await outcome(ctx, leads[index], inputs);
    }
  };
  await Promise.all(Array.from({ length: LANES }, lane));
  return { outcomes };
}

// The key a lead's send is made under: its campaign and its recipient,
// never its draft, so a revised batch's new draft is the same intent and a
// lead already sent is never sent again. An address is compared without
// case, since a mailbox written two ways is one recipient.
function sendKey(campaign, recipient) {
  return `cold-outreach/${campaign}/${recipient.toLowerCase()}`;
}

// How long a lead's qualification is reused: a week, the revisions of one
// campaign. It judges only what it's given, which reuse matches exactly, so
// only the judgment itself ages.
const QUALIFICATION_MAX_AGE_SECONDS = 604800;

// One lead's outcome. A batch past 64 KiB reaches the script lead by lead,
// each a reference ctx.read reads; under it, ctx.read gives the lead back as
// it is. Reading is the lead's first stage, inside its own try, so a lead
// too large to read is that lead's rejection, named by its place in the
// outcomes since its id was never read, and the others go on.
async function outcome(ctx, item, inputs) {
  let stage = "read";
  let named = {};
  try {
    const lead = await ctx.read(item);
    named = { lead: lead.id, idempotency_key: sendKey(inputs.campaign, lead.prospect.email) };
    stage = "enrich";
    const { lead_enrichment_packet } = await ctx.skill("../lead-enrichment#enrich", {
      subject: lead.subject,
      sources: lead.sources,
      requested_fields: inputs.requested_fields,
      as_of: inputs.as_of,
    });
    stage = "qualify";
    const { lead_qualification } = await ctx.skill(
      "../lead-qualify#qualify",
      {
        enrichment_packet: lead_enrichment_packet,
        criteria: inputs.criteria,
        ...eligibility(lead),
      },
      { reuse: { max_age_seconds: QUALIFICATION_MAX_AGE_SECONDS } },
    );
    stage = "prepare";
    const { outreach_review } = await ctx.skill(".#prepare", {
      sender: inputs.sender,
      prospect: { ...lead.prospect, id: lead.subject.id },
      audience: inputs.audience,
      purpose: inputs.purpose,
      offer: inputs.offer,
      required_footer: inputs.required_footer,
      contact_policy: lead.contact_policy,
      qualification: lead_qualification,
      ...(inputs.memory === undefined ? {} : { memory: inputs.memory }),
      ...(lead.facts === undefined ? {} : { facts: lead.facts }),
    });
    return { ...named, status: "fulfilled", outreach_review };
  } catch (rejected) {
    return {
      ...named,
      status: "rejected",
      rejection: { stage, reason: rejected.reason, code: rejected.code, message: rejected.message },
    };
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
