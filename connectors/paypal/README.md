# PayPal

Create one merchant checkout order, capture it after the buyer approves it, and
refund an original capture. Each mutation requires exact native approval and
independent provider readback. These operations run through the same declared
HTTP implementation locally or with hosted credential custody.

Bind `paypal` with the selected REST app credential. Locally,
`PAYPAL_CLIENT_CREDENTIALS` holds a JSON object with `client_id` and
`client_secret`, or bind a secret (`runx secret set`), kept in runx or in a
secrets manager. OAuth
tokens remain inside credential custody; they never become skill inputs,
receipts or graph results. Every operation requires `api_host`: either
`api-m.paypal.com` or `api-m.sandbox.paypal.com`. The same admitted choice
controls the token endpoint, operation requests, relationship checks and
approval target. Bind the matching app credential for that environment.
Sandbox credentials never fall back to the live API.

Inspect the connector for the exact operation schemas. `account_id` is the
merchant's PayPal ID, not the REST app ID. Money inputs use a canonical decimal
string, currency code and precision. The schema fixes HUF, JPY and TWD at zero
decimal places and the other declared PayPal currencies at two. A refund's
native outgoing amount is derived exactly from that same decimal string and
precision; a caller cannot state a smaller amount to bypass a run budget.
There is no float arithmetic, rounding or implied FX conversion. A run with a
USD money limit still refuses an outgoing currency it cannot count.

`payment.paypal.order.create` confirms only that the order exists with the
requested merchant and terms. Its returned links include the provider's buyer
approval step. It does not claim the buyer approved or paid. `payment.paypal`
requires an `APPROVED` order with one purchase unit and checks its merchant,
reference, amount and currency before capture. Independent order and capture
reads must both confirm the original identity, exact terms and `COMPLETED`
state. Pending or declined captures do not confirm collection.

`payment.paypal.refund` reads the original capture before submitting a partial
refund. Its `original_status` pins the observed refundable state (`COMPLETED`
or `PARTIALLY_REFUNDED`). PayPal enforces the remaining refundable amount,
including concurrent refunds. Readback must confirm the refund's identity,
exact terms, `COMPLETED` state and original-capture relationship. The relationship
check compares complete provider link objects as a set; their array order
cannot change which capture the proof names.

Provider idempotency has a finite retention window. These operations do not
advertise unlimited safe resubmission. A recorded acknowledgement resumes at
readback without another write. An uncertain write stays uncertain and must be
reconciled on its original account and resource. A pending refund is not a
failed payment to resubmit. There is no cross-provider recovery fallback.
`payment.paypal.lookup` can observe a capture already recorded on a known order
after its submission answer was lost. Absence is not proof of non-submission.
An unknown refund without a provider refund ID needs provider reconciliation;
this connector does not invent an unlimited idempotency guarantee to replay it.

This is a merchant connector. It does not advertise PayPal buyer-wallet
capabilities or permit app credentials to spend from an arbitrary buyer.
Marketplace posting, fees, transfers and funding policy are separate concerns.

Run the package checks explicitly after building the native CLI and contracts:

```sh
node --test connectors/paypal/tests/connector.test.mjs
```

Package checks are separate from OSS core CI. Real sandbox proof is a separate,
opt-in provider exercise and requires valid PayPal sandbox app and buyer access.
The request shapes and money precision follow the [Orders and Payments
specifications](https://github.com/paypal/paypal-rest-api-specifications) and
[PayPal currency reference](https://developer.paypal.com/api/codes/currency).
