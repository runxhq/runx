---
name: refund
description: Refund one sealed payment through Runx Hosted with bounded authority, approval, idempotency, and readback.
---

# Refund

`refund` is the public reversal contract. It sends an opaque original receipt
reference, bounded refund request, complete parent authority, settlement family,
and stable idempotency key to Runx Hosted, then requires provider readback.

Runx Hosted resolves and verifies the original settlement, accounts for prior
refunds, attenuates authority, holds provider credentials, executes the reversal,
handles ambiguous outcomes, and records the private ledger. OSS contains none of
that implementation; it only governs the generic provider mutation and seals the
returned evidence.

## Operator guide

Use this skill only for a real, sealed original payment. Grants for
`payment.refund` and `payment.refund.read` are required. Stop on missing receipt
lineage, amount/currency/payer/rail drift, incomplete authority, reused
idempotency, or unavailable readback. Never accept raw provider credentials or
claim a refund from a request acknowledgement alone.
