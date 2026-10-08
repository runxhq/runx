---
name: answer-from-docs
description: Answer one question strictly from a small, caller-supplied documentation corpus, with exact supporting quotations or an explicit account of what the corpus cannot answer.
---

# Answer From Docs

Use this skill when the source material is already in hand and the operator
needs one answer that can be checked against it. The corpus is the complete
evidence boundary for the run. The skill does not search the web, fetch current
documentation, consult private state, or fill gaps from the agent's general
knowledge.

The useful result is not merely fluent text. A grounded result binds the corpus
through a native digest, names the source for every citation, and preserves an
exact quotation that the deterministic finalizer can find in that source. An
unsupported result is also useful: it says what evidence is absent instead of
turning a plausible guess into product documentation.

## Evidence boundary

Supply:

- `question`: one concrete question.
- `corpus`: a small, bounded array of source objects. Every source needs a
  stable `id`, a human-readable `title`, and non-empty `text`.

Keep only material relevant to the question in the corpus. Runner inputs are
control data, not a document store. Admit large immutable files with Runx's
artifact tools, retrieve the necessary bounded pages, and then call this skill
with the resulting small source packet. Use `extract` first when the material
must be cleaned or structured. Use `knowledge-router`, `research`, or
`deep-research` when the sources still need to be found.

Source order is not authority. Prefer direct, specific language over a vague
summary. If two sources disagree, preserve the conflict and refuse a single
unqualified answer unless the corpus itself establishes precedence.

## Decision procedure

1. Read the question literally. Do not broaden it to a nearby question the
   corpus happens to answer.
2. Treat the supplied sources as exhaustive for this run. Outside knowledge can
   help interpret language, but it cannot support a claim.
3. Find the smallest set of passages that directly supports the answer.
4. Draft a concise answer. Every material statement needs a citation with the
   source `id` and an exact quotation copied from that source's `text`.
5. Use `unsupported` when the evidence is absent or too weak. Name the missing
   policy, limit, procedure, or source in `kb_gaps`.
6. Use `conflicted` when supplied sources materially disagree. Record the
   conflict and the evidence needed to resolve it.
7. Let the deterministic finalizer verify corpus shape, source membership,
   exact quotations, the native corpus digest, and terminal result shape.
   Invalid evidence cannot seal as a grounded answer.

Lexical overlap is not proof. A sentence that repeats words from the question
but addresses a different rule, product, time period, or actor does not support
the answer.

## Result contract

The terminal `grounded_answer` contains:

- `decision`: `answered`, `unsupported`, or `conflicted`.
- `grounded`: true only for a validated `answered` result.
- `answer.text`: the supported answer, empty for a refusal.
- `answer.citations`: canonical source ids, titles, and exact quotations.
- `kb_gaps`: evidence needed before the question can be answered.
- `conflicts`: material disagreements in the supplied corpus.
- `corpus_digest`: the native digest of the exact corpus used for the run.
- `validation`: pass or fail with deterministic findings.

A sealed refusal proves the lane completed honestly. It does not mean the
product lacks the requested capability; it means this corpus did not prove it.

## Stop and recovery

Stop when the question is missing, the corpus is malformed, a cited quotation
is not present in its named source, or the evidence cannot support the answer.
Do not weaken the wording, invent a citation, or silently answer from memory.

Recover by narrowing the question, correcting malformed source objects, adding
the missing authoritative material, or routing source discovery to the
appropriate research skill. Rerun from the complete corrected corpus so the
new digest represents the whole evidence boundary.

## Agent task contract

### `answer-from-docs-synthesize`

Return exactly one `answer_draft` object.

For a supported answer, set `decision` to `answered`; provide non-empty
`answer.text`, one or more citations, and empty `kb_gaps` and `conflicts`.
Every citation contains only `source_id` and `quote`. Copy `quote` verbatim
from the `text` of the named supplied source.

When the corpus cannot answer the question, set `decision` to `unsupported`;
leave `answer.text` empty, provide no citations, and name at least one specific
gap in `kb_gaps`. When sources materially disagree, use `conflicted`, leave the
answer empty, provide no citations, and describe the disagreement in
`conflicts`.

Do not retrieve, use outside knowledge as evidence, manufacture source ids,
paraphrase quotations, claim external effects, or include fields outside the
contract.
