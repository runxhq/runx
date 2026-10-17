---
name: research-brief
description: Produce a decision-ready research-brief brief from bounded governed evidence and preserve every material source binding.
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

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ghostwrite#draft`
- `research#local-files`
- `research#research`

## When to use it

Use `research-brief` for architecture choices, market or ecosystem questions,
product bets, trust decisions, or other consequential analysis that needs a
reader-ready synthesis. Use plain `research` when the evidence packet itself is
the desired artifact. Use `content-package` when the primary outcome is public
content for a known channel.

Do not use this skill for an unbounded literature review, a daily trend recap,
or research whose sources have not been fetched through a governed reader.

## How the chain works

1. The caller supplies the exact question and audience plus either governed
   source packets or bounded local paths. The `local-files` runner reads those
   paths through Runx's native filesystem boundary; it does not tunnel local
   evidence through HTTP.
2. The canonical `research` skill admits and indexes those sources, separates
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
- `source_packets` are bounded governed evidence. The `local-files` runner
  accepts bounded paths below a selected local root instead. Operator context,
  URLs, bare paths, and model memory do not substitute for admitted evidence.
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
