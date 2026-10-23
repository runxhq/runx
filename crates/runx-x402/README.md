# runx-x402

Effect-free x402 v2 presentation for Runx contracts.

This crate assembles complete `PaymentRequired` values, encodes and decodes the
three standard HTTP headers, binds an assembled challenge to the rail-neutral
Runx purchase contract, and validates retry echoes before any payment
verification may occur.

It contains no HTTP client/server, async runtime, facilitator, credential,
storage, settlement, or provider behavior.

The `runx.purchase` declaration keeps the external `{ info, schema }` form
and advertises its schema by reference, `{ "$ref": <published v1 $id> }`,
so a challenge fits one portable HTTP header next to the seller's own
discovery declaration. Retry validation accepts only that reference; an
inline schema document is refused.

A retry commits to one offered requirement and to the `runx.purchase`
binding, nothing else. The resource and any discovery declaration belong to
whoever presented the challenge: a seller's own door may relay runx's
challenge under its own resource, and the payment it forwards settles as that
door.
