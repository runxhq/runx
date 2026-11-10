---
name: research-brief
description: Produce a decision-ready memo from a question, bounded sources or completed research while preserving every material source binding.
---

# Research Brief

Turn one important question into a durable operator brief: what the answer is,
what evidence supports it, what remains uncertain, and what posture the reader
should take next. Use this when a quick answer is too shallow but an open-ended
report would obscure the decision.

The output should feel like a thoughtful memo, not a narration of the research
process. It leads with the decision and operational implication, then exposes
the evidence, inference, alternatives, and unresolved questions that justify
that posture.

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `ghostwrite#draft`
- `research#investigate`
- `research#local-files`
- `research#research`

## When to use it

Use `research-brief` for architecture choices, market or ecosystem questions,
product bets, trust decisions, or other consequential analysis that needs a
reader-ready synthesis. Use plain `research` when the evidence packet itself is
the desired artifact. Use `content-package` when the primary outcome is public
content for a known channel.

Do not use this skill for an unbounded literature review, a daily trend recap,
or a claim that needs no investigation. Missing sources are acquired through the canonical governed research lane.

## How the chain works

1. The default `brief` method starts from the question and audience, with optional
   `paths`, `urls` or already collected `source_packets`. `research#investigate`
   reads explicit sources directly and searches only when discovery is needed.
   It reuses a nonempty supplied source set without acquisition. The explicit
   `research-brief`, `local-files` and freshness-gated `ecosystem` methods retain
   their existing bounded-source contracts.
2. For a completed `research_packet`, select `from-research` to skip collection
   and research synthesis entirely. Otherwise the canonical `research` skill
   admits and indexes the sources, separates
   evidence from inference, and verifies every citation and recommendation.
3. Only a ready research packet proceeds to `ghostwrite`. The writing stage
   turns decision support into a clear brief without introducing unsupported
   facts.
4. The final artifact preserves the research and content packet bindings. It
   remains local and is not a publication claim.

Local research and drafting need no approval because they do not cross an
external boundary. If the brief is later sent, posted, or published, the
provider delivery skill owns approval, idempotency, acknowledgement, and
readback.

## Inputs and result

- `objective` is the exact decision question.
- `paths` and `urls` select sources for the default method to read; they do not
  themselves count as evidence. `source_packets` reuse already admitted reads.
  `from-research` accepts the canonical `research_packet` and retains its source
  bindings. `local-files` also supports an explicit contained root and byte bound.
- `audience` and `channel` shape the memo's presentation.
- `domain`, `operator_context`, and `target_entities` keep the analysis scoped
  but do not count as evidence.

The result contains the citation-validated `research_packet` and, when evidence
is ready, the evidence-bound `content_draft_packet`. It may finish at
`needs_more_evidence` or `not_worth_publishing` rather than manufacture a memo.

## Stop conditions

- Stop before drafting when no valid source survives research admission.
- Preserve uncertainty that could change the recommendation.
- Do not turn operator preference into a verified market or product fact.
- Do not add a publish or delivery state to a local brief.
- Route any outward action to its owning provider skill with the exact artifact
  digest and evidence packet intact.

## Example

For “Should this provider become a native Runx tool?”, the source set might
include its official API contract, two existing consumer implementations, and
runtime measurements. The brief should recommend native ownership, package
ownership, or no change; show which evidence supports that boundary; and name
the remaining operational risk. It should not become a generic provider profile
or imply adoption merely because the API exists.
