---
name: pay
description: Pay one exact resource with your held x402 wallet or approved Link MPP authorization, within the supplied terms and native authority.
---

# Pay

Use this when you are the buyer paying a known merchant resource. Choose the held
connector explicitly: `wallet/evm` pays external x402 V2 with USDC on Base;
`link` pays a Stripe MPP charge using an already approved one-time Link spend
request. Stripe and PayPal merchant app credentials cannot spend as a buyer.
Missing authority never selects another wallet or method.

Inspect the selected connector to construct its public `request`: the wallet's
`payment_signal`, or Link's `payment.mpp` arguments. Supply the parent mandate's
original term reference, exact resource URL, currency and maximum, plus a stable
`idempotency_seed`. These purchase terms are checked before financial dispatch;
they do not grant native authority. The native kernel still admits capability,
run budget, human approval and the exact effect. No rounding or FX is implied.

## x402 resource payment

The default composes `settle` once, `readback` and the terminal evidence check.
The hosted wallet owns private signing, staged-byte recovery, on-chain finality
and the paid resource result. A complete outcome requires a confirmed chain
transaction and the resource's 2xx response; the resource's own product lifecycle
may continue afterwards. Pending, failed or mismatched evidence never reports a
completed purchase. Durable callers may use the x402-specific `settle` and
`readback` runners separately and schedule later observations externally.

## Link authorization and MPP payment

Install the pinned official Link CLI and bind the intended signed-in account
with `runx connector use link --session`. Run `authorize` with the connector's
`spend.request` arguments to request exact one-time wallet consent. Its approval
URL/status is authorization evidence only; it does not prove payment. The connector binds consent to the original URL, method, body and challenge in
Link metadata; an unrelated or unbound authorization is refused. Complete
the wallet's approval, then supply that original spend request ID and original
provider-issued MPP challenge to the default Link payment lane.

The connector pins and rechecks the account, amount/currency, payment method,
merchant network and challenge. Only the official CLI retrieves the SPT; it
never enters graph inputs, results or receipts. Independent original spend
request readback must confirm the exact paid amount and transaction. This lane
returns payment evidence, not the merchant HTTP body or delivery proof. Read a
merchant artifact through its declared provider operation when needed. Link is
not yet a selectable native paid-skill purchase method.

## Continue the original effect

Stop on missing bindings, changed account/terms, denied approval or uncertain
readback. Preserve the original effect, request key and provider references.
Resume acknowledged effects at readback; never replace an ambiguous payment
with another spend request, challenge, key or method. A refund already observed
on Link cannot be presented as a fresh successful purchase.

Do not put wallet keys, SPTs, bearer tokens or card data in skill inputs. A paid
Runx skill is bought by running that skill through the native purchase boundary;
this job does not replace it. Use `charge` as the merchant, `refund` for an
original collection's reversal, and `invoice-pay` to bind an invoice before pay.
