---
name: governed-outbound
description: Fetch one allowlisted source, scrub personal data, and deliver the exact safe payload through a configured provider with approval and independent readback; use plan explicitly for a handoff only.
---

# Governed Outbound

Deliver exact scrubbed outbound content in this order:

1. `web-fetch` performs the bounded provider read and returns source readback.
2. `redact-pii` detects semantic identifiers, deterministically scrubs the content, and withholds residual content unless its final verdict is `ready`.
3. The exact scrubbed content and digest pass directly to `send-as`; the caller
   selects only the provider and bounded target, never provider request grammar.
4. `send-as#send` plans principal and audience, derives the provider request and
   idempotency key, requests exact approval for the consequential send,
   performs it once, and requires independent provider readback.

The provider's connector remains separately configured and tenant-agnostic. A
missing connector instance blocks before delivery; a plan or accepted request is never
reported as sent. Select the explicit `plan` runner when a provider-neutral
handoff is the requested outcome.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `redact-pii#redact-pii`
- `send-as#plan`
- `send-as#send`
- `web-fetch#web-fetch`

## Stop conditions

- Missing or disallowed fetch inputs stop in `web-fetch`.
- `needs_review` or `blocked` redaction skips planning and delivery.
- The plan runner never asks for performative approval; approval belongs to the
  exact live send in `send-as#send`.
- A send plan whose principal, audience, or digest differs from the approved redaction fails deterministic finalization.
- Provider execution and readback must occur in the selected downstream adapter before any caller reports delivery.

## Output

`outbound_plan` records `completion: plan_only`, `provider_delivery: not_executed`, source and redaction digests, the bounded send plan, and the selected provider actions. The scrubbed content remains available only in the redaction step output for the authorized downstream adapter.

Default delivery inputs are `url`, `allowlist`, `channel`, `principal`, and a
tenant-owned `connector` containing only `provider` and exact `target`.
`operator_context` is optional. The explicit `plan` runner omits `connector`.
