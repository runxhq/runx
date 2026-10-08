---
name: chief-of-staff
description: Convert bounded mailbox and calendar evidence into a reviewable executive action packet.
---

# Chief of Staff

Turn a bounded view of mail and calendar context into an operator-ready action
packet: what deserves attention, which replies are worth drafting, where time
can be offered, and which risks need a human decision. The point is not to
summarize an inbox. It is to reduce it to the few decisions and follow-ups that
matter.

This skill works over context that another product or provider reader has
already hydrated and redacted. It does not fetch a mailbox, send a reply, accept
an invitation, or change a calendar. Those are separate provider actions with
their own authority and approval.

## When to use it

Use `chief-of-staff` for an executive or operator review pass over a known set of
threads, events, and availability. It is useful when priorities must remain
traceable to source items and when proposed actions need to be handed to a human
or a later provider skill.

Do not use it as an always-on mailbox agent, as permission to answer on
someone's behalf, or to infer missing conversation context. If the supplied
snippets cannot support a safe recommendation, the correct result is
`needs_context`.

## How it works

1. Supply redacted threads, events, and availability with stable source digests
   and observation times.
2. Deterministic admission rejects duplicate, missing, malformed, or stale
   evidence and indexes the exact source references available to the review.
3. The synthesis ranks actions, drafts unsent replies, proposes only supplied
   available times, and surfaces risks instead of retelling every item.
4. Finalization rejects invented thread or event references and any proposed
   time outside the supplied availability snapshot.
5. Legal, billing, HR, security, and account-access matters always retain a
   manual-review flag; model judgment cannot downgrade that boundary.

The packet labels upstream digests as caller-supplied provenance. A digest proves
stable binding, not that Runx fetched or independently verified the provider
record.

## Inputs and result

- `objective` says what the operator needs from this pass.
- `as_of` and `max_age_days` define the freshness boundary.
- `mail_context.threads` and `calendar_context.events` carry unique ids,
  digests, observation times, redacted summaries, and sensitivity when known.
- Calendar availability includes its own evidence object, not merely a list of
  times.
- `constraints` narrow tone, ownership, scheduling, and allowed actions.

The result contains a ranked priority queue, unsent draft replies, scheduling
proposals, risks, missing questions, and mandatory-review flags. It always
records `delivery_status: not_sent` and `calendar_mutated: false`. A consumer may
route an accepted reply or calendar proposal to the appropriate provider skill,
but must preserve its source binding and obtain the relevant approval there.

## Stop conditions

- Return `needs_context` for stale, unprovenanced, duplicated, or insufficient
  source material.
- Never propose a time absent from the admitted availability snapshot.
- Do not invent prior conversation, commitments, owners, deadlines, or provider
  state.
- Keep sensitive categories under manual review even when the next action seems
  obvious.
- Do not send, schedule, reschedule, accept, decline, or delete anything.

## Example

A thread asks for a launch call and the calendar snapshot offers two times. The
packet can rank the request, draft an unsent reply, and propose one of those
times. It cannot offer a third time from model intuition, claim the reply was
sent, or book the event. If the thread concerns a billing dispute, the draft may
still be useful but the item remains marked for human review.

## Agent task contract

### `chief-of-staff-synthesize`

Rank only admitted source references. Return priorities, unsent drafts,
scheduling proposals, and risks. Every item must use an exact source reference,
and every proposed time must come from the supplied availability. Never mutate
mail or calendars, invent provider state, or downgrade sensitive review.
