---
name: receipt-audit
description: Review a sealed Runx receipt against a grant baseline, with exact native evidence, scoped governance findings and explicit observability gaps.
---

# Receipt Audit

Assess one sealed run for recorded authority conflicts, inconsistent declared
approval, recorded refusals or exposed material in the data actually inspected.
Supply an exact receipt id and the grant baseline you want checked. The result
identifies findings, their limits and the evidence needed to resolve them.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `receipt-query#read`

## Evidence and assurance

The default `audit` resolves missing receipt metadata through `receipt-query#read`.
A completed `receipt_evidence` packet bypasses that read. The explicit
`from-evidence` phase accepts the same packet directly for composed graphs. Reuse its receipt
identity, native verification posture, recorded authority, decisions and lineage;
state that supplied evidence was reused rather than independently verified here.
Explicit `receipt_rows`/`receipt_details` are replay inputs: their labels are not
cryptographic verification. A summary alone cannot establish a clean run.

The projection's `exercised_scopes` contains authority references and terms. It
is not a complete per-operation usage trace, proof of success or evidence that
unrecorded permission was unused. Grant references do not themselves reveal the
grant ceiling. `granted_scopes` is an operator-supplied comparison baseline, not
a newly minted grant. Preserve opaque scope names. Obtain native policy/term
evidence for wildcard, verb or resource coverage; never implement those rules
by guessing string prefixes.

Use `receipt.read` only when the question needs a retained declared output. It
requires `receipt.claim.read` and verifies the signed binding before returning
current-workspace data. Follow a graph's verified child references to the
relevant step; `not_a_step` is not an output body. `not_retained`, a read
that `reach: workspace` refuses or an exceeded byte budget is an evidence gap.
Narrow a JSON pointer or obtain an operator read when appropriate. Never replay the
operation to reconstruct evidence. A local signature is valid local provenance,
not independent provider truth; hosted/CI labels alone do not establish independence.

Inspect only material needed for this question. A hashed/redacted reference
cannot prove its hidden body secret-free. Do not echo a discovered credential;
report its field/locator and recommend appropriate revocation or rotation.
Raw inputs and transport diagnostics are outside the claim reader.

## Procedure

1. Confirm the exact receipt, source posture and intact native tree. A failed
   or blocked run can be audited too; do not equate its outcome with an anomaly.
2. Compare recorded authority with the supplied baseline. State which fields
   were observed and which grant/resource/policy evidence is still missing.
3. Check approval only where signed effect/authority evidence required it. A
   routine granted mutation does not imply a separate human approval. A URI
   or decision label without exact binding is insufficient to prove approval.
4. Inspect relevant child metadata or retained claims only when needed. An
   absent refusal is a finding only if evidence shows the refused attempt.
5. Return a scoped assessment. Preserve contradictions even when the overall
   verdict is inconclusive. No provider action, ledger repair or grant change occurs.

## Result and continuation

The unchanged `runx/receipt-audit.audit.v1` packet carries the exact receipt id,
operator baseline, recorded scopes in its historical `exercised_scopes` field,
findings, verdict, rationale and next evidence. `clean` means no anomaly found
in the inspected evidence against that baseline; it is never a complete usage
or secret-free guarantee. Missing verification/baseline or relevant evidence
requires `needs_more_evidence`. A final domain check binds identity and scope
arrays and prevents a clean verdict alongside errors or incomplete proof.

For example, a verified receipt may record `repo:read` against a supplied
`repo:read` baseline. That supports a clean recorded-scope comparison. It does
not prove no other permission was used outside the recorded trace. A replay row
labelled verified with no native tree proof remains inconclusive. If exact
signed evidence instead records an act beyond its admitted ceiling, flag the
conflict and preserve the evidence for investigation; do not revoke a grant here.

Use `receipt-query` to locate evidence, `diagnose-skill-run` to investigate a
failure with actual diagnostics, and `least-privilege` to review a grant and
representative coverage before proposing a reduction. Carry the original id,
findings and actual readbacks forward. Do not run the original action again.

## Inputs

- `receipt_id`: exact receipt under review. Without it the result is inconclusive.
- `granted_scopes`: explicit string-array baseline, including an intentional
  empty array. Without it the authority comparison remains inconclusive.
- `receipt_evidence`: optional completed `runx/receipt-query.answer.v1` for reuse.
- `objective`: governance question; default review is recorded evidence only.
- `receipt_summary`: supplemental context, never primary verification.
- `receipt_rows`, `receipt_details`: explicit replay-only projections.

## Agent task: receipt-audit

Return `receipt_audit` using the existing packet's inner assessment shape. Use
`supplied_evidence` when present, otherwise `receipt_evidence`. Identify the
exact receipt and baseline, inspect only relevant records, and separate facts
from hypotheses. You may use native `receipt.prove` for relevant child metadata
and `receipt.read` for exact retained claims. Preserve unavailable/denied claims
as gaps rather than guessing their contents. Do not read again when adequate
evidence is already supplied, and do not replay, mutate or repair anything.

The default scope is recorded evidence. For a question requiring wider usage,
approval or secret coverage that the available evidence cannot establish, use
`needs_more_evidence`. Never describe projected authority as observed successful
usage. Every finding must identify its supporting receipt/field and its meaning;
an approval or over-reach claim needs exact attributable evidence. Do not infer
policy from an action name. Keep secret values out of the result.
