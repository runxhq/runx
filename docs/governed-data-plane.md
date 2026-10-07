# Governed Data Plane

runx should support stateful work without becoming a database. The data plane is
the boundary between domain skills and storage providers.

This is intentionally a skill capability, not a separate database-admin CLI.
The `runx skill` command remains the execution surface; sources and providers
are selected by bindings.

## Shape

- A **domain skill** owns product meaning: board transitions, review
  states, CRM records, approval inboxes, support tickets, ledgers, and so on.
- A **data source** declares resources addressed by exact typed operations:
  append event, read events, read projection, and list stream heads.
- A **data implementation** executes those operations against storage. Runx
  ships native SQLite and a conforming external Redis adapter; other providers
  may implement the same boundary.
- A **data operation result** is provider-neutral receipt evidence:
  `runx.data.operation_result.v1`.

The model never authors arbitrary SQL, Redis commands, or migrations. It selects
declared operations with typed params.

## Adapter Selection

Users choose a **data source**, not a raw provider command. A data source is a
stable logical ref such as `local://runx-data-store/dev-board`,
`tenant://acme/board`, or `runx:data-source:acme-board`. The project or hosted
operator binds that source to a concrete adapter:

```json
{
  "data_sources": {
    "tenant://acme/board": {
      "adapter": "data.postgres",
      "profile": "prod-board",
      "resources": {
        "board_events": {
          "kind": "event_stream",
          "partition_key": "aggregate_id"
        }
      }
    }
  }
}
```

The skill run still passes only the logical source and operation inputs:

```bash
runx skill data-store append_event \
  -i data_source_ref=tenant://acme/board \
  -i resource=board_events \
  -i aggregate_id=posting-123 \
  --input-json expected_version=2 \
  -i idempotency_key=posting-123:claim:agent-9 \
  --input-json event='{"type":"posting.claimed","payload":{"actor":"agent-9"}}' \
  --json
```

For local dogfood, unbound `local://...` refs use native durable SQLite under
`.runx/data/local-sources/`. The file name is derived from the logical source
ref, so independent sources do not collide and stateful skills need no separate
database setup. A configured source can instead route to a conforming external
provider such as `data.redis`. Domain skills never branch on provider type: if
a graph needs another supported backend, change the binding or pass a different
`data_source_ref`; do not put board, CRM, or operator semantics into the
storage implementation.

Adapter binding is authority-bearing configuration. It may name a credential
profile or hosted grant, but it must not contain raw secrets. Provider secrets
are delivered through the normal runx credential boundary.

Adapter preference is explicit and local to the operator. Use `.runx/data-sources.json`
for a project default, or `RUNX_DATA_SOURCES` for a one-run override. The graph
still passes only `data_source_ref`; it does not get to choose Redis over SQLite
unless the operator binds that source to Redis.

## Provider Adapter Contract

A provider adapter is a normal Runx tool manifest. It accepts the internal
operation envelope plus the non-secret `data_source_binding` injected by the
runtime. Operators and agents call the four exact data operations; they do not
call this adapter envelope directly:

```json
{
  "schema": "runx.tool.manifest.v1",
  "name": "data.postgres",
  "source": {
    "type": "cli-tool",
    "command": "node",
    "args": ["./run.mjs"],
    "input_mode": "stdin"
  },
  "inputs": {
    "operation": { "type": "string", "required": true },
    "data_source_ref": { "type": "string", "required": true },
    "data_source_binding": { "type": "json", "required": false },
    "resource": { "type": "string", "required": true },
    "aggregate_id": { "type": "string", "required": true },
    "expected_version": { "type": "number", "required": false },
    "idempotency_key": { "type": "string", "required": false },
    "event": { "type": "json", "required": false },
    "limit": { "type": "number", "required": false }
  },
  "scopes": ["runx:data:read", "runx:data:append"],
  "output": {
    "packet": "runx.data.operation_result.v1",
    "wrap_as": "data_operation_result"
  }
}
```

Provider adapter implementations are responsible for translating the operation
into provider-specific calls. A Postgres adapter may execute SQL internally; a
Redis adapter may call Redis commands internally; a D1 adapter may use
Cloudflare APIs internally. The model and domain skill still see only the
operation envelope and the sealed `runx.data.operation_result.v1` result.

