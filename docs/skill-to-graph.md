# Skill To Graph

Start with [Getting Started](./getting-started.md). This page takes the same
`examples/hello-world` skill and composes it into a two-step graph.

## Graph Shape

The example graph lives at `examples/hello-graph/graph.yaml`:

```yaml
name: hello-graph
owner: runx
steps:
  - id: first
    skill: ../hello-world
    inputs:
      message: hello from graph
  - id: second
    skill: ../hello-world
    inputs:
      message: $steps.first.message
```

The first step runs the skill with an explicit input. The second step reads the
first step's `message` output and passes it as its own `message` input. The graph
receipt links both step receipts, so inspection can show what ran and how the
steps connected.

Graph steps may also execute a cataloged skill from the local registry:

```yaml
steps:
  - id: build_docs
    skill: registry:runx/sourcey@1.0.0
    runner: sourcey
    inputs:
      objective: refresh the public docs
```

Executable registry refs are explicit (`registry:...`, `runx-registry:...`, or
`runx://skill/...`) and resolve only from `RUNX_REGISTRY_DIR`. Graph execution
does not fetch remote registry content implicitly; the operator must install,
publish, or sync the skill into the local registry first. At runtime runx
materializes the resolved `SKILL.md` and optional `run.yaml` into
`.runx/registry-step-skills/` as a generated cache, then executes the normal
skill runner path against that materialized package.

## Skill Context For Agents

Agent steps can also ask for whole skills as context without executing those
skills. Use `context_skills` when a downstream agent should read a reusable
capability, guideline, rubric, or operating procedure as part of its prompt
context:

```yaml
steps:
  - id: apply_taste
    agent: { role: builder, task: apply-taste }
    outputs:
      summary: string
    context_skills:
      - taste-profile
      - registry:runx/taste-profile@1.0.0
```

Each entry becomes a `runx.skill.context` artifact in the agent invocation's
generic `current_context` array. The artifact carries the source ref, skill
name, digest, and `SKILL.md` content. It does not create a domain schema for the
skill; the skill remains an abstract context/capability document.

Local path refs resolve relative to the graph skill directory and must stay
inside the owning skill root. Use registry refs for cataloged skills shared
across skill roots. Registry refs use the local registry (`RUNX_REGISTRY_DIR`)
and must be explicit (`registry:...`, `runx-registry:...`, or
`runx://skill/...`). Graph execution does not fetch remote registry content
implicitly; install or ingest the skill first, then reference the local registry.

The gates are intentionally narrow:

- `context_skills` is accepted only on `agent` steps.
- Local refs must be relative paths, must not contain `..`, must not target
  private graph stages under `skills/<name>/graph/<stage>/`, and must contain a
  valid `SKILL.md`.
- A context skill with an `run.yaml` catalog entry cannot use an implementation-only
  role (`graph-stage`, `runtime-path`, or `harness-fixture`).
  Internal catalog entries are context-loadable only when they explicitly declare
  `catalog.role: context`.
- Registry refs resolve only from the configured local registry.
- Each context skill is capped at 64 KiB, the step is capped at 12 context
  skills, and total resolved skill context is capped at 256 KiB.
- Duplicate context refs are rejected.
- Every context artifact is digest-bound and labeled
  `security_boundary: untrusted-agent-context`.
- Explicitly enabled native managed-agent execution passes the artifacts to the provider with an
  explicit instruction that context artifacts are advisory data, not system
  instructions or authority to change tools, reveal secrets, or bypass policy.

## Run The Harness

Use the graph harness as the executable contract:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
crates/target/debug/runx harness examples/hello-graph/harness.yaml --json
```

The harness expects a sealed `runx.receipt.v1` receipt and the ordered
steps `first`, then `second`.

## When To Use A Graph

A runner is a list of steps; a runner without steps is one agent turn. In an
agent turn the skill's manual is the instructions, the skill's `tools:`,
`skills:` and connector operations are what the agent may call (and it may ask
the human when the skill declares `asks: true`), and the turn normally yields
`needs_agent` to the calling agent. `--managed-agent` explicitly delegates that act to the configured
in-process provider for the current run; credentials alone never enable it. The
round cap is shown in prepared context and defaults to four. Not everything
needs a graph. Reach for one when a single judgment step produces the result.

Reach for a graph when the work has explicit phases, when a later step consumes
an earlier step's receipt-backed output, or when approval and revision boundaries
need to be visible in the execution record.

The line between them is not how many model calls there are; it is **what must be
guaranteed**. The model is for judgment and authoring, never for guaranteeing a
side effect: an agent handed a write-capable tool may narrate the action ("done,
claimed it") instead of calling it. So an action that *must* happen, a mutation,
an API call, or a payment belongs in a deterministic `tool:` or `skill:` step,
not among an agent turn's tools, where the call is optional. The
governed shape is a graph where an agent step authors or decides and the next
deterministic step performs the act; one receipt seals both. Agent steps yield
`needs_agent` unless the current invocation explicitly supplies
`--managed-agent`; provider configuration alone does not change execution.

Graphs should stay small enough to review. If the graph is carrying hidden
policy decisions, split the policy into the skill profile or a separate
governed step instead of burying it in prose.

For long-running agent workflows, do not turn a graph into an unbounded resident
loop. Use [Loop Orchestration](./loop-orchestration.md): an outer loop host
submits one governed runx turn at a time, reads receipts and projections, and
continues only when explicit stop policy allows.

## Domain Receipts: The `act:` Declaration

By default a receipt records that a step ran. A runner can instead declare an
`act:` block so its primary receipt reads as the **domain act** it performed:
what was decided, on what, under what authority, with what effect, and what it
follows.

The discipline is a trust boundary. The model authors only the human reason; the
structure and authority come from the declaration plus trusted inputs, never from
the model, so it cannot forge what kind of act happened, what it targeted, or
under what right. A run that declares no `act:` block seals exactly as before.

Each field maps to a trusted source, a literal or driver-pinned from an input
(`<field>_from: <input>`). The reason and effect come from named steps, so the
effect is read from the deterministic action step's real result, never the
model's restatement of it:

```yaml
runners:
  approve:
    act:
      form_from: act_form        # review | revision | reply | observation | verification
      purpose_from: act_purpose  # what this act is
      target_from: target_ref    # the entity acted on (a ref uri)
      decision_from: decision    # accept | reject | continue | ... -> the receipt decision
      authority_from: authority  # the right exercised (a grant ref)
      actor_from: actor          # who acted (a principal ref)
      previous_from: prior       # the receipt this one chains from (lineage)
      reason_step: decide        # the agent step whose output holds the reason
      reason_from: line          # the field in that step's output
      effect_step: act           # the deterministic action step
      effect_from: id            # the field of its result that is the consequence id
      effect_prefix_from: ns     # wraps it, e.g. ticket: -> ticket:<id>
    steps:
      # ...
