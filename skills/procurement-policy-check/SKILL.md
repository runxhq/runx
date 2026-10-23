---
name: procurement-policy-check
description: Decide whether one proposed procurement request is allowed under an explicit procurement policy before any spend flow starts, refusing budget overages and unapproved payees instead of guessing.
---

# Procurement Policy Check

Procurement Policy Check is the pre-spend decision gate. It answers one
question, is this procurement request allowed under the supplied procurement
policy, and it answers it deterministically. It never moves money; `pay` and
the payment rails own execution, and this skill's sealed decision is what a
caller carries into that flow.

## Procedure

1. Native `data.digest` binds the exact procurement request and procurement
   policy before any judgment.
2. Deterministic rules apply in order: the payee must be in
   `approved_payees`, the request currency must match the policy currency, and
   the amount must fit both `remaining_budget` and `request_cap`. The
   first failed rule refuses with a named `refused_reason`.
3. An in-policy amount that reaches `approval_threshold` returns `needs_human`
   with the policy's `human_lane` (default `procurement-review`); approval below
   the threshold is `approved`.
4. A policy missing any required authority field refuses with
   `missing_policy_authority` and findings; nothing is inferred or invented.

`spend_executed` is always false. The decision packet carries both input
digests so a downstream spend can prove which request and policy were decided.

## Output

`procurement_approval` (`runx/procurement-policy-check.approval.v1`) carries `decision`
(`approved`, `refused`, `needs_human`), `reason`, `payee`, `amount`,
`currency`, `ceiling_amount`, `human_lane`, `refused_reason`, `policy_ids`,
`validation`, and the two digests.

Inputs are `procurement_request` and `procurement_policy`.
