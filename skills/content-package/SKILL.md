---
name: content-package
description: Turn governed source evidence into a citation-bound reader-facing draft, channel package, and provider-neutral publication handoff.
---

# Content Package

This is the standard preparation lane for researched content. It keeps evidence,
writing, packaging, and external delivery distinct so an operator can inspect
one concrete artifact and no local success is mistaken for publication.

Use it for articles, updates, newsletters, explainers, and other channel-bound
content whose substantive claims need traceable support. The result should do
something for the reader—clarify a decision, explain a change, establish trust,
or enable a concrete next step—not merely prove that research happened.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ghostwrite#draft`
- `ghostwrite#handoff`
- `ghostwrite#package`
- `research#research`

## How the chain works

1. The caller provides the objective, audience, channel, and governed source
   packets. Usually those packets come from `web-fetch` or a provider reader.
2. `research` admits the sources, verifies citations, and decides whether the
   evidence supports a useful deliverable.
3. `ghostwrite` drafts only from the admitted claims and digests, applying any
   supplied voice or operator context without treating it as factual evidence.
4. The validated draft is packaged deterministically for the declared channel.
5. A provider-neutral handoff records the target and approval requirement. The
   lane stops before external delivery.

Local research, drafting, and packaging require no approval. A later provider
skill owns the consequential gate, idempotency, send or publish request,
acknowledgement, and readback.

## Inputs and result

- `objective` states what the content should change for its reader.
- `source_packets` contain bounded governed evidence.
- `audience`, `channel`, and `domain` shape the artifact.
- `operator_context` and `target_entities` narrow interpretation but are not
  source evidence.
- `publication_target` and `boundary_kind` (`internal` or `external`) prepare the
  next lane; they do not authorize it. An external delivery needs the
  delivering lane's own approval.

The result preserves the citation-validated research packet, the evidence-bound
draft, a deterministic channel package with `not_sent` state, and a
provider-neutral handoff. `needs_more_evidence`, `needs_review`, and
`not_worth_publishing` are valid terminal states when the topic is unsupported,
stale, duplicative, or unhelpful.

## Stop conditions

- Stop before drafting when the source packet is missing or invalid.
- Do not turn operator context, campaign intent, or a target URL into evidence.
- Withhold any draft whose material claim cannot be tied to an admitted digest.
- Do not add delivery or publication evidence to a local package.
- Route the exact accepted artifact into the relevant provider lane instead of
  rebuilding it there.

## Example

An operator wants a blog post explaining a new governance boundary. The lane
can validate official docs and receipt evidence, draft for engineering readers,
package the accepted body for the blog, and prepare a CMS handoff. It cannot say
the post is live until an approved CMS operation and provider readback prove it.
