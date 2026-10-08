---
name: charge
description: Verify and settle one provider-side paid call through Runx Hosted with approval and provider readback.
---

# Charge

`charge` is the public seller-side contract for a paid operation. Its OSS graph
is deliberately thin: it submits the exact tool call, hosted pricing policy,
opaque returned credential reference, verification capability reference, and
idempotency seed to the approved Runx Hosted payment operation, then requires a
provider readback before the run can close.

Runx Hosted owns pricing validation, challenge and credential semantics,
settlement admission, provider credentials, rail execution, idempotency,
recovery, and the private payment ledger. The OSS runtime owns the generic
provider-effect gate and receipt envelope. It does not implement a payment rail
or infer settlement from caller input.

## Operator guide

Use this skill when Runx is the provider of a paid operation. Use `spend` on the
buyer side and `refund` for a receipt-linked reversal. A hosted connector and
grants for `payment.charge` and `payment.charge.read` are required.

Stop on missing or ambiguous policy, credential reference, capability,
counterparty, or idempotency material. Refuse raw card, wallet, provider-key, or
bearer material. Never release the paid operation unless hosted execution and
readback agree on a terminal successful charge.
