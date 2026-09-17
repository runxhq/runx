# runx-x402

Effect-free x402 v2 presentation for Runx contracts.

This crate assembles complete `PaymentRequired` values, encodes and decodes the
three standard HTTP headers, binds an assembled challenge to the rail-neutral
Runx paid-invocation contract, and validates retry echoes before any payment
verification may occur.

It contains no HTTP client/server, async runtime, facilitator, credential,
storage, settlement, or provider behavior.

The `runx.invocation` declaration keeps the external `{ info, schema }` form
and advertises its schema by reference, `{ "$ref": <published v1 $id> }`,
so a challenge fits one portable HTTP header next to the vendor's own
discovery declaration. Retry validation accepts the reference or the inline
v1 document (challenges assembled before the reference form) and compares the
two normalized; inline acceptance retires once every emitter is on the
reference form.
