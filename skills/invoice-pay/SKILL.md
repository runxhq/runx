---
name: invoice-pay
description: Bind one invoice to exact buyer payment terms, then use pay with native authority, approval and independent readback.
---

# Invoice Pay

Use this for an invoice whose reference, payee, amount and currency are already
known. Choose a held buyer connector: `wallet/evm` for its supported x402 method,
or `link` for a Stripe MPP charge with an approved one-time spend request.
There is no universal EVM address requirement, USD conversion or implicit
provider selection. Each method must actually support the invoice currency.

Supply `invoice_ref`, positive whole `amount_minor`, explicit `currency`,
`payee.payment_url`, the selected connector's public `request`, the original
`parent_payment_authority` and a stable `idempotency_seed`. Inspect the connector
to construct the exact request. For x402 the request is its `payment_signal`;
for Link it is the `payment.mpp` input with the original challenge and approved
spend request. Put wallet keys, SPTs, card data and bearer tokens in none of these
fields; credential custody belongs to the connector.

The deterministic invoice check binds the requested amount, currency and exact
payment URL to the supplied invoice. `pay` then checks its parent mandate and
owns payment execution. Native admission remains authoritative for capability,
run budget, human approval, effect custody and provider confirmation. A supplied
invoice or mandate never creates permission to spend. Invoice authenticity and
procurement approval must come from the caller's evidence or procurement lane.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `pay#pay`

## Completion and recovery

The result is `pay`'s original payment evidence. For x402, completion includes
confirmed payment and the paid resource's 2xx response. For Link, it confirms
the exact original paid amount and transaction; it does not publish a merchant
HTTP body or prove delivery. The outer receipt binds this evidence to the
caller-supplied invoice reference, not a claim that the processor authenticated
the invoice or fulfilled a separate contract.

Stop on mismatched invoice terms, unsupported method/currency, missing wallet
authority, denied approval or uncertain readback. Reuse the original invoice,
provider identifiers and request key. Resume the original effect or schedule its
readback; never pay again under a new identity. This skill does not mark an
accounting record paid, send a message or approve a purchase on the caller's
behalf. Those remain separate governed actions.
