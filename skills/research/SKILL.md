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

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `web-fetch#web-fetch`
- `web-search#search`

## Where it fits

The default `investigate` method accepts the question and optional `urls`,
`paths` or existing `source_packets`. Supplied packets bypass acquisition.
Otherwise the host works a governed acquisition turn over native local reads
and the existing `web-fetch`/`web-search` skills; the workflow then runs the same
research analysis and verifier. Explicit URLs and paths do not require a search.
Use `research` explicitly for already collected source packets, or `local-files`
for a wholly declarative read of known UTF-8 files. All routes converge on the
same source and content-digest index. URLs, paths, model memory, operator context, and target
names do not become evidence merely because they were supplied.

The packet is a reusable primitive for `research-brief`, `content-package`,
`ghostwrite`, product planning, and other decision workflows.
It does not publish, notify, or mutate a provider. Those later operations keep
their own authority, approval, and readback boundary.

## How it works

1. State a bounded objective and the decision the research should inform.
2. Reuse the supplied readback packets, or acquire only the evidence missing
   for the objective through actual declared reader calls. Runx indexes their
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
- Keep source acquisition inside its reader capability; publishing, messaging
  and mutations still require their own skill and authority.

## Example

An operator asks whether a proposed integration belongs in the next release and
supplies a changelog readback, an issue snapshot, and an official API document.
The packet can compare the evidence, state which user problem is verified, and
recommend build, defer, or investigate. It cannot cite a remembered forum post
or operator hunch as verified demand, and it should name the missing evidence if
that gap changes the recommendation.


## Agent task: research-acquire

Acquire evidence for the stated question, not an answer from memory. Read the
provided URLs or local paths directly. For local paths use fs_read_bundle with
the current workspace as root, bounded bytes and on_missing: report; do not
crawl unrelated directories. For a public URL call the offered web-fetch skill
with text extraction. If source discovery is actually missing, use web-search
for a focused query, choose relevant primary sources and fetch the selected
pages. Search result snippets are candidates, not fetched source content.
Use the exact offered tool names and schemas from native turn status.

For `fs_read_bundle`, take the unchanged entries in
`output.file_read_bundle.data.files` as the individual source packets. The
outer bundle groups files and missing-path reports; it is not itself a source.
Keep each file entry's path, contents, byte count, content_digest and truncation
metadata. Inspect `missing` before proceeding; when a missing source prevents
the question being answered, ask for it or return no sources so the existing
evidence stop remains explicit. Do not imply that every requested path was read.

Keep every original source packet, including its contents/extracted text,
source identity, content digest and observation metadata. Return only
source_packets containing those actual outputs. Do not invent a digest,
provider response or read; do not rewrite text into a fabricated source. Carry
provider failures and coverage limits as missing evidence, never a negative
finding. If no valid source can be obtained, return an empty list and let the
existing analysis produce its evidence stop. Ask for missing scope through the
native ask tool when it would change which sources are relevant.

Do not synthesize the final research in this acquisition turn. The workflow's
existing research phase owns that single synthesis and source verification.
Native connector authority and charge limits still apply to search/read calls;
a result count is not a dollar cap. Preserve a pending approval or uncertain
charged call rather than starting another lookup. Reuse a completed call by
its stable call ID and retain receipt lineage. Acquisition never publishes or
changes provider state.

For composition, call research with source_packets or local-files with known
paths. Their contracts and result packet are unchanged. The investigate default
also bypasses acquisition when packets are supplied, so continuing from prior
evidence never requires another discovery turn.
