---
name: list-hygiene-judge
description: Decide and durably record evidence-bound contact consent transitions for engagement decay and hard bounces. Use when an operator needs to re-permission, suppress, or stop for human review before any campaign send.
registry_owner: ArgonautWorks
---

# List Hygiene Judge

Use this skill between contact evidence collection and outbound delivery. It
turns fresh engagement, bounce, and consent evidence into one conservative
consent-state decision, records an allowed transition with compare-and-set
semantics, and reads the contact projection back before reporting success.

This is not a sender. A later `send-as` run must independently read the recorded
consent state at send time and gate delivery. Never treat this skill's output as
send authority, a campaign proposal, or proof that a message was delivered.

## Operating model

1. Read the contact projection through the exact provider-neutral
   `data.read_projection` operation owned by the canonical `data-store`
   contract. Bind `data_source_ref`, `resource`, and `aggregate_id` to the
   contact's event stream. The read establishes the durable version used by the
   decision.
2. Admit the supplied engagement and consent evidence only when its status is
   `read`, its `evidence_version` equals `expected_version`, and the durable
   projection has that same version. Missing, unreadable, ambiguous, or stale
   evidence stops without an append.
3. Honor an active unsubscribe marker as terminal for automation. Route the
   case to a human list-hygiene reviewer; never re-permission it automatically.
4. Suppress when verified `hard_bounces` is greater than zero and policy names
   `suppress`. Otherwise, re-permission only when `recency_days` exceeds the
   declared decay threshold and no unsubscribe marker exists.
5. Append exactly one `list_hygiene.consent_transitioned` event with the
   caller's stable `idempotency_key` and `expected_version`. The data-store owns
   compare-and-set enforcement and idempotent replay.
6. Read the projection back. Report a recorded transition only when the new
   version and event type match the plan. A stop returns `human_review` with no
   event and confirms that the projection version did not change.

## Evidence, authority, and finality

The evidence inputs are bounded observations, not permission to send. Do not
invent opens, clicks, bounces, recency, consent markers, or versions. A caller
that cannot supply them must repair the upstream evidence read rather than fill
defaults.

The runner requests only `runx:data:read` and `runx:data:append` through the
canonical data operations used by `data-store`. The allowed append is
intentionally ungated: it records contact policy state, not external delivery. The stable
`idempotency_key` makes an unchanged retry return the already-recorded version;
a competing write produces a version conflict and must be retried only after a
fresh read and new decision.

A sealed result proves the governed read, decision, optional append, and
readback. It does not prove that a campaign was sent. `downstream_send.status`
therefore remains `not_run` on every result.

## Decisions and recovery

- `re_permission`: engagement is older than the declared threshold, there is
  no hard bounce, and no active unsubscribe marker exists. One transition is
  recorded.
- `suppress`: at least one hard bounce is present and policy explicitly selects
  suppression. One transition is recorded.
- `human_review`: evidence is missing, unreadable, ambiguous, stale, versioned
  differently from the contact projection, protected by an unsubscribe marker,
  or does not justify an automated transition. No event is appended.

On a version conflict, read the contact again and make a new decision. Reuse an
idempotency key only for the identical intended transition. If bounce recovery
or unsubscribe history is ambiguous, keep the case in the human review lane;
do not weaken the stop condition to make the run pass.

## Inputs and result

Provide the logical data source, resource, contact `aggregate_id`, current
`expected_version`, stable `idempotency_key`, engagement counts and recency,
the bounce policy, and the current consent state with evidence status/version.

The result contains:

- `decision`: `re_permission`, `suppress`, or `human_review`, with a reason;
- `recorded_transition`: readback-bound contact, version, event, projection,
  and idempotency evidence, plus whether a write occurred;
- `escalation`: the human lane when automation stopped;
- `downstream_send`: an explicit `not_run` handoff reminder for `send-as`.

For example, a subscribed contact at version 3 with no hard bounces and
`recency_days: 121` under a 90-day threshold may be moved to `re_permission`
with `expected_version: 3`. The result is final only after readback reports
version 4 and the consent-transition event.

## Agent rules

- Read the projection before deciding and read it again after any append.
- Prefer suppression over engagement recency when verified hard-bounce evidence
  exists.
- Never re-permission an active unsubscribe marker.
- Never append on missing, unreadable, ambiguous, or stale evidence.
- Never dispatch, send, mint authority, or claim provider delivery.
- Return the sealed receipt with the result so the next operator can verify the
  decision without private context.
