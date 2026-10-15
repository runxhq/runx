---
name: message-send
description: Send one digest-bound message on behalf of a principal, posting to a channel or replying in a bound conversation through a compatible connector, after exact approval and independent provider readback.
---

# Message Send

Govern a message sent on behalf of a principal: a channel post or a reply in a
bound conversation. The skill never receives a provider token, never lets
approved content drift before delivery, and calls a message delivered only
after the provider reads it back.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `slack#read_thread`

## Runners

- `send` (default): plans one channel post, requests approval only at the live
  post, posts once, and requires independent readback.
- `plan`: the provider-neutral authorization step. It binds the principal,
  audience, content digest, consent basis, preflight checks and approval gate
  into a `runx.message_send.plan.v1` packet, and never delivers.
- `plan_post`: validates a channel name, channel id, or exact
  `slack://workspace/channel` destination, computes the message digest with
  Runx's native `data.digest`, and binds that digest, the named principal and
  the send intent through `plan`. It does not post and needs no approval.
- `plan_post_exact`: for an operator that has already selected one exact
  destination, principal and text under its own policy. It reuses the same
  validation and digest and emits the same undelivered `runx.notify.v1` plan
  without a second model judgment. Its `send_plan.origin` is
  `message-send.exact`, not a claim that `plan` ran.
- `post`: accepts only an exact `notify_plan`, the matching destination and
  text, and a stable UUID idempotency key. It recomputes the digest, compares
  the complete binding, stops unless exact native approval or a matching
  standing notification authority is present, runs `channel.post` through the
  configured binding, then reads the returned message locator back with
  `channel.post.read` and seals only the bounded evidence fields.
- `reconcile_post`: reads back a post whose delivery outcome was ambiguous,
  without posting again.
- `reply`: delivers one admitted reply to a bound conversation. The operator
  supplies the exact source packet, thread, draft, provider and native turn
  identity. The skill holds unless the source proves complete coverage, then
  asks for exact approval, runs `thread.reply` on the chosen connector, and
  requires `thread.reply.read` to resolve a stable message locator with the
  matching content digest. An accepted or queued request alone leaves the work
  held for reconciliation.

A channel post and a source-bound reply keep their own input checks: a post is
bound to a destination and a plan, a reply to a source packet and a turn.

The binding may be local, self-hosted, third-party or Runx-hosted. The skill
never receives a token, constructs a raw HTTP request, owns a tenant, or stores
connector credentials in its inputs or receipts.

## When to use this skill

- An agent needs to post one bounded message, or reply in one conversation, on
  behalf of a user, team, brand, account or service.
- An operator explicitly wants an authorization plan and selects `plan`.
- The workflow must prove the intended audience, content, consent basis and
  approval decision before delivery.

## When not to use this skill

- To write copy only. Use a drafting or brand-voice skill unless delivery is in
  scope.
- To claim delivery from a plan; only provider readback is terminal evidence.
- To send without a named principal and audience.
- To bypass unsubscribe, consent, suppression, preflight, legal or human
  approval gates.

## Procedure for `plan`

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
8. Return `needs_input` for missing principal, audience, content digest,
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
- **Plan, destination, content-digest, principal or audience drift:** stop.
- **Approval denied or absent:** do not deliver.
- **Missing, ambiguous, wrong-provider or insufficient-scope binding:** refuse
  rather than fall back to a raw token.
- **Ambiguous mutation outcome:** resume with the same idempotency key, or
  reconcile; never create a new key to escape uncertainty. Changed content
  never inherits the old approval.
- **Raw credentials or contact dumps:** redact; if redaction would remove the
  evidence needed to decide, return `needs_input`.

## Output schema

```yaml
send_plan:
  decision: ready | needs_input | denied | refused
  action_family: message-send
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

A post is delivered only when the sealed `runx.provider.operation.v1` packet
identifies the operation and carries the provider-returned message locator,
conversation locator, content digest and occurrence time, and the follow-up
readback resolves that exact locator.

## Worked example

An operator plans "release 2.4 is verified" for one internal channel. Approval
binds that exact text and destination. If the text changes to include another
claim, `post` refuses the digest mismatch. With a matching plan and one valid
Slack binding, the provider posts and returns the message id; the follow-up
readback resolves that id, and those provider observations, not the local plan,
prove delivery.

## Inputs

- `send`: `channel_locator`, `text`, `principal`, `idempotency_key`, and
  optional `provider_context`.
- `plan`: `objective` and `principal`, with optional `provider_context`,
  `audience`, `content_ref`, `consent_basis`, `operator_context` and
  `connector`.
- `post`: the `notify_plan`, the identical locator and text, and a stable
  idempotency key.
- `reply`: `source_context`, `turn_id`, `expected_provider`, `thread_locator`
  and `draft_message`.
