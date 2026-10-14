---
name: webhook-handoff
description: Validate a runx execution context and deliver a governed payload once to an n8n Cloud workflow webhook or a Zapier Catch Hook, with exact approval and the receiver's acknowledgement receipted.
---

# Webhook Handoff

Hand off governed runx work to a workflow platform without turning it into the
authority holder.

This skill is the outbound side of a workflow integration. runx owns the
policy decision, execution context, and receipt. The receiving platform owns
its webhook, canvas, branching, fan-out, and downstream notifications.
`platform` chooses the receiver and the connector that delivers to it: `n8n`
for an n8n Cloud workflow, `zapier` for a Zapier Catch Hook.

## Runners

- `preflight`: validates and normalizes the handoff context without network.
- `send`: validates the context and delivers it to the chosen receiver.

Use `preflight` for reviews, CI, and local harnesses; it never needs approval.
The `send` runner first calls native `control.prepare_handoff`, which validates
the execution identity and produces the canonical delivery envelope. It then
runs `handoff.deliver` on the chosen connector, which posts that envelope after
exact approval: for n8n to `https://<workspace>.app.n8n.cloud/webhook/<slug>`,
for Zapier to the Catch Hook. Neither platform offers a way to read a delivery
back, so the receipt records the receiver's answer with finality
`acknowledged`, not a confirmed readback.

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

- Only n8n Cloud hosts are reachable for n8n. A self-hosted instance needs its
  own connector that declares its host.
- Anyone who knows the webhook path can post to it, so have the receiving
  workflow check `handoff_scope` and `handoff_audience` before acting.
- Do not put raw provider credentials into `payload` or `execution_context`.
  Pass credential references or let runx hold the provider secret.
- If an n8n workflow slug changes, update `handoff_audience` to the matching
  `n8n:workflow:<slug>` value.
- The receiver must dedupe by `event_id` before branching or sending downstream
  notifications.

## Inputs

- `platform` (required): `n8n` or `zapier`.
- `event_id` (required): stable id for receiver-side dedupe.
- `execution_context` (required): explicit caller/workflow context.
- `payload` (required): business payload delivered to the receiver.
- `handoff_audience` (optional): defaults to
  `n8n:workflow:runx-governed-effect` or `zapier:zap:runx-governed-effect`.
- `webhook_host` (send runner, n8n only): the n8n Cloud host, such as
  `acme.app.n8n.cloud`. A Zapier handoff refuses one.

The webhook URL is the chosen connector's credential: n8n's `webhook`, bound
with `runx connector use n8n --credential webhook`, which must be on
`webhook_host`; Zapier's `catch_hook`, bound with
`runx connector use zapier --credential catch_hook`. Either URL goes on stdin
and never enters the run's inputs, records or receipts.

`event_id` is also the Runx mutation idempotency key and the value delivered as
`idempotency_key`; there is no second retry identity that can drift from it.
