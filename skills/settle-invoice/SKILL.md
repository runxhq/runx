---
name: settle-invoice
description: Validate and settle one invoice through Runx Hosted with approval and provider readback.
---

# Settle Invoice

`settle-invoice` is the public hosted contract for turning one invoice into one
bounded payment. It submits the invoice reference, exact minor-unit amount,
currency, opaque payee identity, selected hosted rail and profile, parent
authority, realm, and stable idempotency seed. It requires matching provider
readback before completion.

Runx Hosted owns invoice validation, payee and settlement resolution, authority
checks and reservation, provider credentials, rail execution, idempotency,
recovery, finality, and ledger state. OSS provides the discoverable contract,
generic provider gate, approval boundary, and receipt envelope only.

## Operator guide

Grants for `payment.invoice.settle` and `payment.invoice.read` are required.
Stop on malformed invoice data, non-positive minor units, invalid currency,
ambiguous payee identity, authority drift, unsupported rail, missing approval,
or inconsistent readback. Accept opaque settlement references only; never raw
account, routing, card, wallet, or provider credential material.
