---
name: list-hygiene-judge
description: Decide whether a contact should be verified, suppressed, or re-permissioned from read engagement and bounce evidence, record the consent transition through data-store, and leave the send to a separate governed run.
runx:
  category: ops
---

# List Hygiene Judge

List hygiene is the judgment that sits between engagement decay and suppression,
and the dangerous part is the durable consent-state transition. This skill is a
graph runner: it reads a contact through the hosted data-store keyed by the
contact as the domain entity, decides whether to verify, suppress, or
re-permission, and records the transition by appending exactly one event to that
contact's stream.

The decision is a thin `act{form: review}` over content-keyed memory: the seal
proves the read and the verdict. The state write is an ungated compare-and-set
`append_event` under `idempotency_key` plus `expected_version` — not a proposal
and not a mint.

**The skill never sends.** The recorded consent state is honored later by
`send-as`, which is a separate governed run dispatched by naming: `send-as` reads
the recorded state at send time and gates delivery, so a suppressed contact
cannot receive a campaign. `send-as` is the downstream enforcer of the recorded
state, never a consumer of this skill's output.

## Inputs

- `data_source_ref` and `store_id`: the pinned data-store binding.
- `resource`: the data-store resource holding the contact consent stream
  (default `contacts`).
- `aggregate_id`: the contact entity id, used as the data-store aggregate id.
- `expected_version`: the stream version the caller read. A mismatch against the
  projection is a stale-evidence stop, never a write.
- `idempotency_key`: stable append key, so a retry with the same key returns the
  recorded version instead of double-applying the transition.
- `engagement_history`: `opens_count`, `clicks_count`, `hard_bounces`,
  `recency_days`.
- `bounce_policy`: `hard_bounce_action`, `decay_threshold_days`.
- `current_consent_state`: the consent state the caller believes is current. An
  active unsubscribe marker in the store outranks it.

## Output Contract

The output packet is `runx.list.hygiene_judge.v1` data:

- `decision{state,reason}` always appears. `state` is one of `re_permission`,
  `suppress`, `no_change`, or `stop`.
- `recorded_transition` and `contact_event` appear only when a transition is
  warranted, and carry the `aggregate_id` / `expected_version` /
  `idempotency_key` binding of the append.
- `stop_state` appears instead whenever no append is emitted.
- There is no `operational_proposal` envelope and no minted grant.

## The Judgment

Decided in this order, and every branch is grounded in state that was read:

1. **Evidence gate.** A missing `opens_count`, `clicks_count`, `hard_bounces`,
   or `recency_days` counter is a stop, not a zero. An unreadable contact
   projection is a stop. A missing `bounce_policy` is a stop.
2. **Stale `expected_version`.** Checked *before* deciding: if the projection is
   at a different version than the caller passed, our read is behind the stream
   and any append would apply against state we never saw. Stop, escalate.
3. **Ambiguous bounce recovery.** The contact hard-bounced *and* has since
   engaged inside the decay window. Suppressing would drop a live human;
   re-permissioning would ignore a real delivery failure. Neither is decidable
   from the evidence, so neither is taken — escalate to a human approval lane.
4. **Suppress.** `hard_bounces > 0` read from the store. This is the only path to
   suppression: the judgment refuses to suppress without hard-bounce evidence.
5. **Active unsubscribe marker.** Refuse to re-permission a contact carrying one,
   and escalate instead.
6. **Re-permission.** `recency_days` exceeds `bounce_policy.decay_threshold_days`
   with no unsubscribe marker and no hard bounces.
7. Otherwise `no_change`, and no event is appended.

## State Write

`read_projection` on the contact entity, decide, then `append_event` with
`idempotency_key` + `expected_version` compare-and-set against the pinned
`store_id`. The append and the read-back are both guarded on
`decision.writes == true`, so every stop path provably emits no append.

## Harness

Three inline cases, run with `runx harness ./skills/list-hygiene-judge`:

- `sealed_decay_re_permission` — `recency_days` 400 over a 180-day threshold, no
  unsubscribe marker, no bounces → `re_permission`, one append.
- `sealed_hard_bounce_suppress` — `hard_bounces` 2 → `suppress`, one append.
- `stop_missing_or_stale_evidence` — `engagement_history` absent → stop, and the
  guards block both the append and the read-back.

## Install and Run

```bash
runx add <owner>/list-hygiene-judge@<version>
runx skill <owner>/list-hygiene-judge@<version> --json \
  -i data_source_ref=<ref> -i store_id=<store> \
  -i aggregate_id=<contact-id> -i expected_version=0 \
  -i idempotency_key=<key> \
  --input-json engagement_history='{"opens_count":0,"clicks_count":0,"hard_bounces":0,"recency_days":400}' \
  --input-json bounce_policy='{"hard_bounce_action":"suppress","decay_threshold_days":180}' \
  -i current_consent_state=subscribed
runx verify --receipt <receipt.json> --json
```
