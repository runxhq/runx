---
name: n8n-handoff
description: Validate a runx execution context and deliver a governed payload to an n8n Cloud workflow webhook once, with exact approval and the workflow's acknowledgement receipted.
---

# n8n Handoff

Hand off governed runx work to an n8n workflow without turning n8n into the
authority holder.

This skill is for the outbound side of the n8n integration story. runx owns the
policy decision, execution context, and receipt. n8n owns its workflow webhook,
canvas, branching, fan-out, and downstream notifications.


## Runners

- `preflight`: validates and normalizes the handoff context without network.
- `send`: validates the context and delivers it to the selected n8n workflow.

Use `preflight` for reviews, CI, and local harnesses; it never needs approval.
The `send` runner first calls native `control.prepare_handoff`, which validates
the execution identity and produces the canonical delivery envelope. It then
runs `n8n/workflow.invoke`, which posts that envelope to
`https://<workspace>.app.n8n.cloud/webhook/<slug>` after exact approval. The
webhook takes no credential and n8n offers no way to read a delivery back, so
the receipt records the workflow's answer with finality `acknowledged`, not a
confirmed readback.

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

The receiver gets the normalized `delivery` object, not an independently
reassembled copy of these fields. That object carries the business payload,
source context, exact handoff scope and audience, and idempotency binding the
receiver must validate before starting its workflow.

## Edge cases

- Only n8n Cloud hosts are reachable. A self-hosted instance needs its own
  connector that declares its host.
- Anyone who knows the webhook path can post to it, so have the workflow check
  `handoff_scope` and `handoff_audience` before acting.
- Do not put raw provider credentials into `payload` or `execution_context`.
  Pass credential references or let runx hold the provider secret.
- If the workflow slug changes, update `handoff_audience` to the matching
  `n8n:workflow:<slug>` value.
- The receiver must dedupe by `event_id` before branching or sending downstream
  notifications.

## Inputs

- `event_id` (required): stable id for receiver-side dedupe.
- `execution_context` (required): explicit caller/workflow context.
- `payload` (required): business payload delivered to n8n.
- `handoff_audience` (optional): defaults to
  `n8n:workflow:runx-governed-effect`.
- `webhook_host` (send runner): the n8n Cloud host, such as
  `acme.app.n8n.cloud`.

The workflow's production webhook URL is the `n8n` connector's `webhook`
credential, bound once with `runx credential set n8n --credential webhook
--from-stdin`. It must be on `webhook_host`, and it never enters the run's
inputs, records or receipts.

`event_id` is also the Runx mutation idempotency key and the value delivered as
`idempotency_key`; there is no second retry identity that can drift from it.
