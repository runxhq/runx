---
name: refund
description: Refund one original Stripe, PayPal or x402 merchant payment with exact native approval and independently checked settlement.
---

# Refund

Use this when you are the merchant reversing all or part of a confirmed payment.
Choose its original connector and account: Stripe, PayPal or Coinbase CDP for
x402. A buyer paying a merchant uses `pay`; collecting a payment belongs to
`charge`. A refund never changes processor to work around a failed readback.

Inspect `runx connector inspect <connector>` and use the refund operation's
`input_schema` to construct `request`. Supply original account/payment identity,
exact amount and currency, and a stable caller-owned `idempotency_seed`. PayPal
also requires an explicit API host, canonical decimal precision and the original
refundable capture status. Its sandbox host requires a matching sandbox app.
Never put provider keys, wallet keys, card data or tokens in these inputs.

## Confirm the original relationship

Native admission derives the outgoing amount from the exact provider request
before budgeting and approval. There is no rounding or FX conversion. A USD
limit refuses a currency it cannot count. The provider reads the original
payment on its account and atomically enforces the remaining refundable amount,
including prior or concurrent refunds. Local and hosted credential custody use
the same declared connector contract.

Stripe independently confirms the refund against the original PaymentIntent
and currency. PayPal independently confirms the original capture and exact
refund amount, currency and complete capture relationship. Coinbase CDP retains
its original settlement evidence and separate refund readback. Only confirmed
`succeeded`, `COMPLETED` or x402 `settled` evidence completes the respective
lane. The result is that provider's bounded native packet, not a fabricated
universal payment record.

## Unknown and pending outcomes

A provider acknowledgement or HTTP success is not a completed refund. Stop on
unknown payment, wrong account/currency, excess remaining amount, denied
approval, pending/refused refund or unavailable readback. Preserve the original
provider reference, request key and native receipt. Resume the original effect
at readback; do not submit a second refund under another key. An unknown write
without an acknowledgement needs reconciliation, not a new financial action.

A composed caller supplies the same original collection evidence and key. This
skill does not collect again, replace the original payment, send a customer
message or update a ledger on the caller's behalf.
