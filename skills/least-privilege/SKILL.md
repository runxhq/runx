---
name: least-privilege
description: Verify the receipts behind a grant, compare the scopes they exercised with the granted scopes, and propose the narrowest grant the evidence supports.
---

# Least Privilege Auditor

Turn granted authority plus attributable observed usage into a bounded
attenuation proposal.

This skill resolves every supporting receipt id through the native `ledger`
runner before comparing exercised scopes from Runx's redacted receipt detail
with the current grant. Native detail is authoritative for scope use. Missing
receipt proof defers every scope. The output is a
reviewable attenuation proposal, not an automatic change.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `ledger#read`

## What this skill does

1. Diff granted authority against receipt-backed usage.
2. Classify each granted scope as `keep`, `narrow`, `remove`, or `defer`.
3. Propose the narrowest grant that still covers observed usage.
4. State residual risk after attenuation.
5. Emit a receipt-quality report a reviewer can apply or reject.

## When to use this skill

- Periodic least-privilege review of a skill, grant, or principal before
  publish, renewal, or maturity promotion.
- After an incident, to identify authority that can be safely removed without
  breaking observed behavior.
- Before expanding distribution of a public skill, to prove its grant is
  minimal against real receipts.
- When a reviewer asks for a scope-by-scope evidence trail, not just a summary.

## When not to use this skill

- To grant new authority. This skill only narrows; widening is a human
  decision.
- When no usable receipt evidence exists. Return `needs_more_evidence` rather
  than guessing a grant down to nothing.
- For secret material handling or credential exposure. Use the appropriate
  secret-leak triage flow instead of scope review.
- When the user asks for automatic permission changes. Produce a proposal and
  stop unless a separate approved delivery lane exists.
- When grant semantics are unknown and cannot be normalized. Return
  `needs_input` with the exact syntax or policy question.

## Procedure

1. Scope the audit target.
   - Identify `subject`, grant source, receipt ids or receipt window, and
     whether receipts are from the same principal or skill version.
   - Gate: if the subject, grant list, or usage source is ambiguous, stop with
     `needs_input`.
   - Evidence expected: subject id or label, granted scope list, receipt ids or
     an explicit statement that no receipts were available.

2. Normalize granted scopes.
   - Parse each scope into verb, resource, path or namespace, conditions, and
     wildcard breadth.
   - Preserve original scope strings. Do not rewrite policy syntax casually.
   - Gate: if a scope cannot be parsed, keep it as `defer` and request the
     missing policy semantics instead of treating it as unused.

3. Build the usage model from attributable evidence.
   - Resolve each supporting receipt id through `ledger read`.
   - Read exercised scopes and receipt references from native redacted receipt
     detail.
   - Count successful use separately from denied or dry-run checks.
   - Do not infer scope usage from a successful high-level task alone; cite the
     receipt step or policy check that exercised the authority.

4. Classify every granted scope.
   - `keep`: at least one observed successful use requires the granted scope as
     written, or a reserved/break-glass policy explicitly requires it.
   - `narrow`: all observed uses fit a strictly smaller verb, resource,
     namespace, condition, or path.
   - `remove`: no observed use, denied check, or documented reserved purpose
     supports the scope.
   - `defer`: evidence is conflicting, receipt attribution is weak, or policy
     semantics are unknown.

5. Propose attenuation.
   - Remove scopes classified as `remove`.
   - Downgrade scopes classified as `narrow` only when every observed use fits
     the narrower grant.
   - Leave `keep` and `defer` scopes unchanged in the proposed grant.
   - Gate: never produce a proposal narrower than the evidence supports. A
     scope used once is used.

6. State residual risk and reviewer action.
   - Name what the proposed grant can still do.
   - Name any broad scope kept despite thin evidence and why.
   - Separate `applyable now` from `needs human policy decision`.

7. Emit receipt expectations.
   - A valid receipt for this skill should record input grant count, receipt
     sources, classification counts, proposed removals or narrowings, stop
     status, and unresolved questions.

## Edge cases and stop conditions

- Empty or unattributable usage evidence: return `needs_more_evidence`; do not
  remove all scopes by default.
- Missing granted scopes: return `needs_input`; there is no baseline to diff.
- Receipt subject mismatch: return `needs_input` with the mismatched subject or
  version.
- Conflicting receipts: classify affected scopes as `defer` and return
  `needs_human` if the conflict changes the proposal.
- Wildcard grants: narrow only to observed resource prefixes when receipt
  coverage is representative; otherwise keep and flag residual risk.
- Reserved, compliance, or break-glass scopes: keep unless the operator
  provides explicit policy authority to remove them.
- Dry-run-only use: do not count as successful exercised authority unless the
  grant exists solely for validation.
- Grant already matches usage: return `no_change` with the evidence summary.
- User asks to hide or omit unused authority: refuse that part and report the
  complete scope diff.

## Output schema

Return a structured report with these fields:

```yaml
status: attenuation_proposed | no_change | needs_more_evidence
subject: string
evidence:
  receipt_ids: [string]
  grant_source: string | null
  limitations: [string]
scope_diff:
  - granted_scope: string
    normalized:
      verb: string | null
      resource: string | null
      conditions: object | null
    observed_use:
      count: number
      verbs: [string]
      resources: [string]
      receipt_refs: [string]
    classification: keep | narrow | remove | defer
    proposal: string | null
    rationale: string
attenuated_grant: [string]
removed_scopes: [string]
narrowed_scopes:
  - from: string
    to: string
kept_scopes: [string]
deferred_scopes: [string]
residual_risk: [string]
reviewer_action: applyable_now | gather_more_receipts | none
receipt_expectations:
  classification_counts: object
  stop_status: string
  unresolved_questions: [string]
```

## Worked example

Input:

```yaml
subject: skills/report-exporter
granted_scopes:
  - drive.files.read:/reports/*
  - drive.files.write:/reports/*
  - drive.files.delete:/reports/*
receipt_ids: [rx_101, rx_102]
```

The two receipts' native detail shows `drive.files.read:/reports/*` exercised
eight times and `drive.files.write:/reports/*` twice.

Output:

```yaml
status: attenuation_proposed
subject: skills/report-exporter
removed_scopes:
  - drive.files.delete:/reports/*
narrowed_scopes: []
kept_scopes:
  - drive.files.read:/reports/*
  - drive.files.write:/reports/*
attenuated_grant:
  - drive.files.read:/reports/*
  - drive.files.write:/reports/*
residual_risk:
  - The skill can still read and write any file under /reports/*.
reviewer_action: applyable_now
```

The delete scope is removable because no cited receipt exercised delete
authority. The read and write scopes stay because each was used at least once.

## Inputs

- `subject` (optional): skill id, grant id, principal, or other label for what
  is being audited.
- `granted_scopes` (required): the current scopes granted to the subject,
  preferably in canonical policy syntax.
- `receipt_ids` (required): exact receipt ids supporting the audit.
- `receipt_rows` (optional): native-projection rows for deterministic replay;
  live runs resolve `receipt_ids` from the configured receipt store.
- `receipt_details` (optional): native redacted detail projections for
  deterministic replay; live runs resolve them from `receipt_ids`.
- `objective` (optional): operator intent that focuses the review, such as
  "prepare for public publish" or "post-incident attenuation".
- `policy_notes` (optional): reserved scopes, compliance constraints, or
  human-approved exceptions that affect removal decisions.
