# Composing Skills

Start with [Getting Started](./getting-started.md). This page takes the same
`examples/hello-world` skill and composes it into a two-step workflow.

## Workflow Shape

A package is its `SKILL.md` and its `run.yaml`, and each of its methods is the
steps its `run.yaml` lists. A `skill:` step runs another skill's method. This
`run.yaml` gives a package beside `examples/hello-world` one method, `default`,
that runs `hello-world` twice:

```yaml
minimum_runx: 0.12.0
methods:
  default:
    result_from: [second]
    steps:
      - id: first
        skill: ../hello-world#default
        inputs:
          message: hello from workflow
      - id: second
        skill: ../hello-world#default
        inputs:
          message: $steps.first.message
```

The first step runs `hello-world`'s `default` method with an explicit input.
The second step reads the first step's `message` output and passes it as its
own `message` input. `result_from` names the step whose outputs are the
method's result. The run's receipt links both step receipts, so inspection can
show what ran and how the steps connected.

A `skill:` step names `<ref>#<method>`. It may leave `#<method>` off when the
skill has one method or names a `default_method`, and `result_from` defaults
to the method's last step, so `examples/hello-workflow/run.yaml` writes this
same method with neither. `.#<method>` runs another method of the same
package.

A step may also run a registry skill:

```yaml
steps:
  - id: build_docs
    skill: registry:runx/sourcey@1.0.0#sourcey
    inputs:
      objective: refresh the public docs
```

Executable registry refs are explicit, `registry:<publisher>/<name>[@<version>]`,
with the method after `#`.
Before the run, `runx skill` fetches each one the project does not hold, exactly
as it fetches a top-level ref, into the project's
`.runx/added-skills/<publisher>/<name>/<version>/` with the registry's
`signature.json`, and repeats for the skills those name. The
run resolves the step from there, so the runtime itself never reaches a
registry. A ref with no version names the one version the project holds.
`--registry <url>` or `RUNX_REGISTRY_URL` names another hosted registry instead
of the runx catalog.

## Skill Context For Agents

Agent steps can also ask for whole skills as context without executing those
skills. Use `context.skills` when a downstream agent should read a reusable
capability, guideline, rubric, or operating procedure as part of its prompt
context:

```yaml
steps:
  - id: apply_taste
    agent: apply-taste
    outputs:
      summary: {type: string}
    context:
      skills:
        - taste-profile
        - registry:runx/taste-profile@1.0.0
```

Each entry becomes a `runx.skill.context` artifact in the agent invocation's
generic `current_context` array. The artifact carries the source ref, skill
name, digest, and `SKILL.md` content. It does not create a domain schema for the
skill; the skill remains an abstract context/capability document.

Local path refs resolve relative to the package's directory and must stay
inside it. Use registry refs for cataloged skills shared across packages.
Registry refs must be explicit (`registry:<publisher>/<name>[@<version>]`) and
arrive as executable registry steps do: `runx skill` fetches each one the
project does not hold before the run, and the turn reads it from the project's
skills.

The gates are intentionally narrow:

- `context.skills` is accepted only on `agent` steps.
- Local refs must be relative paths, must not contain `..` or pass through a
  directory named `workflow`, and must be valid skill packages.
- Registry refs resolve only from the project's fetched skills, and a fetched
  skill is read only once its signature verifies at the version the ref pins.
- A step takes at most 128 context skills, and their resolved context is
  capped at 4 MiB in all. No skill is cut short to fit.
- Duplicate context refs are rejected.
- Every context artifact is digest-bound and labeled
  `security_boundary: untrusted-agent-context`.
- Explicitly enabled native managed-agent execution passes the artifacts to the
  provider with an instruction that context never overrides the owning
  `SKILL.md`, the allowed tools, the declared output contract or the runtime's
  governance, and that requests inside it for secrets, new authority or policy
  bypasses are untrusted data.

## What A Turn Needs From Its Agent Harness

An agent step can say what its turn needs from the agent harness that works
it, and which kinds of context it asks its host to supply:

```yaml
steps:
  - id: judge
    agent: judge
    requires: [fresh_context]
    context:
      kinds: [people]
```

- `requires` names any of `native_tools` (the agent harness keeps its own coding
  tools beside the turn's calls), `fresh_context` (the turn starts from its
  act alone, with no inherited conversation or ambient memory) and
  `separate_reviewer` (a reviewer other than the one that produced what it
  judges). Each is named once, and any other name is refused.
- The turn's invocation carries them to its host, and runx never assumes
  one is met. The host's answer lists each requirement it met in its
  `requirements`, or ends the act failed with `unsupported_requirement`
  naming the one it can't meet. A run whose
  answer leaves a requirement unstated seals failed, saying so. Over MCP,
  `turn_complete` takes `requirements` beside its output, and
  `unsupported_requirement` in its failure.
