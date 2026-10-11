---
name: ledger
description: Answer a cross-run audit question against the receipt ledger, returning matched receipts and a chain-verification result.
---

# Ledger

Answer one audit question against the whole receipt ledger, and prove the chain
behind the answer.

runx seals a receipt for every run. Those receipts accumulate into a ledger:
every act, every approval, every refusal, across every principal and every
skill, in sealed order. When an auditor asks "did anyone spend over $500 last
week", "how many sends did this principal authorize", or "which runs touched the
billing scope", the answer lives in that ledger. This skill turns the question
into a ledger query, returns the receipts that match by id, and, when asked,
verifies that the matched stretch of the chain is intact. It reads; it never
writes.

## What this skill does

`ledger` applies an explicit bounded query over receipt ids, principal, skill
ref, status, and time range, then returns the receipts that satisfy it as
id-keyed stubs (`receipt_id`, `skill_ref`, `status`, `created_at`, verification
status). Exact-id reads also return `receipt_details` from the native redacted
inspection projection: signed authority, decisions, acts, artifact references,
lineage, seal, and verification without execution bodies, credential material,
or local paths. When `proof` asks for it, the skill confirms the matched stretch of
the chain is intact, naming any break by the receipt ids involved. The chain is
the proof; a count that looks plausible is not. The skill answers the question in
one or two sentences grounded only in the matched set and the verification
result, and stops with `needs_more_evidence` when the ledger is silent rather
than reporting a fabricated zero.

The default `read` runner invokes the native `receipt.query` service directly.
The service resolves the same receipt store and signature policy as Runx's CLI,
projects bounded history rows, resolves exact ids through the redacted detail
reader, and optionally verifies each matched receipt tree. There is no child
process, command-string construction, or package-local receipt parser. It is
the in-runtime way for an agent to inspect its own sealed receipts before a
gated action. The detail packet is curated in Rust and never returns execution
bodies, credential material, or local paths.

The `read` runner accepts optional `receipts` and `receipt_details` inputs for
replay or controlled evaluation. When present it uses those projections instead
of the native store. Replay is labelled as supplied evidence and never becomes
a live proof path.

It queries and proves history across many runs; `audit-receipt` audits the
integrity of a single receipt chain. Its nearest neighbor is
`run-history`, which also reads the ledger but returns deterministic platform
outcome and catalog-coverage metrics and routes them to governance
lanes. `ledger` answers one precise audit question with the receipt ids that
match and a verified chain walk, not an aggregate health report.

## When to use this skill

- An auditor needs a cross-run answer: counts, totals-by-reference, who did what,
  which runs touched a scope or skill over a window.
- A review needs the set of receipts that match a condition before drilling into
  any single one.
- A compliance check needs proof that a stretch of ledger history is unbroken.

## When not to use this skill

- To audit whether one run stayed inside its grant. Use `audit-receipt`; that
  is the integrity-of-one-chain question, not the cross-run history question
  `ledger` answers.
- To narrow a grant from observed usage. Use `least-privilege`.
- For a graded platform-health report over many runs. Use `run-history`.
- To mutate, redact, export, or archive receipts. This skill is read-only and
  refuses any write framing.
- To return receipt bodies, act payloads, or any secret-bearing field. Matched
  receipts are id stubs only.

## Procedure

1. Read the question. With no question, return `needs_agent`.
2. Resolve the filter into a bounded query: principal handle, `skill_ref`,
   status set, and `time_range`. An absent filter means the question alone bounds
   the query.
3. Match receipts against the query and collect id-keyed stubs only. Receipt
   bodies, act payloads, proofs, and material refs stay out of the output.
4. With zero matches, return `needs_more_evidence` and name the query that found
   nothing.
5. When `proof` requests chain verification, take the chain verdict from the
   engine's tree-rooted native proof report, not a hand-rolled link
   walk: `intact` follows the report's overall validity, and each `break` is
   derived from a tree's missing parent or a verification finding, named by the
   receipt ids involved. The engine checks signatures against this home's
   own key and any configured verify key; a receipt no trusted key signed is a
   break, never silently intact. When the proof is bounded out, the chain is
   reported unverified (`checked: true`, `intact: null`). When `proof` is
   absent, set `chain_verification.checked` to false and leave `intact` null.
