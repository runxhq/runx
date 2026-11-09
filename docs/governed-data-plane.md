# Governed Data Plane

runx supports stateful work without becoming a database. The data plane is the
boundary between domain skills and the storage they keep their records in.

## Shape

- A **domain skill** owns product meaning: board transitions, review states,
  CRM records, approval inboxes, support tickets, ledgers.
- The **store** connector keeps a package's ordinary state as event streams.
  It declares four operations: `events.append`, `events.read`, `key.inspect`
  and `keys.list`.
- The **private** connector keeps the person's own data, such as memory and
  people, with the same operations and the same tables, plus `keys.search`.
- A **store instance** chooses where the streams live: SQLite, Postgres or
  Redis. Skills never name a backend.
- A **store result** (`runx.store.result.v1`) is the evidence a skill reads
  and a receipt seals: one shape per operation, each naming the stream and
  key it covers, the backend that served it, and its digests.

The model never writes SQL, Redis commands or migrations. A workflow calls the
store's declared operations with typed inputs, and the kernel admits each call
like any other connector operation.

## Who may touch a stream

A stream belongs to a package. The author writes the stream's own name,
`accounts`, and the runtime places it under the package the call is made from:
`runx/reddit/accounts` for the runx-maintained reddit skill, `acme/counter/counts`
for a package acme published, or `project:counter/counts` for one the project
authored. Who a package is comes from how it was bound, never from what its
manifest says, so a project package that calls itself `reddit` writes
`project:reddit/...` and never `runx/reddit/...`.

- A package reads and writes its own streams with no naming.
- A package dependency, a `registry:` reference, is its own package, so it
  reads and writes the same streams called directly or through any wrapper.
- Code a package incorporates, a relative `skill:` path inside it, belongs to
  the package carrying it, its streams under the carrier: a community
  package's `./github` writes `acme/foo/github/...` and never meets acme's own
  `github`.
- A package outside the one that reaches it, such as a workspace skill a turn
  loads, is bound by itself: `project:<name>`, or its signer's name, so it
  writes `project:<name>/...` and never under its loader.
- An append has no `package` field and a stream is one name with no `/`, so
  another package's stream can't be spelled on a write.
- A read of another package's stream names it with `package`, and is admitted
  only under a term the run's authority states for that stream.
- Every event records its writer: the package and the digest of the exact
  bytes it ran from, set by the runtime. An event's digest covers its writer,
  type and data.
- A turn's own call never appends to either connector; writes go through a
  package's deterministic step.

Every `private` call needs a term, its owning package's included, and only the
owning package writes. A term on `private` caps only `private` calls, because
the two connectors are separate instances.

## Where the streams live

With nothing configured, `store` is the workspace store, a SQLite file at
`.runx/store.sqlite` beside the kernel's own database, and `private` is its own
file in the runx home, `private.sqlite`. A home that fell back to the
workspace, with `HOME` and `USERPROFILE` unset, counts for nothing, so
`private` refuses there and says to set `HOME`. Stateful skills need no setup
to run locally.

To keep the streams somewhere else, confirm an instance with its URL:

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

A configured instance serves every operation of its connector. `private`
includes `keys.search`, which only SQLite implements, so a `private` instance
on Postgres or Redis is refused when it's configured, never on a call. The
`store` and `private` files carry their own SQLite application ids, so neither
connector opens the other's file.

## Operations

| Operation | What it does |
|---|---|
| `events.append` | Append one event, its `type` declared beside its `data`, when its key is at `expected_version` |
| `events.read` | Read one page of a key's events in version order: the latest page, or the page after `after_version` |
| `key.inspect` | Read a key's version, its newest event whole, and the digest that chains its events |
| `keys.list` | List one page of a stream's keys, newest commit first, each with its newest event, optionally by type |
| `keys.search` | On `private` only, rank one stream's keys by a full-text match against each key's newest event |

An append is keyed by the idempotency key the kernel derives from the call's
intent, which includes the caller's `idempotency_key`. The same call answers
as `replayed` in any run, as the first append did. An idempotency key used for
a different event answers `idempotency_conflict`, checked first and never worth
retrying; a key at another version answers `version_conflict` with its current
version, retried after a read. Nothing is appended on either conflict, and a
conflict is the append's answer, not a refusal, so a workflow can read again
and decide.

Every append is confirmed. The kernel reads what the store holds under the
call's key, which must be the event the append reported, or nothing after a
conflict, before the receipt seals.

Every backend keeps the same streams with the same digests: a key's
`chain_digest` chains its events in order. A store moved from SQLite to
Postgres or Redis answers the same reads with the same digests.

`keys.search` quotes its query, so search syntax is plain words, and searches
what's current, never history: an append replaces its key's entry. Matches come
in score order, ties broken by key, a page at a time.

## Stores from before streams had owners

runx 0.12 opens only its own store version. A store an earlier runx wrote
refuses to open, on every backend, and says so: 0.12 doesn't read it, and
moving the file or database aside starts a new store. Its rows are never read
or moved. An older runx refuses a 0.12 store the same way, rather than misread
it.

## Domain example

A domain skill decides whether a transition is allowed and emits a transition
packet. Its workflow calls the store's `events.append` operation, which appends
the packet only if the key is still at the version the skill read. A later
turn reads the events or inspects the key to resume.

No domain enum belongs in runx core. The store keeps and proves the
transition; the domain skill and its reducer own what it means.

## Security

- The store's operations need the `store.read` or `store.append` scope.
- Reads are bounded: 500 events per page and 100 keys or matches per page.
- Inputs are typed and bounded by the connector's schemas; a stream, key or
  event type that is not a safe identifier is refused.
- A store's password is a credential, delivered only to the store its
  instance pins, and redacted from every result.
- The workspace store is a different file from the kernel's protected store,
  with its own SQLite application id. A store refuses to open the kernel's
  database or any file that is not a runx store.

## Durable composition examples

The public skills compose the store instead of embedding storage semantics:

- `operations-review.operate_from_projection` reads one key of another
  package's stream, under a term naming it, before asking the operator agent to
  propose next actions.
- `contact-consent-review` reads a contact's key in its own `consent` stream,
  appends the decided transition and reads the key back.

Their tests run against the workspace store. The same workflows run against
Postgres or Redis by confirming a store instance; no skill changes.
