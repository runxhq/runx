---
name: least-privilege
description: Review a grant against exact attributable verified receipts, identify recorded and unrecorded authority, and state the policy and usage evidence needed before narrowing it.
---

# Least Privilege

Review one skill, actor or grant against a bounded set of exact receipts. The
result distinguishes authority recorded in signed receipts from authority not
recorded in that selection, and gives the reviewer a clear next action. It
never changes a grant.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `receipt-query#read`

Native Runx owns receipt lookup, signature verification and authority semantics.
This skill binds the review subject, checks the completeness of the supplied
proof and organizes the authority evidence. It does not implement a scope
parser, wildcard matcher, permission hierarchy or policy engine.

## What the evidence can establish

The native field named `exercised_scopes` currently projects signed scope
references and authority terms. Its `source` identifies that origin. Those
records are not a per-operation successful-usage trace. Counts in this report
count receipts recording an exact scope, not successful calls.

A scope missing from a finite receipt window may still support infrequent,
reserved, recovery or compliance work. Even a recorded scope is not proved
necessary. `repo:write` does not imply `repo:read`, and a wildcard cannot be
replaced by the first matching string. Scope strings remain opaque here.

The current native evidence is insufficient to prove a minimal replacement
grant. The runner therefore retains the entire grant, emits no executable
attenuation proposal, and requests policy and representative usage review.
A trustworthy narrowing proposal requires the authority owner to establish
coverage and replacement semantics; do not add that mechanism to this skill.

## When to use it

- Review authority before renewal, distribution or an incident follow-up.
- Collect exact receipt references for a scope-by-scope policy decision.
- Identify where actual usage or reserved-purpose evidence is still missing.

Use `receipt-audit` to assess governance within one run, `policy-draft` to draft
and lint a policy, and `diagnose-skill-run` to diagnose an execution failure.
This skill neither grants new authority nor substitutes for those jobs.

## Procedure and stops

1. Supply the exact subject reference, current grant and exact receipt ids.
   `subject` must match each receipt's skill subject, actor reference or grant
   reference. A friendly label alone cannot establish attribution.
2. Resolve evidence through `receipt-query#read` with native chain verification.
   Every requested id needs one verified, attributable native detail. Missing,
   duplicate, unverified or mismatched details stop the comparison. Unrelated
   receipt rows never contribute authority observations.
3. Require a checked, intact native chain verdict with no breaks. Supplied
   replay rows cannot provide this proof even if their fixture labels say
   verified. Unknown verification remains unknown.
4. Compare exact scope strings only. Mark each as `recorded`, `not_recorded`
   or `defer`, preserving the receipt references and native source labels.
5. Retain the grant. With complete proof return `review_required`; with
   incomplete proof return `needs_more_evidence`. Name the missing policy,
   actual usage and observation-coverage evidence before any narrowing.

A checked chain, a sealed review receipt and an empty set of changes do not
prove that a grant is minimal. Do not describe either status as safe to apply.

## Inputs and result

- `subject`: exact skill, actor or grant reference being reviewed.
- `granted_scopes`: current scope strings, including an empty grant.
- `receipt_ids`: exact receipt ids; an empty set stops for evidence.
- `objective`: optional review purpose; it does not supply policy authority.
- `receipt_rows` and `receipt_details`: supplied projections for replay only.

`audit_report` returns the status, subject, completeness and chain verdict,
limitations, scope-by-scope recorded authority, `retained_grant`,
`not_recorded_scopes` and `reviewer_action`. `attenuation_proposals` stays empty
until native evidence can support a replacement. `verdict` is a short operator
summary. The existing `runx.security.least_privilege.v1` packet is updated in
place; there is no alternate contract or compatibility reader.

For a verified review of `email:send` and `payment:spend`, a receipt recording
only `email:send` yields `recorded` and `not_recorded`, respectively. Both remain
in the grant. The missing payment record is a review question, not permission
to remove payment authority.
