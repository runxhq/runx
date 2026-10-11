---
name: receipt-audit
description: Audit a sealed runx receipt for governance, comparing exercised authority and any declared approval requirement with signed evidence, and flag over-reach, approval inconsistency, unrecorded refusals, or exposed secret material.
---

# Receipt Audit

Audit a sealed run for authority over-reach, binding the review to its native
receipt identity and verification posture.

Runx seals a receipt for every run. This skill resolves the exact receipt id
through `receipt-query read`, which returns the Rust-owned redacted detail projection
for signed authority, acts, decisions, artifacts, lineage, and verification. It answers one governance
question: did the run stay inside the authority it was granted? It flags scopes
exercised that were never granted, acts that claim an approval requirement or
decision without matching host-attested evidence, refusals that were not
recorded, and any raw secret material that leaked into the receipt. It pairs
with `least-privilege`: that one reviews a
grant from usage, this one verifies a run honored its grant.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `receipt-query#read`

## What this skill does

1. **Read the proof and the acts.** From the receipt, extract the granted
   authority (the proof) and the scopes the acts actually exercised.
2. **Diff exercised against granted.** Any exercised scope not covered by the
   proof is over-reach.
3. **Check declared approval.** When an act's authority or effect evidence says
   exact approval was required, the receipt must carry the matching
   host-attested decision. A routine write inside its admitted grant does not
   acquire an approval requirement merely because it changed provider state.
4. **Check exposure.** The receipt must carry only hashed material references; a
   raw secret in the receipt is a leak.
5. **Verdict.** `clean`, `anomaly`, or `needs_more_evidence`, with the exact
   findings and a recommendation for each anomaly.

## Core principles

- **The receipt is the evidence.** Audit what the receipt records, not what the
  skill claims it did.
- **Granted is the ceiling.** Exercised authority must be a subset of the proof;
  anything beyond is over-reach, full stop.
- **Approval follows the act contract.** Missing or mismatched approval is an
  anomaly only when the signed authority/effect evidence required that exact
  decision. Never infer human-approval policy from a write-shaped verb.
- **No raw material.** A receipt must reference material by hash; raw credential
  material in a receipt is a leak, not a convenience.
- **Absence of evidence is not clean.** With no receipt or an unattributable
  one, return `needs_more_evidence`, never `clean`.

## When to use this skill

- Post-run governance audit of a sealed, successful run.
- Spot-checking that a skill honored its authority bound in production.
- Before promoting a skill toward a higher trust posture.

## When not to use this skill

- To diagnose a failed run and propose a fix. That is `diagnose-skill-run`
  (failure-to-improvement). This skill audits a sealed run for over-reach
  (success-to-governance); the two are different lenses on a receipt.
- To review a grant before narrowing it. Use `least-privilege`; recorded authority alone does not prove successful usage or a safe replacement.

## Diagnostics

- `receipt.authority.over_reach` (error): an exercised scope is not covered by
  the authority proof.
- `receipt.approval.inconsistent` (error): an approval-bound act lacks its
  matching host-attested decision, or the recorded decision does not match the
  exact act.
- `receipt.refusal.unrecorded` (warning): a denied request is not reflected as a
  sealed refusal.
- `receipt.material.exposed` (error): raw credential material appears in the
  receipt instead of a hash reference.
- `receipt.clean` (info): exercised authority is within the grant, every
  declared approval requirement is satisfied, and no material is exposed.

## Procedure

1. Resolve `receipt_id` through the native ledger, load its native redacted
   detail, and verify the matched tree when keys are available. Treat a provided
   `receipt_summary` as supplemental context, never the primary live evidence.
2. Extract the authority proof, granted scopes, acts, approvals, refusals,
   material references, and receipt signature metadata.
3. Normalize exercised scopes from the acts and compare them with the granted
   scopes. Exercised must be a subset of granted.
4. Identify acts whose signed authority/effect evidence explicitly required
   approval. Confirm each has one matching host-attested approval decision. Do
   not treat an ordinary write scope as an implicit approval requirement.
5. Check that denied requests appear as sealed refusals when the receipt records
   the attempt.
6. Scan receipt-visible material for raw credentials or secret-bearing payloads.
7. Return a verdict with findings, recommendations, and the success checkpoint.

## Edge cases and stop conditions

- **Missing receipt:** return `needs_more_evidence`; never infer a clean run.
- **Unattributable receipt:** return `needs_more_evidence` when the receipt
  cannot be tied to the run under audit.
- **Malformed proof:** return `needs_more_evidence` unless enough normalized
  grant data is supplied separately.
- **Unknown scope name:** treat it as over-reach unless the grant explicitly
  covers it.
- **Approval-bound act without matching decision:** emit
  `receipt.approval.inconsistent` even if the provider operation succeeded.
- **Granted routine write without an approval requirement:** audit authority,
  idempotency, provider evidence, and finality normally; the absent approval is
  not a finding.
- **Raw token, key, or credential in the receipt:** emit
  `receipt.material.exposed` and recommend revocation/rotation.

## Output schema (`receipt_audit`)

```yaml
decision: ready | needs_more_evidence
run_ref: string
granted_scopes: [string]
exercised_scopes: [string]
refusals: [string]
findings:
  - id: string
    severity: error | warning | info
    message: string
verdict: clean | anomaly | needs_more_evidence
rationale: string
recommendations: [string]
success_checkpoint:
  milestone: string
  description: string
```

A `clean` verdict requires zero `error` findings.

## Worked example

A sealed run was granted `repo:read`. The receipt shows the acts exercised only
`repo:read` and material is referenced by hash. Exercised is a subset of
granted, no declared approval is unresolved, and no material is exposed:
`verdict: clean`. Had an act exercised `repo:write` while the proof
granted only `repo:read`, that would raise `receipt.authority.over_reach` and a
`verdict: anomaly` with a recommendation to revoke the run's grant and
investigate. Conversely, a run granted `repo:write` is not anomalous solely
because it has no approval record; approval is checked only if that exact act's
signed contract required it.

## Inputs

- `receipt_id` (optional): the receipt id to audit.
- `receipt_summary` (optional): a sanitized receipt or its acts/proof summary
  when the full receipt is not available.
- `granted_scopes` (optional): the authority the run was granted, when not
  derivable from the receipt alone.
- `objective` (optional): operator intent that focuses the audit.
- `receipt_rows` (optional): native-projection rows for deterministic replay;
  live runs resolve `receipt_id` from the configured receipt store.
- `receipt_details` (optional): native redacted detail projections for
  deterministic replay only.

At least one of `receipt_id` or `receipt_summary` is required; with neither, the
skill returns `needs_more_evidence`.

## Agent task contracts

### `receipt-audit`

Audit one run for authority over-reach, inconsistent declared approval, unrecorded refusal, or exposed
material. Native ledger evidence proves receipt identity, verification posture, signed
authority, approval decisions, acts, artifacts, and lineage through its redacted detail projection. Treat
receipt_summary and granted_scopes as supplemental operator context only. If a receipt id is not
resolved, or native detail is insufficient, return needs_more_evidence rather than clean. Do not
infer an approval requirement from a write-shaped verb. Do not repair the skill or mutate the ledger.