Provider adapters must fail closed when a write's commit state is ambiguous.
They should return `provider_unavailable` only when no commit can be proven, and
must include enough provider evidence to diagnose the failure without exposing
credentials or private payloads.

## Provider Adapter Envelope

Every provider adapter accepts the same internal envelope:

```json
{
  "operation": "append_event",
  "data_source_ref": "tenant://example/board",
  "resource": "board_events",
  "aggregate_id": "posting-123",
  "expected_version": 2,
  "idempotency_key": "posting-123:claim:agent-9",
  "event": {
    "type": "posting.claimed",
    "payload": {}
  }
}
```

And return:

```json
{
  "schema": "runx.data.operation_result.v1",
  "data_source_ref": "tenant://example/board",
  "provider": "postgres",
  "operation": "append_event",
  "resource": "board_events",
  "aggregate_id": "posting-123",
  "status": "committed",
  "before_version": 2,
  "after_version": 3,
  "idempotency_key": "posting-123:claim:agent-9",
  "event_ref": "board_events:posting-123:3",
  "result_digest": "sha256:...",
  "projection_digest": "sha256:...",
  "redactions": []
}
```

Adapters may include provider evidence, but not credentials or raw secrets.
When an event carries an explicit `type`, adapters use it as `event_type`.
When a domain skill emits the generic runx effect packet shape instead, adapters
derive `event_type` from `effect_family.operation`, for example
`operator_inbox.disposition`. If neither field exists, the event remains
`data.event`.

## Provider Rules

SQL providers should expose named query templates and append/update routines,
not free-form model SQL. Redis providers should expose declared commands by
purpose, not arbitrary command strings. Object stores should expose keyed read
or append operations with content digests and size caps. Product APIs should
declare the same resources and operation names even if they are backed by HTTP.

All writes need an idempotency key. Versioned resources should require
`expected_version`; append-only streams still return the before and after
versions so replay can prove order.

## Continuation and offline migration

`data.read_events` has two intentional modes. Omit `after_version` to read the
latest bounded tail. Supply it—including `0` for the first forward page—to read
events in ascending order whose version is strictly greater than that cursor.
Continue with `next_after_version` while `has_more` is true. Every page is
bounded to 500 events, the cursor must advance monotonically, and an operation
failure is a failure packet rather than an empty event array. Stream-head lists
use their returned opaque keyset cursor; callers do not synthesize offsets.

Native SQLite opens only the current schema. Legacy stores are migrated out of
band under exclusive access:

```bash
runx data migrate \
  --database .runx/data/events.sqlite \
  --source tenant://example/events \
  --json
```

The database and optional `--backup` path are workspace-relative. Runx creates
a SQLite-consistent backup, fingerprints a recognized legacy schema, assigns
the supplied source only to formerly unscoped rows, installs the current schema
and indexes, rebuilds stream heads and projection digests from ordered events,
then independently verifies event/stream counts, content digest, and readback.
The typed proof records source, backup, and result digests. A current database
returns an idempotent `current` proof; an unknown or partial schema remains
byte-identical and no backup is created.

## Domain Example

A domain skill decides whether a transition is allowed and emits a domain
transition packet. A graph then calls `data-store.append_event` with the
packet. The data adapter appends it to the declared resource only if the
current stream version matches `expected_version`. A later turn reads events or
a projection to resume.

No domain enum belongs in runx core. The data plane stores and proves the
transition. The domain skill and its app-specific reducer own the meaning.

### Dogfood A Stateful Inbox Journal

This is the current end-to-end local proof. It uses the public `operator-inbox`
skill, the public `data-store` skill, and a logical source binding. The
operator-inbox graphs do not know whether the storage backend is SQLite or
Redis: each runner plans a deterministic transition packet, appends it through
`data-store`, and no model runs anywhere in the turn.

For SQLite, bind the source to a local database:

```json
{
  "data_sources": {
    "tenant://dogfood/sqlite/inbox-1": {
      "adapter": "data.sqlite",
      "database_path": ".runx/data/inbox-1.sqlite",
      "resources": {
        "operator_inbox_scans": {
          "kind": "event_stream",
          "partition_key": "aggregate_id"
        },
        "operator_inbox_actions": {
          "kind": "event_stream",
          "partition_key": "aggregate_id"
        }
      }
    }
  }
}
```

