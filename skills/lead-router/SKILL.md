---
name: lead-router
description: Route a validated lead to direct outreach planning, nurture planning, or a recorded hold, preserving why that path was chosen.
---

# Lead Router

Not every lead should receive the same treatment. `lead-router` takes one
validated enrichment packet and chooses exactly one governed posture:
`reach_out`, `nurture`, or `hold`. The route and its evidence remain visible in
the receipt, so the decision is reviewable instead of disappearing inside an
ad-hoc prompt.

This is a routing and planning skill. It does not send a message, enroll a
campaign, or mutate a CRM. Reach-out and nurture outcomes prepare the exact
input for the canonical `send-as` planning boundary. A hold is recorded in the
Runx receipt only; it is not an external suppression-list update.

## When to use it

Use `lead-router` after `lead-enrichment` has produced a validated packet and a
team needs one consistent qualify-then-plan decision. Use a campaign or
provider-specific skill directly when the audience and authorization are
already decided and no lead-level routing remains.

Never use it to bypass consent, contact a suppressed lead, or draft bespoke copy
from raw signals. Enrichment owns evidence synthesis; content skills own copy;
provider connectors own delivery.

## How it works

1. Runx digests and admits the enrichment packet, preserving its signal
   references and validation state.
2. Consent and suppression outrank model judgment. A do-not-contact packet takes
   the deterministic `hold` path.
3. For a ready packet, bounded judgment selects `reach_out`, `nurture`, or
   `hold` and cites admitted signal references in the rationale.
4. Finalization produces either a deterministic hold or an exact, non-executed
   `send-as` handoff marked `prepared_for_send_planning`.
5. The later send lane performs content preflight, obtains any required
   approval, binds idempotency, invokes the provider, and verifies readback.

Ambiguity should become a hold, not an optimistic send plan. A hold is a clean,
auditable terminal state rather than a workflow failure.

## Inputs and result

- `enrichment_packet` is validated `runx.growth.lead_enrichment.v1` data.
- `principal` names the proposed downstream sender; it is not itself send
  authority.
- `objective` describes the intended follow-up.
- `provider_context` may narrow a future provider binding but cannot authorize
  delivery.

The result is a `runx.growth.lead_route.v1` packet with one route, rationale,
evidence references, enrichment digest, and either a hold record or a canonical
downstream planning handoff. It never reports enrollment or delivery.

## Stop conditions

- Deterministically hold when consent or suppression forbids contact.
- Prefer hold when evidence is contradictory, insufficient, or cannot justify a
  channel.
- Reject invented signal references and packets that did not pass enrichment
  validation.
- Do not draft content, authorize a send, or reinterpret provider context as a
  grant.
- If the downstream send is denied or blocked, preserve the route decision but
  do not claim the action occurred.

## Example

A validated packet shows recent product interest, an appropriate role, and
email consent. The router may choose `reach_out` and prepare a `send-as` input
whose rationale cites those exact signals. If consent is missing or a
suppression flag exists, it records `hold`; no nurture or outreach handoff is
created.

## Agent task contract

### `lead-route`

Choose `reach_out`, `nurture`, or `hold` from the admitted enrichment context.
Return the route, rationale, segment, and exact evidence references. Prefer
`hold` when evidence is ambiguous. Do not draft content, authorize a send, or
claim enrollment, provider mutation, or delivery.
