---
name: ghostwrite
description: Turn validated evidence and operator intent into publication-ready drafts, deterministic channel packages, and provider-neutral handoffs.
---

# Ghostwrite

Write one bounded artifact in the voice of the human or team responsible for
it. The skill turns validated evidence and a clear objective into useful public
or internal prose without exposing the machinery that produced the evidence or
inventing facts to make the draft feel complete.

This is the reusable authorship primitive behind briefs, release notes, trust
reports, maintainer updates, newsletters, and social drafts. It can also package
an accepted draft for a channel and prepare a provider-neutral handoff. It never
publishes, posts, sends, or claims provider acknowledgement.

## Writing standard

- Lead with the reader's problem, decision, or next action—not the evidence
  collection process.
- Turn evidence into concrete claims and examples; do not dump receipts, issue
  threads, graph traces, or machine packets into the body.
- Match the project's vocabulary and supplied writing context instead of
  defaulting to generic AI, launch, preview, or adoption language.
- Write as the responsible maintainer or operator. The surfaced artifact should
  not call itself agent-generated or narrate how a model worked.
- Prefer one sharp, useful artifact over several thin sections.
- Narrow the claim or return `needs_more_evidence` rather than filling a gap
  with plausible prose.

## Runners and chain boundaries

`draft` composes `admit`, one writing turn and `verify`. `admit` binds a ready
research or evidence packet by its native digest and optionally admits
digest-bound `brand-voice` and `taste-profile` context. Only admitted claims and
source digests may enter the draft. `verify` checks every material claim and
every context binding before releasing it, and withholds anything it cannot
bind. A skill that owns its own writing judgment composes `admit` and `verify`
around its own writing turn, so its draft keeps the same guarantees.

`package` converts a validated draft bound to its evidence into a deterministic
channel payload, bound to its digest, in `not_sent` state; a draft without an
evidence binding, or with a claim citing a source it is not bound to, is held.
`handoff` binds that payload's digest to the exact target and records whether
delivery crosses outside the team, whether it then needs the delivering lane's
approval, and the next actor. Neither runner grants external authority.
Delivery belongs to `send-as`, a publication skill, or the relevant provider
connector.

## Inputs and result

- `objective`, `audience`, and `channel` define what the artifact must achieve.
- The evidence input must be a validated packet with stable claim and source
  digests; raw assertions are not promoted into evidence.
- Optional writing-context packets apply voice or taste. A packet is admitted
  only when its declared `applicability` names the requested `audience` exactly
  and lists the requested `channel` among its brand `channels` or taste
  `surfaces`. A packet outside that scope, or one that declares none, returns
  `needs_more_evidence` so the right context can be supplied. Each admitted
  packet's exact digest, the audience and channel it was admitted for, and only
  the rules actually used are recorded.
- Apply the admitted context's structured `profile` and `applicability`, not
  just its evidence-rule summaries. Brand vocabulary, cadence, claim limits and
  channel adjustments matter; taste principles, examples and dislikes matter.
- `verify` takes the evidence context `admit` returned and the candidate draft
  written from it. The admitted context, not the candidate, is the authority
  for source digests and context rules.
- `package` takes a `runx.content.draft.v1` packet; `handoff` takes the
  `runx.content.publish.v1` packet `package` returned, its channel, the exact
  target, and `boundary_kind` (`internal` or `external`).

The draft result contains the content brief, reader-facing body with claim
references, review checklist, distribution notes, and writing-context bindings.
Packaging returns the channel payload, its evidence binding and digest, and
`not_sent` state. Handoff returns the target, boundary and next actor, without
delivery evidence.

## Stop conditions

- Withhold the draft when material claims lack admitted evidence.
- Reject unknown source or writing-context digests and any context rule that was
  not present in the supplied packet.
- Withhold the draft when a supplied writing context does not apply to the
  requested audience and channel.
- Do not let voice guidance turn an unsupported factual claim into a safe one.
- Do not mark a package published, delivered, acknowledged, or approved merely
  because its local formatting succeeded.
- Keep confidential source content and secrets out of public bodies and handoff
  metadata.

## Example

A research packet supports three claims about a release. A brand-voice packet
asks for direct, evidence-led language. `draft` can write a concise release note
using those claims and record the voice packet digest. `package` can create the
newsletter payload. `handoff` can point to the email provider lane. None of those
steps means the newsletter was approved or sent.

## Agent task contract

### `ghostwrite-draft`

Write only from the supplied evidence context. Return the content brief, draft,
review checklist, distribution notes, and exact context bindings. Every material
claim must cite admitted source digests. When writing contexts are present,
record each exact packet digest and only rules actually applied. Return
`needs_more_evidence` when the requested artifact cannot be supported. Do not
add publication state, provider claims, or facts absent from the evidence.
Respect each context's applicability, structured profile and stop conditions.
Profile examples guide style; they are not evidence for new factual claims.
