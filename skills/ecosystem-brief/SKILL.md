---
name: ecosystem-brief
description: Produce a decision-ready ecosystem briefing from fresh governed evidence without claiming publication.
---

# Ecosystem Brief

Produce one time-bounded update on what changed, why it matters, and what an
operator should do with that information. This is the focused monitoring
variant of the research and content chain: a sharp decision brief, not a generic
news roundup or an article padded from weak signals.

Lead with the operational implication. Then show the verified change, sources,
inference, uncertainty, and recommended posture. Connect a signal to product,
trust, distribution, positioning, or catalog work only when the evidence
supports that connection.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ghostwrite#draft`
- `research#research`

## When to use it

Use `ecosystem-brief` for daily or periodic monitoring of a bounded market,
technology, project set, or competitive surface. Use `deep-research` when the
question is durable and needs a fuller decision memo. Use `content-pipeline`
when the principal outcome is reader-facing publication rather than operator
awareness.

## How the chain works

1. Supply governed source packets and an explicit `as_of` time.
2. A deterministic freshness gate rejects missing provenance, future-dated
   observations, and sources older than the declared window.
3. `research` verifies citations and separates evidence from inference.
4. `ghostwrite` turns the ready packet into a concise brief whose claims remain
   bound to admitted source digests.

The brief remains local. A Slack, email, social, or publication skill must own
any outward notification and its approval, idempotency, and provider readback.

## Inputs and result

- `objective` identifies the monitoring question or decision.
- `source_packets` provide governed evidence.
- `as_of` is the explicit evaluation time; `max_age_hours` defines freshness.
- `audience`, `channel`, `domain`, `operator_context`, and `target_entities`
  scope relevance without becoming evidence themselves.

The result includes the freshness report, citation-validated research packet,
and evidence-bound brief. It returns `needs_more_evidence` when no source
survives freshness and provenance checks, and `not_worth_publishing` when the
change is real but offers no useful posture.

## Stop conditions

- Reject stale, future-dated, malformed, or untraceable source packets.
- Do not disguise an old event as a new signal because an article was reposted.
- Carry meaningful uncertainty and conflicting evidence into the brief.
- Do not claim a provider was monitored continuously or a brief was delivered.
- Route external distribution to the owning provider skill with the exact brief
  and source bindings.

## Example

Three fresh official sources show that a framework changed its extension model.
The brief can explain the verified change, infer which integrations may be
affected, and recommend inspect, adopt, or wait. A stale opinion post may inform
background only if admitted under policy; it cannot be presented as the current
event, and a true change with no bearing on the operator can end as
`not_worth_publishing`.
