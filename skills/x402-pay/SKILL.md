---
name: x402-pay
description: Pay one x402 challenge through Runx Hosted with approval and paid-resource readback.
---

# X402 Pay

`x402-pay` is the public x402 buyer contract. `settle` submits the exact payment
signal once, and `readback` performs one observation of that durable payment and
resource. The default `pay` runner composes both and requires immediate terminal
readback, while durable callers compose `settle` once and re-enter `readback`
through Runx's external-job scheduler.

Runx Hosted owns challenge validation, authority attenuation and reservation,
wallet or facilitator credentials, payload signing, settlement recovery,
finality, and the private ledger. The connector never sleeps or polls; durable
continuation cadence belongs to Runx scheduling. OSS contains the public
v1 Runx contract and generic provider-effect gate only. References to x402
protocol version 2 in upstream conformance material describe the external
standard, not a Runx v2 contract.

## Operator guide

Use this skill for one exact HTTPS resource already named by a hosted x402 grant.
The resource request carries bounded inline control data or artifact references;
large documents and media never travel as oversized graph values. The hosted
buyer accepts external x402 V2 only, signs and stages the payload before the paid
POST, and reuses those exact bytes after an uncertain response.

Stop on malformed or expired requirements, amount/currency/payee/resource drift,
incomplete authority, missing approval, or inconsistent paid-resource readback.
Never pass wallet keys, seed phrases, bearer tokens, or facilitator credentials
in skill input. A 2xx response is not finality: the readback must return the
provider transaction, confirmed chain block, and the paid resource's terminal
Runx receipt reference.
