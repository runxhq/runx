# Link

Use a local Link wallet to request a bounded, one-time payment authorization and
pay a Stripe MPP resource. The official Link CLI owns login, refresh and SPT
retrieval. Runx never reads its authentication file or exports its tokens.

Install `@stripe/link-cli@0.29.0`, authenticate for agent payments, then run
`runx connector use link --session`. Runx pins the stable account id returned by
`link-cli user-info retrieve`. A CLI that cannot return a stable id is refused.
Each operation checks that account again; the payment process also reads the
original, account-owned spend request immediately before use. A changed login
cannot substitute another account's wallet or spend request.

`spend.request` requests an exact amount, currency, payment method and MPP
network under the native effect key. Present its returned approval URL to the
person. Pending wallet approval, approved spending authority and settled payment
are different states. This operation does not pay a merchant.

`payment.mpp` accepts a one-time approved spend request and its exact MPP charge
challenge, destination and request. Runx owns the payment approval and outgoing
budget. The connector checks the terms before the official CLI privately obtains
the SPT. The CLI pins the challenge and destination and refuses redirects.
No credential is returned to a graph or supplied in a graph input.

A resource's HTTP response is not settlement proof. Confirmation independently
reads the original Link spend request and requires a successful payment
observation for the exact amount/currency and a transaction reference. Pending,
failed or mismatched observations cannot confirm payment. Public outputs contain
bounded ids, terms and status, never the SPT or the resource's raw headers/body.

Create an authorization through `spend.request` with the original resource URL,
method, body and provider-issued challenge as well as its monetary terms. The
connector binds those exact bytes and the pinned account in Link's existing
spend-request metadata. Both approval readback and payment verify that binding.
An old authorization without it, or one naming another resource/body/challenge,
cannot be relabeled as this payment even when the merchant and price match.

The connector does not claim unlimited idempotency. If a submission response is
lost, retain the original spend request and inspect it with `spend.read`. Never
create a replacement request or credential to recover an uncertain payment.
A readback-confirmed original payment can be observed without paying again.

The local connector does not make Link a hosted credential and does not yet add
Link to native paid-skill purchase choices. That integration must preserve the
purchase's one approval, reservation and custody record. This package's tests
are deterministic driver proof; actual Link proof needs an authenticated wallet.

Run the explicit package checks with:

```sh
node --test connectors/link/tests/payment.test.mjs
```
