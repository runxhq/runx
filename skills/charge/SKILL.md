---
name: charge
description: Quote, then verify and settle, one buyer's payment for a resource you sell, with approval and provider readback.
---

# Charge

`charge` is the seller contract. `quote` states the payment requirements for
one HTTPS resource at an exact price. The default `charge` runner settles the
buyer's payment against those requirements once, then requires a readback of
the same payment, settled at that price for that resource, before the run can
close and the resource is released.

The rail is x402, settled through Coinbase CDP (`coinbase/cdp`) in the
seller's own account. Runx Hosted verifies the payment payload with the
facilitator, settles it, keeps the attempt idempotent and recoverable, and
holds the CDP credential. The OSS runtime owns the generic provider-effect gate
and the receipt envelope; it implements no rail and infers no settlement from
caller input.

## Operator guide

Use this skill when you are the seller. Use `pay` on the buyer side and
`refund` to reverse a settled charge. A hosted `coinbase/cdp` grant is
required.

Pass the buyer's payment payload and the quoted requirements exactly as they
arrived, with the digest that binds the paid request, and a stable idempotency
seed. Stop on missing or mismatched requirements, resource, amount or currency.
Refuse raw card, wallet, provider-key, or bearer material. Never release the
resource unless settlement and readback agree on a settled payment.
