---
name: list-hygiene-judge
description: Judge a mailing-list contact for verify, suppress, or re-permission over a sealed data-store read, and record the consent-state transition with one compare-and-set append. Never sends.
runx:
  category: data
---

# List Hygiene Judge

List hygiene is the judgment that sits between engagement decay and suppression,
and the dangerous part is the durable consent-state transition. This skill is a
graph runner that makes that judgment auditable: it reads the contact through
the pinned data-store first, decides deterministically, and records the
transition by appending exactly one event to that contact's stream under an
idempotency key and an expected version.

The seal proves the read and the verdict. The state write is an ungated
compare-and-set `append_event` — not a proposal and not a mint. The skill never
sends: the recorded consent state is honored later by `send-as`, a separate
governed run dispatched by naming, which re-reads the recorded state at send
time and gates delivery itself. A suppressed contact cannot receive a campaign;
`send-as` is the downstream enforcer of the recorded state, never a consumer of
this skill's output.

## What this skill does

- Reads the contact projection (`read_projection`) from the pinned
  `data_source_ref` before judging, so the verdict rests on sealed store
  evidence, not on caller assertions alone.
- Decides one of four states deterministically: `verify`, `suppress`,
  `re_permission`, or `stop`.
- Records a durable transition only for `suppress` and `re_permission`, via one
  `append_event` with `idempotency_key` + `expected_version` compare-and-set,
  then reads the recorded transition back from the projection.
- Escalates to a human approval lane (`human_list_hygiene_review`) instead of
  writing when evidence is missing or unreadable, when bounce recovery is
  ambiguous, when the expected version is stale, or when the contact carries an
  active unsubscribe marker.
- Seals `downstream_send_performed: false` in every packet: no send ever
  happens here.

## Decision rules

1. No readable store projection → `stop`, no append.
2. Malformed engagement or bounce policy → `stop`, no append. The judgment
   never invents opens, clicks, bounce counts, or recency it cannot read.
3. Projection version differs from `expected_version` → `stop`, no append
   (stale caller view; re-read and re-judge).
4. `hard_bounces > 0` with recent opens or clicks inside the decay threshold →
   ambiguous bounce recovery → `stop` + escalate, no append.
5. `hard_bounces > 0`, no recovery signal, and
   `bounce_policy.hard_bounce_action: suppress` → `suppress` + one append.
   Suppression is never decided without hard-bounce evidence.
6. `recency_days > bounce_policy.decay_threshold_days` with no unsubscribe
   marker → `re_permission` + one append. An active unsubscribe marker refuses
   re-permission and escalates instead.
7. Engaged and clean → `verify`, no durable transition, no append.

## Inputs

- `data_source_ref` (required): pinned logical store ref for the contact event
  stream, bound by project or hosted configuration to `runx/data-store`.
- `resource` (required): declared event resource, e.g. `contact_events`.
- `aggregate_id` (required): the contact as the domain entity (stream key).
- `expected_version` (required): stream version the caller judged from.
- `idempotency_key` (required): stable retry key; a retry with the same key
  returns the recorded version instead of double-applying.
- `engagement_history` (required): `{opens_count, clicks_count, hard_bounces,
  recency_days}`.
- `bounce_policy` (required): `{hard_bounce_action, decay_threshold_days}`.
- `current_consent_state` (required): caller's view of the contact's consent
  state; cross-checked against the sealed stream read.

## Output

`runx.list_hygiene_result.v1`: `decision{state, reason}`,
`escalation{required, lane, reason}`, the evidence basis with digests, the
persistence record (`committed` with `after_version` and `event_ref`, or
`skipped`), and the recorded transition read back from the contact's
projection. There is no `operational_proposal` envelope and no minted grant.

## Invocation example

```bash
runx skill list-hygiene-judge \
  -i data_source_ref=local://list-hygiene/contacts \
  -i resource=contact_events \
  -i aggregate_id=contact:ava@example.com \
  --input-json expected_version=0 \
  -i idempotency_key=contact:ava:re-permission:v1 \
  --input-json engagement_history='{"opens_count":0,"clicks_count":0,"hard_bounces":0,"recency_days":120}' \
  --input-json bounce_policy='{"hard_bounce_action":"suppress","decay_threshold_days":90}' \
  -i current_consent_state=subscribed \
  --json
```

Unbound `local://...` refs default to native durable SQLite under
`.runx/data/local-sources/`; production binds the same ref to a hosted
data-store adapter through `RUNX_DATA_SOURCES` or `.runx/data-sources.json`.

## When not to use this skill

- To send anything. Dispatch `send-as` by naming; it gates on the recorded
  state itself.
- To bulk-erase or export contact data (see `data-subject-request`).
- To override an unsubscribe. That path always escalates to a human.
