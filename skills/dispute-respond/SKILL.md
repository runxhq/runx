---
name: dispute-respond
description: Prepare and explicitly file an evidence-bound payment dispute response using native receipt proof, approval, provider execution, and independent readback.
---

# Dispute Respond

Turn one provider dispute event and the exact Runx payment history behind it
into a reviewable response posture. A good dispute packet does not merely tell a
persuasive story: it proves which charge and refund receipts were consulted,
which external evidence may be cited, what remains unknown, and why the operator
should accept, contest, review, or acknowledge an existing refund.

The default `file-response` runner prepares the evidence-bound response, files
it through the selected configured provider after one exact approval, and
requires independent readback. `respond` remains the explicit draft-only
runner, while `file` consumes an admitted prior response packet without
repeating the evidence work. No runner closes a dispute, moves money, or
invents provider acceptance.

## When to use it

Use `dispute-respond` after a provider has created a bounded dispute event and
the original charge should exist in Runx receipt history. Use `refund` when the
operator chooses a new reversal rather than a dispute response. If the original
settlement cannot be proven, gather that evidence before contesting.

## How it works

1. Normalize provider, dispute, charge, amount, currency, reason, and optional
   response deadline.
2. Resolve the exact original charge and every prior refund by content-addressed
   Runx receipt id. Caller-supplied receipt rows or summary projections are not
   accepted as substitutes.
3. Require complete production-signature verification and redacted native
   detail for each linked receipt tree.
4. Admit external evidence only as bounded ref, SHA-256 digest, kind, and
   summary. Raw provider payloads and credentials remain outside the packet.
5. Draft one posture using only admitted receipt ids and evidence refs.
6. Deterministic finalization rejects invented citations, effect claims, and
   any completion state the evidence cannot support.
7. When the operator deliberately invokes `file`, Runx validates the unchanged
   ready packet, binds it to a SHA-256 digest, obtains approval, and calls the
   configured provider under the narrow `dispute.file` scope.
8. Runx projects only the filing reference, dispute id, request digest, and
   status from the provider result, then performs an independent
   `dispute.read` and requires those fields to match before reporting success.

A contest posture must cite delivery or consent evidence. A verified prior
refund forces `refund_already_sent` or `operator_review`; the model cannot ignore
it to produce a stronger contest narrative.

## Inputs and result

The input includes the normalized dispute event, exact original receipt id,
prior refund ids, and bounded external evidence. The result contains the
posture, response summary, cited receipt ids and evidence refs, open questions,
validation, and an exact provider-filing handoff for later review.

`ready_for_review` means only that the packet is complete enough for an
operator. The preparation result always reports `filing.status: not_filed`,
`provider_status: not_called`, and `approval_status: not_requested`.

Use the named `file` runner only after reviewing that packet. Supply the exact
packet, the configured provider name, and one stable idempotency key for that
filing. Runx injects the key into the provider request, rejects a conflicting
nested key, refuses a packet whose decision or filing state has drifted, and
does not accept the mutation response as proof by itself. A successful run must
also return matching `dispute.read` evidence. Provider credentials remain in
the configured adapter boundary; they never belong in skill inputs or output.

## Stop conditions

- Return `needs_more_evidence` for missing, invalid, local-development, or
  incompletely verified receipt trees.
- Refuse caller-authored receipt detail, raw provider evidence, credentials, or
  citations not present in the admitted set.
- Do not contest without evidence of delivery, consent, or the provider-specific
  fact on which the response depends.
- Do not file a packet that is incomplete, failed validation, already marked as
  filed, or different from the packet approved by the operator.
- Do not claim filing from the mutation response alone. Require the independent
  provider readback, and do not equate a filed response with dispute acceptance,
  closure, settlement, or a refund.

## Example

A dispute alleges an unrecognized charge. Native history proves the charge but
contains no consent evidence. The packet may recommend `operator_review` and
name the missing evidence; it should not contest by inference. If a verified
refund receipt already covers the amount, the posture becomes
`refund_already_sent` and cites that exact receipt. If the operator later invokes
`file`, the approved packet—not a rewritten summary—is what reaches the provider,
and matching provider readback is the completion proof.

## Agent task contract

### `dispute-response-draft`

Choose `accept`, `contest`, `refund_already_sent`, `needs_more_evidence`, or
`operator_review` from the admitted dispute, evidence refs, and native redacted
receipt detail. Cite only exact admitted ids and refs. Never claim filing,
provider acceptance, closure, settlement, or a refund not proven by the packet.
