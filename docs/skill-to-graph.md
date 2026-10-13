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

Graph steps may also execute a registry skill:

```yaml
steps:
  - id: build_docs
    skill: registry:runx/sourcey@1.0.0
    runner: sourcey
    inputs:
      objective: refresh the public docs
```

Executable registry refs are explicit, `registry:<owner>/<name>[@<version>]`.
Before the run, `runx skill` fetches each one the project does not hold, exactly
as it fetches a top-level ref, into the project's
`skills/<owner>/<name>/<version>/`, and repeats for the skills those name. The
run resolves the step from there, so the runtime itself never reaches a
registry. A ref with no version names the one version the project holds.
`RUNX_REGISTRY_DIR` or `--registry <dir>` names a local registry instead of the
hosted catalog.

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
across skill roots. Registry refs must be explicit
(`registry:<owner>/<name>[@<version>]`) and arrive as executable registry steps
do: `runx skill` fetches each one the project does not hold before the run, and
the turn reads it from the project's skills.

The gates are intentionally narrow:

- `context_skills` is accepted only on `agent` steps.
- Local refs must be relative paths, must not contain `..`, must not target
  private graph stages under `skills/<name>/graph/<stage>/`, and must contain a
  valid `SKILL.md`.
- A context skill with an `run.yaml` catalog entry cannot use an implementation-only
  role (`graph-stage`, `runtime-path`, or `harness-fixture`).
  Internal catalog entries are context-loadable only when they explicitly declare
  `catalog.role: context`.
- Registry refs resolve from the project's fetched skills, or a named local
  registry.
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
submits one governed runx run at a time, reads receipts and projections, and
continues only when explicit stop policy allows.

## Acts: Why A Governed Call Is Made

By default a receipt records that a step ran. A governed call can also say
what act it performs and why. An `operation` step declares it as `act:`; an
agent's operation call carries it as its `act` argument:

```yaml
steps:
  - id: close
    operation: tracker/item.update
    input: { item_id: $input.item_id, state: closed }
    idempotency_key: $input.idempotency_key
    act:
      form: review               # observation (default), reply or review
      purpose: Close the reviewed item
      reason: $act.reason        # what the invoker says about this call
      target: $input.item_locator
      decision: close            # continue, close, decline, defer, escalate or monitor
      previous: $input.review_receipt
```

`form` and `decision` take only those values, and `purpose` is required: an
act outside them refuses the call before it is admitted. The act is bound into
the call's admitted intent, so an approval of the call approves its act and a
changed act is another call. It never reaches the provider.

The act sits in the call's own receipt. Who acted, under what grant, term and
mandate, and why that was legitimate come from admission, never from the act:
the act's legitimacy names the execution term admission derived. What the call
produced comes from the operation, which declares its `effect`.

`$act` is the act the run's invoker states: `runx skill <skill> --act '<json>'`, an MCP
call's `act` argument, or a harness case's `caller.act`. A step reads it only
inside an `act:`, whole (`act: $act`) or by field (`purpose: $act.purpose`);
no other value may read it. A `skill` step's `act:` is the act its child run
is invoked with, which the child reads as its `$act`. An act that resolves to
nothing leaves the call without one.

## HTTP Steps And Credentials

Every executing Runx CLI command captures one workspace environment when the
command starts. Help and version rendering remain independent of workspace
state. Runx first resolves
the workspace from the process environment and current directory, then parses
the exact `<workspace>/.env` file when it exists. The file only fills missing
keys, so an exported process value always wins, and it may not set Runx's own
`RUNX_` variables. Runx parses the file as data; it does not source a shell or
mutate the process environment.

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
`runx connector use <connector> --credential <name>` (the secret on stdin),
`--session` or `--hosted`. See [Credentials](./credentials.md) for the connector
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
      event: $steps.decide.claim_packet
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

Paths are dotted, and a numeric segment indexes a list. A step's result is
the values it declares: a reference reads an output's value, never the
artifact envelope runx stores it in. References may sit at any depth, so one
input can be assembled from several steps:

```yaml
steps:
  - id: select
    agent: { role: scout, task: select }
    outputs: { selection: object }
  - id: review
    operation: example-crm/claim.read
    input:
      claim_id: $steps.select.selection.claim_id
      audience: $input.audience
```

`inputs: $input` passes the whole run input. No run input reaches a step it
does not bind. An absent optional run input leaves itself out of the object
or list that holds it; a step reference must resolve, and one that does not
fails the step with the keys that were available. A literal string that
begins with `$` is written `$$`. A bare `select.result` is an ordinary string.

A condition runs its step only when one field it reads, of the run's input or
an earlier step's result, equals or does not equal a literal:

```yaml
when: { value: $steps.select.selection.decision, equals: ready }
```

A step whose condition does not hold is skipped, and so is every step that
reads a skipped step. A condition reads a field, never a literal or a whole
result, and the literal it compares with reads nothing (`$` is written `$$`).
A condition whose field resolves to nothing when its step is reached stops the
run; an absent optional input is nothing, not `null`, so give an input a
`default` when a step branches on it. A runner's `policy.guards` read and
compare the same way, each for the one `step` it gates, and block that step
unless they hold.

An ask's `reason` is the text a person reads; the values they need to decide
belong in its `inputs`, which are shown beside it.
