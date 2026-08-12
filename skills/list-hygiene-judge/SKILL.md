---
name: list-hygiene-judge
description: Read a contact's engagement and bounce projection, decide whether to re-permission or suppress, and record one idempotent consent-state transition.
links:
  source: https://github.com/SmartMenu9872/runx/tree/smartmenu9872-list-hygiene-judge/skills/list-hygiene-judge
runx:
  category: growth
---

# List hygiene judge

This skill is a bounded consent-state judgment. It reads one contact projection
through the declared data store, refuses to invent engagement or bounce facts,
and records at most one compare-and-set transition. It never sends a message.

Inputs are `data_source_ref`, `resource`, `aggregate_id`, `expected_version`,
`idempotency_key`, `engagement_history`, `bounce_policy`, and
`current_consent_state`. The projection must contain the same evidence fields;
the supplied values are cross-checked against the read result rather than used
as an unverified substitute.

Decision rules:

- hard bounces suppress;
- an active unsubscribe marker is never re-permissioned and escalates without a
  write;
- stale/missing evidence or a version mismatch stops without an append;
- otherwise, engagement older than `decay_threshold_days` is re-permissioned;
- all other states remain unchanged and are returned without a write.

The recorded event is consumed later by a separately governed send-as run. This
skill does not dispatch, publish, or contact anyone.
