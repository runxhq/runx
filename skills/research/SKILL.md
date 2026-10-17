---
name: research
description: Produce bounded, source-backed research packets for product, ecosystem, and operator decisions.
---

# Research

Answer one practical question with enough evidence to change a decision. This
skill is for applied research: which issue is worth tackling, whether a proposal
is grounded, what changed in an ecosystem, or which claims a public artifact can
safely make. It is not an open-ended browsing session or a summary for its own
sake.

The result should make the operator's posture clearer: build, write, avoid,
defer, compare, or gather a named missing fact. Prefer a small number of
well-supported findings over a broad report whose conclusions cannot be traced.

## Where it fits

`research` consumes governed source packets. Fetch evidence with `web-fetch` or
a provider reader, or use the `local-files` runner to admit bounded UTF-8 files
through Runx's native filesystem reader. Both paths converge on the same source
and content-digest index. URLs, paths, model memory, operator context, and target
names do not become evidence merely because they were supplied.

The packet is a reusable primitive for `research-brief`, `content-package`,
`ghostwrite`, product planning, and other decision workflows.
It does not publish, notify, or mutate a provider. Those later operations keep
their own authority, approval, and readback boundary.

## How it works

1. State a bounded objective and the decision the research should inform.
2. Supply up to the declared source limit as governed readback packets, or name
   bounded local files through the dedicated runner. Runx indexes their
   provenance, observation times, source digests, and content digests before
   synthesis.
3. Synthesis separates verified facts from inference, explains relevance,
   records confidence, and carries open questions rather than filling gaps with
   plausible prose.
4. Every material claim cites an admitted source digest. Every recommended
   option cites the evidence on which its rationale depends.
5. The deterministic verifier rejects unknown citations, digest drift, and
   unsupported decision support before the packet is released.

Return `needs_more_evidence` when the admitted sources cannot support the
decision. Return `not_worth_publishing` when a finding is sound but irrelevant
to the stated deliverable. Neither state is a failure; both prevent weak
evidence from turning into confident downstream content.

## Inputs and result

- `objective` is the exact question to answer.
- `source_packets` are bounded provider or fetch readback packets with stable
  provenance and content digests. `local-files` instead accepts contained paths
  and never requires HTTP or a hosted connector.
- `domain`, `deliverable`, `operator_context`, and `target_entities` narrow the
  analysis. They are interpretation context, never source evidence.

The research packet contains the bounded brief, evidence log, decision options,
risks, open questions, admitted source evidence, and deterministic validation.
Consumers should carry the packet and its digest forward rather than rewriting
its findings into an untraceable summary.

## Stop conditions

- Stop when no valid source packet was supplied, evidence is stale for the
  decision, or source identity cannot be verified.
- Never cite an unknown source digest or treat a URL as proof that it was read.
- Label inference as inference and lower confidence when evidence conflicts.
- Do not manufacture balance by inventing an opposing claim with no source.
- Keep source content out of public artifacts unless the downstream skill is
  allowed to use it and respects quotation and privacy constraints.
- Route requests to browse, publish, message, or mutate to the skill that owns
  that boundary.

## Example

An operator asks whether a proposed integration belongs in the next release and
supplies a changelog readback, an issue snapshot, and an official API document.
The packet can compare the evidence, state which user problem is verified, and
recommend build, defer, or investigate. It cannot cite a remembered forum post
or operator hunch as verified demand, and it should name the missing evidence if
that gap changes the recommendation.
