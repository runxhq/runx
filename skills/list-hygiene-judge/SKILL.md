---
name: list-hygiene-judge
description: Judge one contact's engagement decay, bounce evidence, and consent state; persist only a version-bound re-permission or suppression transition and stop safely when evidence or durable readback is unsafe.
---

# List Hygiene Judge

A stale list is a consent problem before it is a delivery problem. This skill
reads the contact's durable stream position, judges one bounded transition from
explicit engagement and consent evidence, and records only `re_permission` or
`suppress`. It never sends a message, invents consent, or treats an attempted
write as proof that state changed.

Use `judge` with a configured data source, event-stream resource, contact key,
current stream version, and a stable idempotency key. Supply bounded engagement
metrics, the hard-bounce and decay policy, and current consent evidence aligned
to that same version. Durable history stays in the data source; the runner
carries only the control values needed for this decision.

The graph reads the projection before deciding. A verified hard bounce becomes
`suppress` only when policy names suppression. Engagement older than the decay
threshold becomes `re_permission` only when no active unsubscribe marker is
present. The append uses optimistic concurrency and the supplied retry key,
then the graph reads the projection again. A sealed result describes observed
durable state, not downstream delivery authority.

## Stops and recovery

Missing, unreadable, stale, ambiguous, or version-mismatched evidence returns a
governed `stop` and emits no transition. An active unsubscribe marker also
stops automated re-permission. If the append conflicts, disappears, or the
post-write projection does not show the intended event at the next version,
`finalize` returns a `readback_mismatch` stop instead of throwing or reporting
success.

Refresh the contact projection and rebuild evidence against its current
version before retrying. Reuse the same idempotency key only for the same
intended transition. Ambiguous bounce recovery and unsubscribe disputes belong
in a human list-hygiene review lane. Any later send must independently read the
recorded consent state and refuse suppressed contacts.

## Domain computation

`list-hygiene-judge.mjs` owns only deterministic consent classification and
readback verification. Runx owns projection reads, compare-and-set append,
idempotency, isolation, and receipts. The module has no filesystem, network,
credential, clock, or dispatch authority.
