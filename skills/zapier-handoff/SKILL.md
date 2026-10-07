---
name: zapier-handoff
description: Validate a runx execution context and deliver a governed payload to a Zapier Catch Hook once, with exact approval and the hook's acknowledgement receipted.
runx:
  category: ops
---

# Zapier Handoff

Hand off governed runx work to a Zapier Catch Hook while keeping authority,
provider credentials, and receipts in runx.

This skill is for the outbound side of the Zapier integration story. It is not
the public Zapier App Directory app; that app should call hosted runx APIs. This
skill gives the same execution-context contract to local dogfood and any
operator-owned Zap that receives governed effects from runx.


## Runners

- `preflight`: validates and normalizes the handoff context without network.
- `send`: validates the context and delivers it to the selected Catch Hook.

Use `preflight` for reviews, CI, and local harnesses; it never needs approval.
The `send` runner first calls native `control.prepare_handoff`, which validates
the execution identity and produces the canonical delivery envelope. It then
runs `zapier/hook.invoke`, which posts that envelope to
`hooks.zapier.com/hooks/catch/<account>/<hook>/` after exact approval. A Catch
Hook takes no credential and Zapier offers no way to read a delivery back, so
the receipt records Zapier's acknowledgement with finality `acknowledged`, not
a confirmed readback.

## Execution context

`execution_context` must identify where the handoff came from. Include at least
one of:

- `caller` or `caller_id`
- `principal` or `principal_id`
- `workflow`, `workflow_id`, `workflow_ref`, or `source_workflow`
- `upstream_execution_id` or `upstream_run_id`

When present, these fields must match the top-level inputs:

- `platform`
- `event_id`
- `idempotency_key` (bound to `event_id`)
- `handoff_scope`
- `handoff_audience`

The Catch Hook receives the normalized `delivery` object, not a second
package-authored rendering. It carries the business payload, source context,
exact handoff scope and audience, and idempotency binding the Zap must validate
before any downstream action.

## Edge cases

- Public Zapier directory work must use hosted HTTPS runx APIs, not a local
  Catch Hook template.
- Do not include payment, token-transfer, or settlement actions in public Zapier
  v1. This local skill can model a hook handoff, but the public app must stay
  non-payment until review constraints are satisfied.
- Anyone holding the hook id can post to the hook, so treat it as a secret and
  have the Zap check `handoff_scope` and `handoff_audience` before acting.
- Do not put raw provider credentials into `payload` or `execution_context`.
  Pass credential references or let runx hold the provider secret.
- Zapier may retry or replay hook deliveries. The Zap must dedupe by `event_id`
  before downstream actions.

## Inputs

- `event_id` (required): stable id for receiver-side dedupe.
- `execution_context` (required): explicit caller/workflow context.
- `payload` (required): business payload delivered to Zapier.
- `handoff_audience` (optional): defaults to
  `zapier:zap:runx-governed-effect`.
- `zapier_account_id` and `zapier_hook_id` (send runner): Catch Hook path
  segments.

`event_id` is also the Runx mutation idempotency key and the value delivered as
`idempotency_key`; there is no second retry identity that can drift from it.