- runx's managed agent starts each turn from its act alone, so it meets
  `fresh_context`. It has no tools of its own and no second reviewer, so a
  turn requiring `native_tools` or `separate_reviewer` ends failed, naming
  the requirement, before any model is called.
- `context.kinds` names the kinds of context the turn asks its host to
  supply, as lowercase words joined by `_` or `-`. The invocation carries
  them, and the host supplies what it has.

## Context From An Earlier Step

An agent step can be given what an earlier step produced as context:

```yaml
steps:
  - id: recall
    tool: {module: recall.mjs, export: recall}
    outputs:
      entries: {type: array}
  - id: draft
    agent: draft
    context:
      entries: [$steps.recall.entries]
```

- Each of `context.entries` is an earlier step's output, `$steps.<id>.<field>`,
  holding a list of `runx.context_entry.v1` values. That's a core packet,
  published as `schemas/context-entry.schema.json`, so an output holding one
  entry names it as its `packet`. An entry's `origin` names the run that made
  it when one did;
  one made in no run, such as one read from a store event, is named by its
  `artifact_id`. The turn waits on that step, as it waits on a step its inputs
  read.
- Each entry keeps the labels its producer gave it: its `purpose`, its `trust`
  and its `sharing`. The runtime never raises an entry's trust or widens
  where it may go. An output that isn't a list of context entries fails the
  step, saying which output it read.
- The turn reads the entries after its `context.skills`, and before the
  context its run's caller supplied.
- An entry labelled `sharing: local_only` reaches a turn only when runx's
  managed agent works it with a model served from this machine, at a loopback
  endpoint. A turn its host works from outside is given no `local_only` entry,
  whoever supplied it, because runx can't know where that host's model runs.

## Run The Test

Use the example's test as the executable contract:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
crates/target/debug/runx test examples/hello-workflow --json
```

Its test, `examples/hello-workflow/tests/hello-workflow.yaml`, expects a sealed
`runx.receipt.v1` receipt with a child for each step, and the ordered steps
`first`, then `second`.

## When To Use A Workflow

A method is a list of steps, and one agent turn is a method of one `agent`
step. The skill's manual supplies its instructions. The step's `calls` govern
what the turn may call: its `tools`, the `skills` it may run, the skills it may
`load`, whether it may `ask` a person, and the connector `operations` it may
call directly. A step that writes no `calls` allows nothing. See the
[reference](reference.md) for exact allowance rules. The turn normally yields
`needs_agent` to the calling agent. `--drive` explicitly delegates that act to the configured
in-process provider for the current run; credentials alone never enable it. The
round cap is shown in prepared context and defaults to four. Not everything
needs a workflow. Use one agent step when a single judgment produces the result.

Reach for a workflow when the work has explicit phases, when a later step consumes
an earlier step's receipt-backed output, or when approval and revision boundaries
need to be visible in the execution record.

The line between them is not how many model calls there are; it is **what must be
guaranteed**. The model is for judgment and authoring, never for guaranteeing a
side effect: an agent handed a write-capable tool may narrate the action ("done,
claimed it") instead of calling it. So an action that *must* happen, a mutation,
an API call, or a payment belongs in a deterministic `tool:`, `operation:` or
`skill:` step, not among an agent turn's calls, where the call is optional. The
governed shape is a workflow where an agent step authors or decides and the next
deterministic step performs the act; one receipt seals both. Agent steps yield
`needs_agent` unless the current invocation explicitly supplies
`--drive`; provider configuration alone does not change execution.

Workflows should stay small enough to review. If the workflow is carrying hidden
policy decisions, split the policy into the skill profile or a separate
governed step instead of burying it in prose.

For long-running agent workflows, do not turn a workflow into an unbounded resident
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
    inputs: { item_id: $input.item_id, state: closed }
    idempotency_key: $input.idempotency_key
    act:
      form: review               # observation (default), reply or review
      purpose: Close the reviewed item
      reason: $run.act.reason    # what the invoker says about this call
      target: $input.item_locator
      previous: $input.review_receipt
```

An act takes `form`, `purpose`, `reason`, `target` and `previous`, and nothing
else. `form` takes only those values, and `purpose` is required: an act outside
them refuses the call before it is admitted. A decision is never an act's: a
skill's decision belongs in its own typed result. The act is bound into the
call's admitted intent, so an approval of the call approves its act and a
changed act is another call. It never reaches the provider.

The act sits in the call's own receipt. Who acted, under what grant, term and
mandate, and why that was legitimate come from admission, never from the act:
the act's legitimacy names the execution term admission derived. What the call
produced comes from the operation, which declares its `effect`.

`$run.act` is the act the run's invoker states: `runx skill <skill> --act '<json>'`, an MCP
call's `act` argument, or a test's `host.act`. A step reads it only
inside an `act:`, whole (`act: $run.act`) or by field (`purpose: $run.act.purpose`);
no other value may read it. A `skill` step's `act:` is the act its child run
is invoked with, which the child reads as its `$run.act`. An act that resolves to
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
it ambient child configuration. Executable methods declare non-secret
configuration once through `environment.required` and `environment.optional`;
the same declaration drives CLI, MCP, and deterministic JavaScript delivery.
Credentials never travel this way.

