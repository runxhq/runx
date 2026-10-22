# Stripe

Collect a payment using a payment method already available to your Stripe
account, read its processor status, and refund the original PaymentIntent.
The same declared HTTP operations run locally or through hosted credential
custody. Marketplace funding, application fees and transfers are not part of
these operations.

Bind the connector with `runx connector use stripe`. Its local credential is
read from `STRIPE_API_KEY` when set, or from the selected secret profile or
secret-manager reference. A hosted instance uses its own selected grant.
Use `runx connector inspect stripe --json` for the exact input contracts.

Every payment call names `account_id`. A credentialed account read must match
that id before collection, payment reads or refunds proceed. Every provider
request also carries that exact `Stripe-Account` header, so credential rotation
between checking reads and submission cannot silently switch the merchant.
Prices are exact
integer currency units as Stripe defines them; `2500` with `currency: aud`
means AUD 25.00. Currency codes in these provider inputs are lowercase.

`payment.stripe` creates and confirms a PaymentIntent with the approved amount,
currency, payment method and caller reference. Collection confirms only after
an independent read matches those terms, the received amount and `succeeded`
status. A response requiring customer action or still processing does not
confirm payment. The projection retained for recovery excludes client secrets
and payment-method details.

`payment.stripe.refund` reads the original PaymentIntent and checks its currency
and status before requesting a partial refund. Stripe enforces the remaining
refundable amount, including concurrent requests. An independent refund read
must match the original payment, requested amount, currency and `succeeded`
status. A refund carries the native outgoing amount; collection does not debit
the caller's outgoing budget.

Stripe retains idempotency keys for a limited time, so these operations do not
claim unlimited safe resubmission. A stored acknowledgement resumes at readback.
A lost acknowledgement remains unknown and is not resubmitted automatically.
`payment.stripe.lookup` can locate the original collection by its native effect
key. Search is eventually consistent: an absent result never proves no payment
occurred. Multiple matches refuse. Use `payment.stripe.read` or
`payment.stripe.refund.read` to observe an identified processor object.

This connector does not advertise buyer-wallet capabilities. Link authorization,
SPT acceptance and MPP charging require their own completed integration and are
not implied by the Stripe mark or by these direct-payment operations.

Run this package's explicit contract checks after building Runx and the contract
package:

```sh
node --test connectors/stripe/tests/connector.test.mjs
```

These checks inspect the actual connector through the native CLI and verify
its provider requests. They are deterministic contract tests, not sandbox
transactions, and are separate from OSS core test gates.

For an explicit provider proof, set `STRIPE_TEST_SECRET_KEY` in an ignored env
file and run:

```sh
node --env-file=<test-env-file> connectors/stripe/tests/sandbox.mjs <new-evidence-file>
```

This refuses live keys. It collects USD 1.50 in test mode, verifies independent
readback and duplicate submission, competes two partial refunds against the
remaining amount, and finishes by refunding the entire test payment. Only
bounded payment and refund references, amounts and check results go into the
evidence file. An interrupted attempt must be reconciled using those references;
the script refuses to overwrite its evidence or silently retry a lost write.
It exercises the declared provider requests; it does not substitute for native
approval, custody or receipt tests. The stable Stripe API version is pinned in
the connector manifest for every request.
