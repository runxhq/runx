---
name: spend
description: Execute one governed outbound payment through Runx Hosted with approval and provider readback.
---

# Spend

`spend` is the canonical public buyer-side payment contract. The single default
runner submits an exact payment signal, full parent authority, explicit hosted
rail, opaque rail profile, optional opaque admission material, realm, and stable
idempotency seed to Runx Hosted. It closes only after provider readback matches
the mutation result.

Runx Hosted owns quote validation, authority attenuation and reservation,
aggregate limits, approval admission, credential custody, rail execution,
idempotency, recovery, finality, and the private payment ledger. The OSS runtime
owns only generic provider permission enforcement and receipt sealing. There is
no local real-payment implementation and no automatic mock fallback.

## Operator guide

Use `charge` for seller-side paid calls, `refund` for reversals, and
`settle-invoice` when an invoice first needs hosted validation. The selected
hosted connector needs `payment.spend` and `payment.spend.read` grants.

Stop on malformed or ambiguous signals, missing complete authority, absent rail
profile, cap or binding drift, missing approval, unavailable hosted execution,
or inconsistent readback. Never put card data, wallet keys, provider keys,
tokens, or arbitrary endpoints in skill input.
