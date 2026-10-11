---
name: receipt-query
description: Retrieve bounded receipt evidence by exact ids or explicit filters, returning native verification results for downstream review.
---

# Receipt Query

Collect the exact signed run evidence a reviewer needs, using explicit receipt
ids or bounded filters. Native Runx owns receipt lookup and verification; this
skill normalizes the query and returns a reusable evidence packet. It is
read-only.

The question records the review purpose. It is not translated into hidden
filters: amount thresholds, business totals, scope searches and arbitrary
natural-language conditions are not implemented by this runner. Supply the
supported filter fields, then pass the resulting evidence to the appropriate
review skill.

## What this skill does

`receipt-query` applies an explicit bounded query over receipt ids, principal, skill
ref, status, and time range, then returns the receipts that satisfy it as
id-keyed stubs (`receipt_id`, `skill_ref`, `status`, `created_at`, verification
status). Exact-id reads also return `receipt_details` from the native redacted
inspection projection: signed authority, decisions, acts, artifact references,
lineage, seal, and verification without execution bodies, credential material,
or local paths. When `proof` asks for it, the skill returns the native tree-verification
verdict and findings. A requested check is not a successful check: an unknown
or failed verdict stays unknown or failed. The deterministic summary reports
matched receipt count and verification status, not a synthesized business
answer. Zero matches return `needs_more_evidence`.

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

It retrieves evidence across many runs; `receipt-audit` assesses governance
against an exact receipt and its authority baseline. Its nearest neighbor is
`run-history`, which also reads the ledger but returns deterministic platform
outcome and catalog-coverage metrics and routes them to governance
lanes. `receipt-query` answers one precise audit question with the receipt ids that
match and a verified chain walk, not an aggregate health report.

## When to use this skill

- A reviewer needs the receipts for an exact run or a bounded skill, actor,
  status, issuer or time window.
- A review needs the set of receipts that match a condition before drilling into
  any single one.
- A compliance check needs proof that a stretch of ledger history is unbroken.

## When not to use this skill

- To audit whether one run stayed inside its grant. Use `receipt-audit`; that
  is the integrity-of-one-chain question, not the cross-run history question
  `receipt-query` answers.
- To review a grant before narrowing it. Use `least-privilege`.
- For a graded platform-health report over many runs. Use `run-history`.
- To mutate, redact, export, or archive receipts. This skill is read-only and
  refuses any write framing.
- To return receipt bodies, act payloads, or any secret-bearing field. Matched
  receipts are id stubs only.

## Procedure

1. Read the question. With no question, return `needs_agent`.
2. Resolve the filter into a bounded query: principal handle, `skill_ref`,
   status set, and `time_range`. An absent filter uses the bounded default
   limit; the prose question never adds a filter.
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
6. Return the deterministic `summary` of matched count and native verification from the
   matched set and the verification result only. The question states the purpose;
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
- **Write, delete, or reseal requests:** this read runner cannot perform them.
  Its sole native query scope is `receipt.read`; prose does not grant a write.

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

Input:

```yaml
question: Collect receipt evidence for the campaign review.
receipt_ids: [rx_101, rx_102]
proof:
  verify_chain: true
```

The runner returns the matched id stubs, redacted native details and the native
verification verdict. Missing ids remain absent; the caller must compare the
requested ids with the matched set before relying on the evidence. A failed,
unknown or bounded-out verification verdict cannot support a claim of intact
proof. Supplying replay projections returns an unknown chain verdict even when
the fixture labels its rows verified.

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