For Redis, keep the same logical resources and change only the binding:

```json
{
  "data_sources": {
    "tenant://dogfood/redis/inbox-1": {
      "adapter": "data.redis",
      "endpoint": "redis://127.0.0.1:6379/0",
      "key_prefix": "runx:dogfood:inbox-1",
      "resources": {
        "operator_inbox_scans": {
          "kind": "event_stream",
          "partition_key": "aggregate_id"
        },
        "operator_inbox_actions": {
          "kind": "event_stream",
          "partition_key": "aggregate_id"
        }
      }
    }
  }
}
```

Record a scan page:

```bash
RUNX_DATA_SOURCES=.runx/data-sources.json \
runx skill skills/operator-inbox record_scan_page \
  -R .runx/receipts \
  -i data_source_ref=tenant://dogfood/sqlite/inbox-1 \
  --input-json expected_version=0 \
  -i observed_at=2026-07-14T00:00:00.000Z \
  --input-json scan='{"scan_id":"scan-demo-1","provider":"demo","query_digest":"sha256:1111111111111111111111111111111111111111111111111111111111111111","page_index":1,"status":"complete","started_at":"2026-07-13T23:59:00.000Z"}' \
  --input-json messages='[]' \
  -j
```

The run seals in one pass: the `plan` step derives the transition packet and
its idempotency key deterministically, and `append` lands it on
`operator_inbox_scans` only if the stream is still at `expected_version`.
`record_action_observation` and `record_disposition` follow the same
start-and-seal shape against `operator_inbox_actions`, advancing
`expected_version` per aggregate as the stream grows. The checked-in
`skills/operator-inbox/fixtures/record-*.yaml` files show the exact input
shapes.

Then read the stream:

```bash
RUNX_DATA_SOURCES=.runx/data-sources.json \
runx skill skills/data-store read_events \
  -R .runx/receipts \
  -i data_source_ref=tenant://dogfood/sqlite/inbox-1 \
  -i resource=operator_inbox_scans \
  -i aggregate_id=sha256:1111111111111111111111111111111111111111111111111111111111111111 \
  --input-json limit=10 \
  -j
```

The readback labels each event from its packet, for example
`operator_inbox.scan_page`. Switching the `data_source_ref` from the SQLite
binding to the Redis binding exercises the same skill graphs against Redis. No
graph edit, provider branch, or inbox-specific storage code is required.

## Security Gates

- require explicit resource, tenant, stream, or partition keys;
- cap rows, event count, object size, and response bytes;
- redact fields declared secret or private;
- reject broad exports and schema-free reads;
- separate read scopes from append/update scopes;
- make retries idempotent;
- fail closed on ambiguous commit state;
- seal result digests and provider evidence, not credentials.

## Current OSS Proof

The OSS data plane has two storage implementations:

- Native SQLite uses in-process transactions, optimistic concurrency,
  idempotency keys, bounded indexed reads, and a constant-size rolling
  projection. It is the default for unbound `local://...` refs. `data.sqlite`
  is its runtime binding identifier, not a parallel executable tool.
- `data.redis`: a Redis adapter that uses a Redis list for the event stream, a
  Redis hash for idempotency keys, and one Lua script for atomic append,
  optimistic-concurrency, and idempotency checks. It is selected by binding a
  logical source to `adapter: "data.redis"` with a non-secret endpoint and key
  prefix.

Future providers must implement the same operation result shape behind their
own adapters; naming a future provider does not imply it ships today.

The public catalog entry is `data-store`; its graphs use the four exact native
operation capabilities. `data.redis` is an external provider implementation,
not a duplicate domain skill.

## Durable Composition Examples

The public skills intentionally compose the data plane instead of embedding
storage semantics:

- `operator-inbox.record_scan_page`, `record_action_observation`, and
  `record_disposition` plan an inbox transition deterministically and append
  the packet through `data-store`.
- `ops-desk.operate_from_projection` reads a projection before asking the
  operator agent to propose next actions.
- `business-ops.route_and_append` classifies one business signal and persists
  the routed packet for replay.

These examples run against native SQLite fixtures today. The same graph shape
can run against Redis once the logical source ref is rebound to that adapter.