```

When the operator, not an agent, authors the reason, `reason_from_input` names a
trusted input instead of `reason_step`/`reason_from`.

Transport never enters the receipt: the tool name, url, status, and any token
stay out; only the domain act and its effect ref are sealed. See
[the act model](./act-model.md) for the receipt's act/decision
shape.

## HTTP Steps And Credentials

Every executing Runx CLI command captures one workspace environment when the
command starts. Help and version rendering remain independent of workspace
state. Runx first resolves
the workspace from the process environment and current directory, then parses
the exact `<workspace>/.env` file when it exists. The file only fills missing
keys, so an exported process value always wins. Runx parses the file as data; it
does not source a shell or mutate the process environment.

Keep `.env` local and ignored by version control. Loading a key does not make
it ambient child configuration. Executable runners declare non-secret
configuration once through `environment.required` and `environment.optional`;
the same declaration drives CLI, MCP, and deterministic JavaScript delivery.
Credentials never travel this way.

A graph reaches a provider only through a connector operation. There is no
skill-owned HTTP: the connector manifest declares each operation's requests,
host allowlist, readbacks and how each credential authenticates, and the skill
lists the operations it calls and passes their input:

```yaml
connectors:
  example-crm:
    operations: [account.read]
runners:
  main:
    steps:
      - id: read_account
        operation: example-crm/account.read
        result_fields: [account]
        input:
          account_id: $input.account_id
```

The kernel admits the call against the bound instance, renders the declared
request, and presents the instance's credential at the transport boundary;
skills never construct authorization headers. Bind an account once with
`runx credential set <connector> --credential <name> --from-stdin` or
`runx connector use`. See [Credentials](./credentials.md) for the connector
contract, instances, and readiness. `skills/nws-weather-forecast` is the
canonical public connector read; credentialed provider skills use the same
path.

## Governed Data Steps

Use the data plane when a graph needs durable state, not when it needs a model
to invent database commands. The canonical shape is a domain skill followed by
a declared data operation:

```yaml
steps:
  - id: decide
    skill: ./your-domain-skill
    runner: claim
  - id: append
    skill: ./data-store
    runner: append_event
    inputs:
      data_source_ref: "$input.data_source_ref"
      resource: board_events
      aggregate_id: "$input.posting_id"
      expected_version: "$input.expected_version"
      idempotency_key: "$input.idempotency_key"
      event: $steps.decide.claim_packet.data
```

The store can be SQLite, Postgres or Redis. The graph calls the store
connector's declared operations, usually through `data-store`, and receives
`runx.data.operation_result.v1` with version movement, digests and provider
evidence.

Storage choice is not product logic. A graph passes `data_source_ref` such as
`local://runx-data-store/dev-board`; the workspace's store instance decides
where the streams live. With none confirmed it is the workspace store, a SQLite
file under `.runx/`. Moving to Postgres or Redis is a store instance, not a
change to the domain skill.

Do not put board, CRM, billing or support-specific state machines into the
store. Domain skills own meaning; the store owns bounded reads, idempotent
appends and projection evidence. See
[the governed data plane](./governed-data-plane.md).

## Values And References

A step receives exactly what it binds. Its `inputs`, and an operation step's
call fields (`input`, `idempotency_key`, `operation_name`, ...), hold
literals and references:

- `$input` or `$input.<path>`: the run's input, or a field of it;
- `$steps.<id>` or `$steps.<id>.<path>`: an earlier step's result, or a field
  of it.

Paths are dotted, and a numeric segment indexes a list. References may sit at
any depth, so one input can be assembled from several steps:

```yaml
steps:
  - id: select
    agent: { role: scout, task: select }
    outputs: { selection: object }
  - id: review
    operation: example-crm/claim.read
    input:
      claim_id: $steps.select.selection.data.claim_id
      audience: $input.audience
```

`inputs: $input` passes the whole run input. No run input reaches a step it
does not bind. An absent optional run input leaves itself out of the object
or list that holds it; a step reference must resolve, and one that does not
fails the step with the keys that were available. A literal string that
begins with `$` is written `$$`. A bare `select.result` is an ordinary string.

Conditions and guards read the same way:

```yaml
when: { value: $steps.select.selection.data.decision, equals: ready }
```

An ask's `reason` is the text a person reads; the values they need to decide
belong in its `inputs`, which are shown beside it.
