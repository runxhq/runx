---
name: data-store
description: Read and append durable event state through the store connector's declared operations, not model-authored raw queries.
---

# Data Store

Operate durable event state through the store connector's typed operations.
This skill gives an agent enough context to append, read and project state
without learning storage credentials, inventing queries, or depending on one
storage backend.

The store keeps append-only event streams with an expected version per append,
a rolling projection per stream, and stream heads by latest event. Where the
streams live is the workspace's store instance: the workspace store (SQLite
under `.runx/store.sqlite`) unless the operator confirmed another, in SQLite,
Postgres or Redis. The skill never names the backend, and every backend answers
with the same digests.

## Runners

Each runner calls one store operation and returns its result as
`data_operation_result`:

- `append_event`: `store/events.append`.
- `read_events`: `store/events.read`.
- `read_projection`: `store/projection.read`.
- `list_stream_heads`: `store/heads.list`.
- `append_and_readback` (the default): append, then read the projection, so a
  requested durable transition is not reported complete until its projection
  has been read.

Graph callers that already know the operation select the narrower runner and
reuse its typed result.

## What this skill does

- Reads bounded event pages, projections and stream-head pages.
- Appends state transitions with idempotency keys and expected versions.
- Produces receipt-bound evidence for the source, resource, operation,
  versions and digests.
- Keeps product semantics outside the data layer. Messageboards, CRMs, billing
  ledgers and support desks define their own events and reducers.

## When to use this skill

- A graph needs durable state between turns, such as queue position, board
  state, sync cursor, review status, or approval inbox state.
- A skill must read a bounded slice of event state before deciding the next
  action.
- A workflow needs to append an auditable transition with optimistic
  concurrency.

## When not to use this skill

- To let a model write SQL, Redis commands or migrations.
- To export broad data sets, secrets, raw PII or whole tables.
- To hide product decisions in storage. Domain skills still own state
  machines, acceptance criteria and business rules.
- To treat a projection as independent truth when the event stream or receipt
  chain is available and required for review.
- To bypass payment, send, deploy, moderation or human approval gates.

## Procedure

1. Identify the domain skill and transition first. The store is a carrier,
   not the policy owner.
2. Name the logical source with `data_source_ref`. Never put database URLs,
   credentials or queries in the skill input.
3. Select one runner. Direct writes use `append_and_readback`; composed graphs
   select the exact runner.
4. For writes, pass the stream version you read as `expected_version` and a
   stable `idempotency_key` for the transition. The same call answers as an
   idempotent replay in any run; a stream at another version answers
   `conflict`.
5. Return the result with its versions, digests and stop conditions, so the
   receipt links the data effect to the transition that caused it.

## Edge cases and stop conditions

- `conflict`: the stream is not at `expected_version`. Nothing was appended;
  read the stream again before deciding.
- A store that cannot be reached fails the step with nothing appended.
- A write whose commit the store did not confirm is held for reconciliation by
  the kernel; the step never reports a write it cannot prove.

## Output schema

All runners return `runx.data.operation_result.v1`:

```json
{
  "schema": "runx.data.operation_result.v1",
  "data_source_ref": "local://example",
  "provider": "store",
  "operation": "events.append",
  "resource": "board_events",
  "aggregate_id": "posting-123",
  "status": "committed",
  "before_version": 0,
  "after_version": 1,
  "idempotency_key": "runx:sha256:...",
  "event_ref": "board_events:posting-123:1",
  "event_digest": "sha256:...",
  "result_digest": "sha256:...",
  "projection_digest": "sha256:...",
  "projection": null,
  "rows": [],
  "events": [],
  "redactions": [],
  "stop_conditions": [],
  "provider_evidence": { "backend": "sqlite" }
}
```

`idempotency_key` is the key the store holds the event under, which the kernel
derives from the call.

An event's type is, in order: `event.type`, `event.event_type`,
`event.effect_family + "." + event.operation`, or `event.operation`, else
`data.event`. Domain skills that emit `runx.effect.transition.v1` packets
include `effect_family` and `operation` on every event so projections say
`board.accept` or `business_ops.route` instead of `data.event`.

## Worked example

A board skill decides that `posting.claimed` is allowed and emits a transition
packet. Its graph calls `data-store` with `runner: append_event`, resource
`board_events`, aggregate `posting-123`, expected version `2` and idempotency
key `posting-123:claim:agent-9`. The store appends the event only if the stream
is still at version `2`. The receipt proves the decision, the append and the
new version. A later turn calls `read_events` or `read_projection` to resume.

## Inputs

- `data_source_ref` (required): the logical source inside the workspace's
  store.
- `resource` (required): the event resource or stream family.
- `aggregate_id` (required for stream operations): the stream key.
- `event` (required for appends): the domain event or transition packet.
- `idempotency_key` (required for appends): a stable key for the transition.
- `expected_version` (required for appends): the stream version the caller
  read.
- `observed_at` (optional for appends): an RFC 3339 commit time; the time of
  the append when omitted.
- `limit` (optional): events per page (at most 500) or heads per page (at most
  100).
- `after_version` (optional for `read_events`): read the ascending page after
  this version; omit it to read the latest page.
- `event_types` (optional for `list_stream_heads`): at most 20 exact latest
  event types.
- `cursor` (optional for `list_stream_heads`): the opaque cursor the previous
  page returned.

## Invocation example

```bash
runx skill data-store append_event \
  -i data_source_ref=local://runx-data-store/dev-board \
  -i resource=board_events \
  -i aggregate_id=posting-123 \
  --input-json expected_version=0 \
  -i idempotency_key=posting-123:create:v1 \
  --input-json event='{"type":"posting.created","payload":{"title":"verify a receipt link"}}' \
  --json
```

The same command appends to Postgres or Redis once the operator confirms a
store instance there:

```bash
runx connector use store --parameter url=redis://cache.internal:6379/0
```

A store that needs a password takes it as the instance's credential:

```bash
printf '%s' "$BOARD_DB_PASSWORD" |
  runx credential set store --credential password \
    --parameter url=postgres://board@db.internal:5432/board --from-stdin
```
