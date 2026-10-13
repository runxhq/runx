# Governed Data Plane

runx supports stateful work without becoming a database. The data plane is the
boundary between domain skills and the storage they keep their records in.

## Shape

- A **domain skill** owns product meaning: board transitions, review states,
  CRM records, approval inboxes, support tickets, ledgers.
- The **store** connector keeps durable event streams. It declares four
  operations: `events.append`, `events.read`, `projection.read` and
  `heads.list`.
- A **store instance** chooses where the streams live: SQLite, Postgres or
  Redis. Skills never name a backend.
- A **data operation result** (`runx.data.operation_result.v1`) is the
  provider-neutral evidence a skill reads and a receipt seals.

The model never writes SQL, Redis commands or migrations. A graph calls the
store's declared operations with typed inputs, and the kernel admits each call
like any other connector operation.

## Where the streams live

A workspace has one store. With nothing configured it is the workspace store, a
SQLite file at `.runx/store.sqlite` beside the kernel's own database. Stateful
skills need no setup to run locally.

To keep the streams somewhere else, confirm a store instance with its URL:

```bash
runx connector use store --parameter url=sqlite:data/board.sqlite
runx connector use store --parameter url=redis://cache.internal:6379/0
```

A store that needs a password takes it as the instance's credential, never as
part of the URL: from `RUNX_STORE_PASSWORD` when that is set in the process
environment, otherwise asked for, or read from stdin when piped:

```bash
printf '%s' "$BOARD_DB_PASSWORD" |
  runx connector use store --credential password \
    --parameter url=postgres://board@db.internal:5432/board
```

URLs are `sqlite:<path>` (relative to the workspace), `postgres://` or
`postgresql://` (TLS unless the URL says `sslmode=disable`), and `redis://` or
`rediss://`. A URL that carries a password is refused. The instance pins its
URL, so a store is never moved under a running skill.

`data_source_ref` names a logical source inside the store. Many sources share
one store, and each source's streams stay separate.

## Operations

| Operation | What it does |
|---|---|
| `events.append` | Append one event when its stream is at `expected_version` |
| `events.read` | Read one page of a stream's events, the latest page or the page after `after_version` |
| `projection.read` | Read a stream's version, last event and projection digest |
| `heads.list` | List one page of a resource's newest stream heads, optionally by event type |

An append is keyed by the idempotency key the kernel derives from the call's
intent, which includes the caller's `idempotency_key`. The same call answers
as `idempotent_replay` in any run. A stream at another version answers
`conflict` with the current version, and nothing is appended. A conflict is the
append's result, not a refusal, so a graph can read the stream again and
decide.

Every append is confirmed. The kernel reads what the store holds under the
call's key, which must be the event the append reported, or nothing after a
conflict, before the receipt seals.

Every backend keeps the same streams with the same digests: an event's digest
covers the event, and a stream's projection digest chains its events in order.
A store moved from SQLite to Postgres or Redis answers the same reads with the
same digests.

## Domain example

A domain skill decides whether a transition is allowed and emits a transition
packet. Its graph calls `data-store` with `runner: append_event`, which appends
the packet only if the stream is still at the version the skill read. A later
turn reads the events or the projection to resume.

No domain enum belongs in runx core. The store keeps and proves the
transition; the domain skill and its reducer own what it means.

## Security

- The store's operations need the `store.read` or `store.append` scope.
- Reads are bounded: 500 events per page and 100 heads per page.
- Inputs are typed and bounded by the connector's schemas; a resource,
  aggregate or event type that is not a safe identifier is refused.
- A store's password is a credential, delivered only to the store its
  instance pins, and redacted from every result.
- The workspace store is a different file from the kernel's protected store,
  with its own SQLite application id. A store refuses to open the kernel's
  database or any file that is not a runx store.

## Durable composition examples

The public skills compose the store instead of embedding storage semantics:

- `ops-desk.operate_from_projection` reads a projection before asking the
  operator agent to propose next actions.
- `list-hygiene-judge` reads a contact's consent stream, appends the decided
  transition and reads the result back.

Their fixtures run against the workspace store. The same graphs run against
Postgres or Redis by confirming a store instance; no skill changes.
