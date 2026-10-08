---
name: send-as
description: Send one digest-bound provider-neutral message or campaign action through a compatible connector, with an explicit plan runner and approval plus readback at the live-delivery boundary.
---

# Send As

Govern a message, campaign, or notification sent on behalf of a principal.

The default `send` runner performs one delivery through a compatible normalized
connector and closes only after independent provider readback. `plan` is the
explicit no-effect runner. `apply` consumes an existing
`runx.send_as.plan.v1` packet without planning again, and `verify` consumes the
provider mutation result without sending again. Provider-specific skills can
compose these phase runners while retaining the same authority model.

## What this skill does

`send-as` binds the principal, provider, channel, recipients or audience,
content digest, consent basis, preflight checks, and approval gate. The caller
selects only a compatible provider and bounded target; it does not author
provider operations, result fields, payload plumbing, or retry keys. Runx
derives a canonical `message.send` request from the approved plan, binds its
idempotency key to a native digest, applies it under one approval, and verifies
the stable identity with `message.read`. It refuses to treat a draft, preview,
mutation acknowledgement, or test message as live delivery. A live send is
final only after provider readback and the Runx receipt seal.

Use `plan` when only an authorization packet is wanted. A sealed plan receipt
means only that the plan was sealed. A completed default result emits
`runx.send_as.result.v1`; its `sent` status is unavailable unless mutation and
readback both succeed.

## When to use this skill

- An agent needs to send or schedule one bounded message on behalf of a user,
  team, brand, account, or service through a compatible connector.
- An operator explicitly wants a plan and selects `plan`.
- A provider-specific skill needs a shared authority model before it can call a
  send API or MCP tool.
- The workflow must prove the intended audience, content, consent basis, and
  approval decision before delivery.
- A review needs to distinguish draft, test, scheduled, approved, sent, denied,
  and failed states.

## When not to use this skill

- To write copy only. Use a drafting or brand-voice skill unless delivery is in
  scope.
- To claim delivery from a `send_plan`; only `send_result` with provider
  readback is terminal evidence.
- To import contacts, enrich leads, verify domains, or configure billing as the
  main objective.
- To send without a named principal and audience.
- To hide provider credentials, raw contact lists, or customer data in the
  agent-visible output.
- To bypass unsubscribe, consent, suppression, warmup, preflight, legal, or
  human approval gates.

## Procedure

1. Identify the principal being represented and the provider account or surface.
2. Classify the send: `transactional`, `campaign`, `flow_step`, `support_reply`,
   `outreach`, `status`, or `internal`.
3. Bind channel and audience. Audience must be a named recipient, list, segment,
   support thread, channel, or scoped all-contacts decision; never an implicit
   broad default.
4. Bind content by digest or stable draft reference. Do not approve mutable
   content by prose summary alone.
5. Check consent, unsubscribe, suppression, compliance, preflight, and provider
   readiness. Missing evidence becomes a blocker.
6. Decide the gate:
   - drafts, previews, and test sends may proceed without live-delivery
     approval when provider policy permits them;
   - customer, public, audience, or live sends require explicit approval;
   - billing/account mutation is outside this skill and needs its own gate.
7. Produce the smallest provider-neutral `send_plan` that execution can consume
   without widening authority.
8. For the default runner, require only a compatible connector provider and
   bounded target. Runx derives the normalized operations, payload, expected
   stable identity, result fields, and digest-bound idempotency key. Missing
   connector authority blocks before mutation.
9. Apply once, then verify using the mutation result. The provider-observed
   mutation idempotency key must match the runtime-bound send key; the
   independent read operation has its own operation identity. Never re-plan or
   resend during `verify`.
10. Return `needs_input` for missing principal, audience, content digest,
    consent basis, or provider readiness; return `refused` for gate bypass.

## Edge cases and stop conditions

- **No principal:** return `needs_input`; the agent cannot speak as an unnamed
  actor.
- **No audience:** return `needs_input`; do not default to all contacts or a
  whole channel.
- **All contacts or broad audience:** require explicit reconfirmation and a
  stricter preflight block.
- **Mutable content:** return `needs_input` until content is digest-bound.
- **Missing consent or unsubscribe path:** block live delivery.
- **Preflight failure:** block provider send and preserve blocker evidence.
- **Approval denied or absent:** do not deliver.
- **No compatible connector operation:** return one actionable blocker; do not
  downgrade the default invocation to a plan or simulation.
- **Ambiguous mutation outcome:** resume with the same idempotency key; never
  create a new key to escape uncertainty.
- **Raw credentials or contact dumps:** redact; if redaction would remove the
  evidence needed to decide, return `needs_input`.

## Output schema

```yaml
send_plan:
  decision: ready | needs_input | denied | refused
  action_family: send-as
  principal:
    type: user | team | account | service
    ref: string
  provider:
    name: string
    account_ref: string
    runtime_path: string
  send_class: transactional | campaign | flow_step | support_reply | outreach | status | internal
  channel: email | sms | chat | push | webhook | other
  audience:
    type: recipient | list | segment | thread | channel | all_contacts
    ref: string
    requires_reconfirmation: boolean
  content:
    draft_ref: string
    digest: string
    subject_or_title: string
  gates:
    preflight_required: boolean
    human_approval_required: boolean
    approval_ref: string
  blockers: array
  provider_actions: array
  evidence_refs: array
  success_checkpoint:
    milestone: string
    description: string
```

Successful default execution additionally emits:

```yaml
send_result:
  schema: runx.send_as.result.v1
  status: sent | failed
  outcome: completed | failed
  provider: string
  target: string
  operation: string
  content_digest: string
  operation_id: string
  readback_ref: string
  idempotency_key: string
  evidence:
    mutation_readback_ref: string
    verification_readback_ref: string
  errors: array
```

## Worked example

Input: "Schedule the June newsletter to the subscribers list" with a campaign
draft digest, verified sender, named list, and provider account snapshot.

Output: `decision: ready`; `send_class: campaign`; audience is the named
subscribers list; content is digest-bound; preflight and human approval are
required; the provider actions are compose/review/test, then gated schedule.
No live send is authorized until the approval gate is satisfied.

## Inputs

- `objective` (required): bounded send or delivery objective.
- `principal` (required): who the message is sent as.
- `provider_context` (optional): provider/account readiness, connector, or MCP
  status.
- `audience` (optional): recipient, list, segment, thread, channel, or audience
  brief.
- `content_ref` (optional): digest, draft id, template id, campaign id, or
  stable content reference.
- `consent_basis` (optional): why the recipient/audience may receive this.
- `operator_context` (optional): approval posture, legal constraints, or extra
  guardrails.
- `connector` (required for `send`, `apply`, and `verify`): provider name and
  bounded account, workspace, channel, campaign, or equivalent target. It
  contains no credentials or caller-authored operation grammar.