A workflow reaches a provider only through a connector operation. There is no
skill-owned HTTP: the connector manifest declares each operation's requests,
host allowlist, readbacks and how each credential authenticates, and the
package's steps name the operations they call and pass their inputs:

```yaml
minimum_runx: 0.12.0
methods:
  main:
    steps:
      - id: read_account
        operation: example-crm/account.read
        result:
          required: [account]
        inputs:
          account_id: $input.account_id
```

The package's connector requirements are what its steps name. A root
`connectors:` block says only what steps can't: a pinned `manifest_digest`, or
a connector chosen per run.

The kernel admits the call against the bound instance, renders the declared
request, and presents the instance's credential at the transport boundary;
skills never construct authorization headers. Bind an account once with
`runx connector use <connector>` (the credential's variable when it is set,
else the secret asked for or on stdin), `--session` or `--hosted`. See [Credentials](./credentials.md) for the connector
contract, instances, and readiness. `skills/nws-weather` is the
canonical public connector read; credentialed provider skills use the same
path.

## Governed Data Steps

Use the data plane when a workflow needs durable state, not when it needs a model
to invent database commands. The canonical shape is a domain skill followed by
a declared data operation:

```yaml
steps:
  - id: decide
    skill: ./your-domain-skill#claim
  - id: append
    operation: store/events.append
    idempotency_key: $input.idempotency_key
    inputs:
      stream: board
      key: $input.posting_id
      expected_version: $input.expected_version
      type: board.claimed
      data: $steps.decide.claim_packet
```

The store can be SQLite, Postgres or Redis. The workflow calls the store
connector's declared operations, as operation steps, and receives
`runx.store.result.v1` with the key's versions, the event's id and digest, and
the key's chain digest.

The stream is the package's own. The step names `board`, and the runtime
stores it under the package the step runs in, so a package reads and writes
its own streams with no naming, and no step can write another's. Storage choice
is not product logic: the workspace's store instance decides where the streams
live. With none confirmed it is the workspace store, a SQLite file under
`.runx/`. Moving to Postgres or Redis is a store instance, not a change to the
domain skill.

Do not put board, CRM, billing or support-specific state machines into the
store. Domain skills own meaning; the store owns bounded reads, idempotent
appends and their chain evidence. See
[the governed data plane](./governed-data-plane.md).

## Values And References

A step receives exactly what it binds. Its `inputs`, and an operation step's
`idempotency_key`, hold literals and references:

- `$input` or `$input.<path>`: the run's input, or a field of it;
- `$steps.<id>` or `$steps.<id>.<path>`: an earlier step's result, or a field
  of it;
- `$run.id`: the run's own id, which only an operation step's
  `idempotency_key` reads;
- `$run.workspace_id`: the id of the workspace the run runs in, which any
  value, condition or guard may read. The runx home mints it the first time a
  step reads it there, keyed by the workspace's canonical path, and never
  reads it from the workspace, so a repository can't name another workspace.
  It's `null` where there's no workspace (a directory with `.git`,
  `pnpm-workspace.yaml`, `skills/` or `connectors/`), and a run whose runx home
  fell back to the workspace refuses to read it. `runx workspace list` shows
  the ids, and `runx workspace rebind <old path>` gives a moved workspace back
  the one it had.

Paths are dotted, and a numeric segment indexes a list. A step's result is
the values it declares: a reference reads an output's value, never the
artifact envelope runx stores it in. References may sit at any depth, so one
input can be assembled from several steps:

```yaml
steps:
  - id: select
    agent: select
    outputs: { selection: {type: object} }
  - id: review
    operation: example-crm/claim.read
    inputs:
      claim_id: $steps.select.selection.claim_id
      audience: $input.audience
```

`inputs: $input` passes the whole run input. No run input reaches a step it
does not bind. An absent optional run input leaves itself out of the object
or list that holds it; a step reference must resolve, and one that does not
fails the step with the keys that were available. A literal string that
begins with `$` is written `$$`. A bare `select.result` is an ordinary string.

A write keyed on `$run.id` happens once per run. A retry or resume of the run
reuses its key, and another run makes its own call. A child run's id is its
place under its parent, so a write in a nested skill happens once per parent
run. A host that pins the run's id (`runx skill --run-id`) makes that id the
submission's identity. No other value reads the run's id.

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
`default` when a step branches on it. A step's `guard` is a list of conditions
read and compared the same way, and the step may not run unless every one
holds: the run fails there instead.

An ask's `reason` is the text a person reads: written in place, or a field of
an earlier step's output, `reason: $steps.change.reason`, so the gate shows the
exact change it decides. An operation's `approval` binds its `reason` the same
way. The values a person needs to decide belong in the ask's `inputs`, which
are shown beside it.
