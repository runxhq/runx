---
name: charge
description: Collect one exact payment on your selected Stripe, PayPal or x402 merchant account, with native approval and independent readback.
---

# Charge

Use `charge` when you are the seller and need to collect a payment. Choose the
merchant connector explicitly: Stripe collects an existing payment method, PayPal
captures a buyer-approved order, and Coinbase CDP settles the buyer's x402
authorization against its original quote. The selected connector instance fixes
the account; an unavailable account never selects another method.

Run `runx connector inspect <connector>` to read the exact operation input schema.
The default runner takes that public `request`, the connector name and a stable
`idempotency_seed`. Its examples show each supported method. Provider-specific
arguments stay with their owning connector, rather than becoming another payment
API in the skill. Never supply card data, SPTs, wallet keys, API keys or bearer
credentials. Credentials belong to the connector's configured custody.

## Collect and confirm

Supply the exact account, amount, currency, reference and existing payment
identifiers. The native operation owns admission, human approval, the provider
request key, acknowledgement custody and readback. Stripe must independently
return `succeeded` with the approved amount received; PayPal must independently
return `COMPLETED` for the original merchant, order, amount and currency. The
x402 lane retains its separate read of the original settled payment and returns
the original quote, resource and settlement evidence. Incoming collection does
not consume an outgoing spending allowance.

The result remains the selected provider's bounded native operation packet.
Release the resource only when the exact payment is confirmed. A prepared order,
buyer approval, submitted request, HTTP success or sealed plan does not establish
settlement. Pending, refused, mismatched or unknown outcomes stop that release.

## Prepare a buyer's payment

`quote` retains the x402 requirements for one exact resource and price; it takes
no payment. `checkout` creates and independently reads one PayPal order and
returns its buyer-action links. Creation is a provider mutation, not buyer
approval or capture. Continue the approved order through the default PayPal
collection lane with its original order identity and terms. Stripe collection
requires an existing permitted payment method; this skill does not collect card
details or impersonate the buyer. MPP purchase collection belongs to the
existing purchase boundary and is not claimed as a standalone merchant lane here.

## Continue safely

Stop on a missing binding, changed account or terms, denied approval, or uncertain
readback. Resume or inspect the original native effect and payment identifiers;
retain the original request key. A response lost after submission is not a
reason to create another checkout, authorization, payment method or charge.

A buyer uses `pay`; invoice validation belongs to `invoice-pay`. To reverse a
confirmed collection, use `refund` with its original processor, account, currency
and payment reference. Those are separate governed actions and are not performed
by this skill.
