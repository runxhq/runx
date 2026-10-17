---
name: invoice-pay
description: Check one invoice against the authority that may pay it, then pay it through pay with approval and readback.
---

# Invoice Pay

`invoice-pay` turns one invoice into one bounded payment. It checks the
invoice first: a positive amount in minor units, a currency a payment method
settles, the payee's exact HTTPS payment endpoint and address, and a parent
authority that covers the amount and names that payee. Only then does it
compose `pay`, which settles the payment once and reads the paid resource
back.

The check is deterministic and runs before anything is approved or paid. A
payment method settles USD today, so an invoice in another currency is refused,
never converted. Everything after the check is `pay`'s: hosted wallet custody,
approval, idempotency, recovery and finality.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `pay#pay`

## Operator guide

Pass the invoice reference, the amount and currency due, the payee's
`payment_url` and `pay_to`, the parent's authority, and a stable idempotency
seed. Stop on malformed invoice data, a non-positive amount, an unsupported
currency, a payee the authority does not name, an amount over the authority,
missing approval, or inconsistent readback. Accept the payee's public payment
endpoint and address only; never account, routing, card, wallet, or provider
credential material.
