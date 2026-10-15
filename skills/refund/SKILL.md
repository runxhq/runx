---
name: refund
description: Refund all or part of one settled payment on its original rail, with approval, idempotency, and readback.
---

# Refund

`refund` is the reversal contract. It returns all or part of one settled
payment to its payer, on the rail the payment was settled on, then requires a
readback of the same refund, settled for that amount against that payment,
before the run closes.

The payments `charge` settles are x402, through Coinbase CDP
(`coinbase/cdp`), so their refunds are too. Runx Hosted resolves the original
settlement, accounts for prior refunds, holds the CDP credential, broadcasts
the reversal, and carries an ambiguous outcome to a terminal one. The amount
the refund moves counts against the run's authority like any payment. OSS
implements none of the rail; it governs the provider mutation and seals the
returned evidence.

## Operator guide

Use this skill only for a payment `charge` settled. A hosted `coinbase/cdp`
grant is required. Pass the paid resource, the payment's reference, the amount
and currency, a reason, and a stable idempotency seed. Stop on an unknown
payment, an amount over what remains refundable, currency drift, reused
idempotency, or unavailable readback. Never accept raw provider credentials or
claim a refund from a request acknowledgement alone.
