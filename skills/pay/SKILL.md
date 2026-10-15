---
name: pay
description: Pay one external payment requirement within exact parent authority, through a payment method the payer holds, and read the paid resource back.
---

# Pay

`pay` is the buyer contract. `settle` pays the exact requirement once, and
`readback` performs one observation of that durable payment and the resource it
paid for. The default `pay` runner composes both and requires immediate terminal
readback, while durable callers compose `settle` once and re-enter `readback`
through Runx's external-job scheduler.

The payment method is a connector the payer holds. Today that is `wallet/evm`:
an x402 V2 challenge on an HTTPS resource, paid from the payer's EVM wallet in
USDC on Base. Other methods join as connectors when they are implemented.

Runx Hosted owns challenge validation, authority attenuation and reservation,
wallet credentials, payload signing, settlement recovery, finality, and the
private ledger. The connector never sleeps or polls; durable continuation
cadence belongs to Runx scheduling. `pay` never buys a runx paid skill: a
paid skill is bought by running it, and the kernel asks its buyer for that
purchase itself.

## Operator guide

Use this skill for one exact HTTPS resource already named by a hosted wallet
grant. The resource request carries bounded inline control data or artifact
references; large documents and media never travel as oversized graph values.
The hosted payer accepts external x402 V2 only, signs and stages the payload
before the paid POST, and reuses those exact bytes after an uncertain response.

Stop on malformed or expired requirements, amount, currency, payee or resource
drift, incomplete authority, missing approval, or inconsistent paid-resource
readback. Never pass wallet keys, seed phrases, bearer tokens, or facilitator
credentials in skill input. A 2xx response is not finality: a complete readback
needs the transaction confirmed on chain and a 2xx answer from the resource. It
never waits on the resource's own product state.

Use `charge` to take a payment as the seller, `refund` to reverse one, and
`settle-invoice` when an invoice needs validating before it is paid.