6. Write a one or two sentence `summary` that answers the question from the
   matched set and the verification result only. The question bounds the answer;
   do not generalize from the matched slice to the whole ledger.
7. Return the `ledger_answer` with `decision: answered`.

## Edge cases and stop conditions

- **No question:** return `needs_agent`; there is nothing to query.
- **No matching receipts:** return `needs_more_evidence` with the resolved query,
  so the gap is the query, not a silent zero. Silence is a stop, not a zero.
- **Filter references an unknown principal or skill_ref:** treat as zero matches
  and return `needs_more_evidence`; do not guess a near match.
- **Chain break found:** keep `decision: answered` but set
  `chain_verification.intact` false and list the breaking id pairs from the
  verify report; an intact answer set with a broken chain is still a reportable
  result.
- **Verify keys absent:** report `chain_verification.intact` null with
  `checked: true`; the chain is unverified, not intact.
- **Verification requested over an empty match set:** the stop is
  `needs_more_evidence` for the match, not a chain claim over nothing.
- **Write, delete, or reseal framing in the question:** refuse; this skill holds
  `ledger:read` scope only and no gate can widen it.

## Output schema

```yaml
ledger_answer:
  decision: answered | needs_agent | needs_more_evidence | refused
  question: string        # the audit question, restated in operational terms
  query:                  # the resolved filter actually run, so the answer reproduces
    principal: string
    skill_ref: string
    status: array
    time_range:
      from: string
      to: string
matched_receipts:         # id-keyed stubs only; never a receipt body
  - receipt_id: string
    skill_ref: string
    status: string
    created_at: string
receipt_details:          # exact-id reads only; Rust-curated, bounded, redacted
  - id: string
    authority: object
    decisions: array
    acts: array
    artifact_refs: array
    lineage_refs: array
chain_verification:
  checked: boolean        # was verification requested
  intact: boolean | null  # null when unchecked
  breaks:                 # empty when intact
    - from_receipt_id: string
      to_receipt_id: string
      reason: string
summary: string           # one or two sentences answering the question
```

The `ledger_answer` object is the named packet `runx.ledger_answer.v1`. Scope is
`receipt.read` only; no gate is required because the skill cannot mutate, and a
delete, redact, reseal, or reorder request is refused, not gated. The run's own
receipt carries the question, the resolved query, the matched count, the list of
matched `receipt_id` values, and the chain-verification result; it carries no
matched receipt body, principal PII, or secret material. Matched receipts are
always referenced by id.

## Worked example

Input: "Which spend runs over $500 sealed for the ops principal last week, and is
that stretch of the chain intact?" with a filter scoping `principal:ops`,
`skill_ref runx/spend`, status `sealed`, and the 2026-06-01 to 2026-06-08
window, plus `proof: { verify_chain: true }`.

Output: `decision: answered`. The resolved query is echoed for reproducibility.
`matched_receipts` lists two sealed spend stubs by id with `skill_ref`, `status`,
and `created_at`, no bodies. `chain_verification` reports `checked: true`,
`intact: true`, `breaks: []` from the engine's tree-rooted verify verdict over
the store. The `summary` reads: two sealed spend runs over $500 ran for
`principal:ops` in the window, and the verify verdict is intact. Had the window
matched zero receipts, the run would stop at `needs_more_evidence` naming the
query, not report a clean zero. Run through the `read` runner, the same answer
comes straight from the native history and proof services over the resolved store;
when the verify keys are absent the chain is reported unverified rather than
intact.

## Inputs

- `question` (optional): the audit question bounding the ledger read. Without
  one the runner returns `needs_agent` and does not query.
- `filter` (optional): JSON narrowing the query by `principal`, `skill_ref`,
  `status`, `time_range` (`from`/`to`), and `limit` (default 500, maximum 5000).
- `receipt_ids` (optional): up to 100 exact receipt ids to resolve and verify.
- `proof` (optional): JSON requesting chain verification over the matched
  receipts, for example `{ "verify_chain": true }`.
- `receipts` (optional): explicit ledger rows for replay or controlled
  evaluation; when present the `read` runner does not query live native
  history.
- `receipt_details` (optional): explicit native redacted detail projections for
  deterministic replay only.
