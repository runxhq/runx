---
name: invoice-pay
description: Bind one invoice to exact buyer payment terms, then use pay with native authority, approval and independent readback.
---

# Invoice Pay

The default method pays an invoice whose reference, payee, amount and currency
are already known; `from-document` reads them from the invoice itself. Choose a
held buyer connector: `wallet/evm` for its supported x402 method, or `link` for a
Stripe MPP charge with an approved one-time spend request. There is no universal EVM address requirement, USD conversion or implicit
provider selection. Each method must actually support the invoice currency.

Supply `invoice_id`, positive whole `amount_minor`, explicit `currency`,
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

## Pay from the invoice document

Use `from-document` when you hold the invoice as text. Supply its workspace
`path`, `connector`, parent mandate, `idempotency_seed` and the method's `request`
without the terms the document states: for `wallet/evm` leave out `amount_minor`
and `resource.url`, and for `link` leave out `resource_url`, `amount_minor` and
`currency`. Restating them is refused, so the amount paid cannot drift from the
amount read. Convert a PDF or image to text first.

`extract` reads the balance due, currency, reference and HTTPS payment URL, each
bound to a quotation. A native conversion then turns the decimal into the unit
the selected method counts in, with no rounding. That unit comes from the method,
never from the caller or the document: both held methods bind USD to cents here,
so another currency stops and points back to the default method with exact minor
units. It also stops when the document omits a term or states two, or when the
amount has the wrong number of fractional digits.

The extracted amount is a model's reading of the document, not independent
financial proof. Your parent mandate still bounds the exact URL and maximum, and
native approval shows the exact amount before anything moves. The result returns
the terms and their quotations beside `pay`'s evidence. An invoice that offers
only bank or card details has no payable HTTPS resource, so it stops here.

## Composes

<!-- Generated from the native closure; run pnpm core-skills:composes:generate. -->

- `extract#from-file`
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
