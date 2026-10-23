---
name: sign-receipt
description: Bind an off-runtime action claim to opaque evidence locators in a signed Runx receipt without pretending Runx verified the external action.
---

# Sign Receipt

Record a bounded attestation about something that happened outside Runx so a
later governed run can cite exactly what was claimed, by whom, on which evidence,
and for what reliance scope. The native Runx receipt signature proves the
attestation packet was sealed without later modification. It does **not** prove
the external action itself happened.

That distinction makes this skill useful without turning a signature into a
fictional provider verifier. The actual proof remains in the referenced commit,
provider object, approval record, transaction, or other evidence system.

## When to use it

Use `sign-receipt` when an operator performed a legitimate off-runtime action
and downstream automation needs a stable, auditable assertion about it. Examples
include a manual console change, an externally approved decision, or a provider
operation completed before a native adapter existed.

Do not use it to launder an unsupported claim, copy raw evidence into a receipt,
or replace a provider readback surface that Runx can actually call. If the
external record is available through a governed reader, prefer that stronger
evidence path.

## How it works

1. Supply the action, named principal, exact claim, optional reliance scope, and
   bounded opaque evidence locators.
2. Each evidence item names only a stable locator, SHA-256 digest, and what that
   evidence is asserted to prove. Raw messages, receipts, secrets, and personal
   records are not accepted.
3. Native `receipt.attest` validates completeness, rejects duplicates and
   secret-shaped material, and computes a deterministic attestation digest.
4. The normal Runx receipt signer seals that packet into the parent run.
5. Later consumers can verify the receipt and decide whether its declared
   reliance scope is sufficient for their own action.

The skill never calls the provider named in the claim and never appends to an
external ledger. Its signature mode and proof boundary are explicit in the
result.

## Inputs and result

- `action` describes the external act without overstating its result.
- `principal` identifies the actor named by the attestation.
- `claim` is the exact assertion a later run may rely on.
- `evidence` contains at most the declared number of `{locator, digest, proves}`
  records using lowercase `sha256:<64 hex>` digests.
- `scope` narrows who or what may rely on the claim.

The `runx.attestation.v1` packet returns `ready_to_seal`, `needs_agent`, or
`needs_more_evidence`, a deterministic digest, redactions, and the explicit
statement that no provider or external ledger was called. The sealed parent
receipt—not the ready plan—is the signature evidence.

## Stop conditions

- Stop when action, principal, claim, or evidence identity is missing.
- Reject malformed or duplicate locators and digests, raw evidence bodies,
  secret-shaped strings, or evidence that cannot say what it proves.
- Do not claim independent verification, provider acknowledgement, delivery,
  settlement, or external ledger append.
- Keep reliance scope narrow; a downstream skill still evaluates whether the
  attestation is adequate for its own consequential action.

## Example

An operator manually changed a DNS record and stores the provider audit-event
locator plus a digest of the exported record. The skill can seal the claim “record X
was changed by principal Y” with those opaque refs. It cannot claim DNS
propagation or provider verification unless the cited evidence and a later
reader actually prove them.
