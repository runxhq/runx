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

`draft` admits a ready research or evidence packet and optionally applies
digest-bound `brand-voice` and `taste-profile` context. Only admitted claims and
source digests may enter the draft. Deterministic finalization verifies every
material claim and every context binding before releasing it.

`package` converts a validated draft into a deterministic channel payload with
quality checks and `not_sent` state. `handoff` records the exact target,
boundary, approval need, and next actor. Neither runner grants external
authority. Delivery belongs to `send-as`, a publication skill, or the relevant
provider connector.

## Inputs and result

- `objective`, `audience`, and `channel` define what the artifact must achieve.
- The evidence input must be a validated packet with stable claim and source
  digests; raw assertions are not promoted into evidence.
- Optional writing-context packets apply voice or taste. Their exact digests
  and only the rules actually used are recorded.
- Packaging and handoff runners accept the validated draft or packet and the
  exact target metadata needed by the next boundary.

The draft result contains the content brief, reader-facing body with claim
references, review checklist, distribution notes, and writing-context bindings.
Packaging returns a channel payload and QA state. Handoff returns a
provider-neutral next-action contract without delivery evidence.

## Stop conditions

- Withhold the draft when material claims lack admitted evidence.
- Reject unknown source or writing-context digests and any context rule that was
  not present in the supplied packet.
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
