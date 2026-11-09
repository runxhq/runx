# runx reference

Architecture and contributor reference for the runx OSS repo: the CLI surface,
the trusted Rust local runtime, generated contracts, extension protocols, SDKs,
tests, local receipts, registry, runx-maintained skills, and packaging. For the
overview and quickstart, start at the [README](../README.md).

The npm CLI package is `@runxhq/cli` and exposes the `runx` binary.

## Your First Skill In 5 Minutes

Start with the checked-in hello-world skill:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
export RUNX_RECEIPT_DIR="$(mktemp -d)"
crates/target/debug/runx skill examples/hello-world \
  -i message="hello from docs" \
  --json
```

On macOS 26, complete the
[Developer Tools permission prerequisite](../CONTRIBUTING.md#macos-developer-tools-permission)
if the initial build stalls.

When no signer is configured, local `runx skill` runs sign with the runx home's
own key. Every `runx test` test signs with the public fixture key whatever
signer is configured, so its receipts stay deterministic.
Publishing and hosted verification require a hosted issuer.

Then inspect the emitted receipt. The full walkthrough is in
[getting-started.md](getting-started.md), and the next step is
[composing-skills.md](composing-skills.md).
For governed code changes, see [github-issue-to-pr.md](github-issue-to-pr.md).

## Payment Boundary Proof

Run `runx test fixtures/skills/mock-pay` for deterministic local simulation and
`runx test skills/pay/tests/hosted-contract.yaml` for the public hosted
contract. The former always records `money_moved: false`; the latter uses
fixture provider responses and does not prove live settlement. Real rail
dogfood, credentials, ledger state, recovery, and finality live in Runx Hosted.

## Requirements

Native CLI:

- Rust 1.97+
- The native Rust CLI path must stay useful without Node, pnpm, tsx, or
  TypeScript packages installed.

Workspace and npm wrapper:

- Node.js 20+
- pnpm 10+

## Install For Development

```bash
pnpm install
pnpm build
pnpm test
pnpm typecheck
pnpm verify:fast
pnpm rust:check
```

Contributor setup, test selection, and commit sign-off rules are in
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Local CLI

For a live creator workflow, link the global `runx` binary to this checkout once:

```bash
pnpm cli:link-global
```

Then invoke the linked `runx` binary from anywhere. Use explicit paths outside
a runx workspace; bare skill names resolve from the current workspace's
`skills/` directory.

A reference names the method it runs after `#`, as in
`runx skill ./skills/skill-build#design`, and `runx skill inspect` and
`runx mcp` take the same form. A reference with no `#method` runs the
package's `default_method`, or its only method. A package directory, or a file
in one, is that package: it never falls through to a registry lookup.

```bash
runx --help
runx skill /path/to/runx/oss/fixtures/skills/echo -i message=hello --json
cd /path/to/runx/oss
runx skill ./skills/skill-build#design -i objective="build sourcey docs skill" --json
```

Recommended flows:

```bash
runx skill runx/skill-build#build -i objective="Create a bounded documentation decision skill" -i target_dir=skills/docs-demo
runx list skills
runx registry search weather --json
runx skill acme/weather@1.0.0 --registry https://runx.example.test --json
runx registry install acme/weather@1.0.0 --registry https://runx.example.test --json
runx skill github-issue-to-pr -i repository=acme/app -i issue_number=42 -i objective="Fix the failing build"
runx resume <run-id> answers.json
runx mcp --run <run-id>
runx history
runx history <run-id> --json
runx verify <receipt-id> --json
runx mcp ./fixtures/skills/echo
runx skill ./skills/skill-build#design -i objective="build github review skill"
runx test ./examples/hello-world
runx config set agent.provider openai
runx config set agent.model gpt-6-sol
runx secret set openai
runx config set agent.secret openai
```

`runx resume` continues a paused run from its run record. A host reads a run
with `runx history <run-id> --json` (`runx.run_read.v1`): its state, every
request it waits on with the digest an answer binds to, whether a process
drives it, and its events stream by stream. A turn call's event carries the
`call_id` its host pinned, and its outcome: a completed call's output as
project artifacts of the run (one per packet output, named, and one for the
rest, each with its digest and media type), and a refused or failed call's
reason, so a host reads every call's outcome from the run alone. The read's
`cursor`, passed back as `--after`, returns only what followed. A sealed run is
terminal; inspect its receipt with `runx verify <receipt-id>`. To execute
the work again, invoke the original `runx skill` command and retain both
receipts if you need to compare the outcomes. The current CLI has no standalone
`replay` or run `diff` command.

The resume document (`runx.resume.v1`) keeps agent work and human
authorization distinct, and binds every entry to the request it answers:

```json
{
  "answers": {
    "summarize": {
      "request_digest": "sha256:…",
      "value": { "result": {} }
    }
  },
  "approvals": {
    "deliver": {
      "request_digest": "sha256:…",
      "approved": true,
      "reason": "I authorize this exact provider action."
    },
    "send/retry": {
      "request_digest": "sha256:…",
      "value": { "retry": true }
    }
  }
}
```

Each `request_digest` is the one the run reported for that request: in the
paused output's `requests`, and in `runx history <run-id> --json` under
`waiting`. An entry whose address the run does not wait on, or whose digest
belongs to another request, refuses the whole resume, so a decision never
reaches a request that changed after the person saw it, and nothing answers a
request the run has not asked yet. The pending run's `answers_template` is
already bound.

`answers` is the agent's lane: agent acts. `approvals` is the person's: an
approval takes `approved` and `reason`, a claimed call's retry takes `value:
{"retry": true}`, and questions take `value: {<question id>: {option?: <label>,
text?: <string>}}`, each naming one of its question's options or, where it
allows free text, words of the person's own, every required question answered;
each entry is checked against the request it answers. Runx records every decision it admits under `approvals` in
the run before it takes the run further (`answered` in `runx history <run-id>
--json`), so a decision admitted before a crash is used by the run's next
driver, and a resume with no entries continues such a run. The same decision
delivered again is recorded once, a different one replaces a decision no step
has used yet, and one delivered after its request was answered names no
request the run waits on, and is refused. An agent's answer is checked by the
step that uses it and recorded when used, so one its step refuses is never the
run's.

Only entries under `approvals` carry host-attested human approval provenance.
The local CLI cannot authenticate a person from JSON: by placing a decision
there, the host asserts that a human approved the exact pending gate. An agent
must never author or promote that entry; it returns the pending approval to its
human or to an integrated host that authenticates the decision. Never place a
consequential decision under `answers`; that section records agent or
caller-supplied work and cannot resolve an approval gate. Runx rejects an agent
response presented to an approval request.

A host ends a run instead with `{"close": {"outcome": "declined"}}`
(a person stopped it) or `"superseded"` (a new attempt replaced it). The close
comes alone. It seals any run that is not sealed and that no process drives,
whether it waits at an act or stopped between steps, from its record alone:
nothing is bound first, so a package that changed since the run started never
traps it open. The run seals with that outcome and the seal reason
`host_closed`; it is allowed while a retry waits, and an effect the run left
unconfirmed stays unconfirmed.

A driver stops gracefully on `SIGTERM` (`runx resume`, and `runx skill` while
it drives a run) and on MCP's cancellation of an in-flight call
(`notifications/cancelled` to `runx mcp`): the run admits no further step or
call, an operation already reaching its provider returns and is recorded, a
command it supervises is terminated and recorded with decision `stopped`, and
the run stays where it stopped for its next driver (`runx resume` exits 143).
A second `SIGTERM`, `SIGINT`, and the death of the driver's parent interrupt at
once.

`runx mcp --run <run-id>` serves the agent turns a paused run waits on as MCP
tools over stdio, scoped to that run alone. `runx mcp <skill-ref...>` serves
the same tools beside its skills, over the runs its own skill calls open. A ref
with `#method` offers that method under the skill's name; a ref without one
offers its default method and each other method only a connection can work. The tools are fixed and take a turn's address: `turn_list` shows
everything the run waits on, each open turn as a line, `turn_status` reads a
turn, `turn_read` reads what a locator of it names, `turn_call` makes a call in
it and `turn_complete` ends it. `turn_status` gives the act's instructions and
inputs whole and its context as an index: each entry's locator, type, summary,
scope and size, never its data. `turn_read {turn, locator, offset?, limit?}`
reads an entry by its locator (`current:<n>`, `historical:<n>`), an input of
the act as `input:<name>`, or a call of the turn as `call:<index>`: a string
as it is, anything else as JSON, a page of at most 32,000 characters with the
`next_offset` that reads on from what it delivered. A locator the turn does
not hold is refused. A turn's address is the id of the step
that asks it, as an approval's is its gate. A turn that a call opens in a child
skill is addressed by the child's place in the run, as in `answer/0#report`,
and is worked on the same connection. `turn_status` lists the calls a turn may make, one per entry of
its agent step's `calls` (`tools`, `operations`, `skills`, and `ask_human` when
the step says `ask: true`), with their input schemas. An agent step grants
direct provider calls with `calls: { operations: [posthog/insight.read] }`,
each as `<connector>/<operation>`, and the package's connector requirements
are derived from those entries and from its operation steps. Omitting the
field or listing none offers and admits no connector operation in that turn.
Namespaced connectors use the same syntax as operation steps, such as
`fixture/forge/issue.read`. This limits agent calls, as `calls.tools` and
`calls.skills` do; operation steps remain part of the method's declared steps.
Narrowing never changes a connector's credential, approval, scope or provider
semantics. The catalog rules a turn may load packages under are its
`calls.load`, separate authority. Writing an entry grants nothing, and a turn
is given only what its `calls` names, so a step that should only interpret
supplied inputs names no calls.
`ask_human` takes `{reason}`, a yes or no, or `{questions}`: one to three, each
`{id, prompt, options?: [{label, detail?}], free_text?, required?}` with at most
four options, the recommended first; a question without options takes text.
Each call is admitted against the turn's allowance and the run's authority,
executed, and recorded in the run; the turn's receipt names every call it made.
A host pins its own `call_id` on a call, unique within the turn: a call made
again with that id returns the recorded call rather than running twice, and one
naming another tool or input is refused. Calls made at once run at once: each
is admitted and claimed in order under the session, the run's one driver,
executes apart from it, and is recorded under it again. Where a call reaches the
world it takes the session's effect gate: a native tool that only reads, or an
operation that reads, shares it; anything else, a tool runx runs as a process
included, holds it alone, inside a child run too. So effects never overlap and
reads never wait on each other, and the run's record, by `call_id`, is what a
host reads back, whatever order the calls finished in. `turn_status`,
`turn_read`, `turn_list` and `turn_complete` wait until no call is executing. A completed call returns each packet
output's project artifact reference under `artifacts`, by output name; where a
tool's or skill's input is of a packet type, a call may pass a reference to any
packet the project holds in place of the packet, one an earlier call returned
or one an earlier run left. Runx reads it from the project's artifact store
(the artifact an earlier call of the turn recorded, when one did), checks its
digest and the input's packet schema, gives the input its content, and names
the artifact, labeled with the input, in the call's receipt; a reference whose
digest is not its content's, or that leaves the artifact store, is refused.
An input explicitly typed `runx.project_artifact.v1` receives the descriptor
itself; it is not automatically dereferenced. The child can then open its exact
retained content through `artifact.open` with the required scope and bounds.
A skill call states its `act` beside
the child's inputs, as an operation call does; it becomes the child run's act.
A write names its `idempotency_key`, which a retry reuses. A call whose outcome
runx cannot confirm stays pending on its own retry request, which says whether
sending it again is safe (`resend`: a documented free read, a journaled file
change, or a call its provider deduplicates, including billed usage); a safe
one runs again with the same input when `runx resume`
answers it under `approvals` with `{"retry": true}`, and an unsafe one is never
resent: check what it did, then close the run. A bounded retry (one with
`retry_after_seconds`) is runx's own: the next resume after its delay runs it. A call that needs approval stays pending until `runx resume`
answers it under `approvals`, by the address the paused run lists.
`turn_complete` ends a turn with its output once none of its calls is pending,
and the run continues; an act that cannot be done ends with
`{"failure": {"reason": ...}}` instead, which fails its step with that reason,
and a skill call whose child act failed settles failed with it. A run whose host authority sets `max_fanout` or
`max_child_depth` refuses a skill call that would start more child runs in one
turn, or nest them deeper, than it allows. A host sets what the run may change
with `effects` and where its tools may go with `reach` (see Effects and reach,
below); a host whose own agent works the run sets `effects: ask` and
`reach: workspace`.

A run's authority may also set `max_spend_micro_usd`, the most the run may spend in
integer micro-USD (1 = $0.000001); `runx skill --max-spend <usd>` sets it for a
local run. Every priced call's quote, and every amount a call moves in USD or
USDC, is reserved against what is left before anything is sent, so a call over
it is refused as `cost_limit_exceeded` and seals the run blocked. Child runs and
turn calls spend from the same limit, and none can widen it. A call that
proves nothing billable gives its reservation back. A payment in any other
currency is refused under a limit, since it cannot be counted. Unknown provider
prices are also refused under a money limit; missing price information is not
zero. With no money limit, no dollar reservation is made, while scoped use
limits, charge custody and applicable approval still hold. A run started with `--drive` is worked by the
managed agent: `runx mcp --run` refuses it, and `--drive` cannot take
over a run its host drives.

Hosted and replay drivers may bind `runx skill` or `runx resume` with
`--package-digest` and `--execution-closure-digest`. Runx recomputes both at the
native execution boundary, persists them in pause checkpoints, and rejects a
resume whose supplied bindings disagree with that checkpoint. These flags bind
execution; they are never skill inputs. The closure digest includes the
immutable Runx release identity, so a pending continuation cannot silently
cross a runtime upgrade. Runx-generated run ids and durable workflow state bind
the same package and execution-closure digests, so changing a skill, transitive
child package, or runtime closure cannot merge incompatible checkpoint history.

Execution ceilings remain owned by the capability they constrain rather than
being flattened into one misleading global number: a provider operation request
timeout, a managed-agent request timeout, a native command argument budget, and
a file-bundle budget are different contracts. Their constants use capability-qualified names. Limits
that are selectable per invocation or materially shape a sealed execution are
typed at the invocation boundary. An invocation that hits one fails, and its
failure message names the ceiling it hit; the receipt seals the failure, not
the limits.

Credential redaction and untrusted-data admission are also distinct. Exact
values delivered by Runx enter the run-scoped taint set and are scrubbed at
every output boundary. External configuration and provider output can contain
raw material Runx never delivered, so those narrow boundaries additionally
reject exact secret-field names before the data can enter receipts. That check
is not a general text heuristic and never removes URLs or unrelated operator
diagnostics.

With `agent.provider`, `agent.model` and a key configured, the CLI can resolve
managed agent work directly. Deterministic tools, approvals, and required human
inputs keep their existing local behavior. `anthropic`, `openai` and `gemini`
are built-in providers, each reading its own key variable when `agent.secret`
is unset; `openai` speaks OpenAI's Responses API. Runx names no model of its
own: every provider sets `agent.model`, and a managed run refuses without one.
Any other provider sets `agent.dialect` (`chat_completions`,
`anthropic_messages` or `openai_responses`) and `agent.endpoint_url` too, and
reads only `agent.secret` or `RUNX_AGENT_API_KEY`; a local OpenAI-compatible
server is such a provider, speaking `chat_completions`. A provider's refusal
reaches the receipt as its status and its own error message, never its raw
body or the key.

Prepared context never asks for performative approval: it binds the selected
skill, inputs, execution closure, and drift guards into the receipt. A
consequential action stops once at its owning boundary. An `operation` step
accepts an optional typed `approval` request, and an operation the connector
marks `approval: required` always needs one; the kernel then suspends the call
for one exact host-attested human decision. Otherwise the admitted grant is
sufficient and no performative gate is invented. Use an explicit workflow approval only when no
native effect owns the consequential decision. There is no persistent or
environment-based auto-approval override; development configuration must not
silently acquire live authority.

Provider-backed skills list the connectors they call in `run.yaml`; bind an
account with `runx connector use <connector>`: the credential's variable when it
is set, else the secret asked for or piped on stdin, or `--session` or
`--hosted`. See [Credentials](credentials.md) for the storage contract.

Ctrl-C (or the terminal's configured interrupt shortcut) interrupts the whole
active Runx context, including supervised tool, JavaScript, adapter, and MCP
child process groups. Runx allows a two-second cleanup window, exits with status
130, and treats a second interrupt as an immediate exit. On macOS, Cmd-C is
normally copy; Ctrl-C is the interrupt.

### Skill result JSON

`runx skill ... --json` and `runx resume ... --json` return the canonical
`runx.skill_run.v1` envelope. A sealed run has one caller-facing `result`, a compact
`trace` for workflow runs, its `outcome` (with a `seal_reason` when the outcome
alone does not say why), and the `receipt_id` needed for independent
inspection:

```json
{
  "schema": "runx.skill_run.v1",
  "status": "sealed",
  "outcome": "completed",
  "skill_name": "example",
  "run_id": "run_example_123",
  "receipt_id": "sha256:...",
  "result": {},
  "trace": {
    "graph": "example-read",
    "status": "succeeded",
    "steps": [
      {
        "step_id": "read",
        "skill": "operation",
        "status": "success",
        "receipt_id": "sha256:..."
      }
    ]
  }
}
```

`result` is the selected method's normalized output: the declared contract of
its successful result step. When a guard or an authority refused a
step, `refusal` carries why (`code` and `message`) beside what the steps
produced; a failed run's `result` is its failure. Neither is part of the signed
receipt, whose seal records the typed `outcome` and, when the outcome alone
does not say why, a `reason` from a closed vocabulary. `trace` deliberately carries
references rather than step payloads. Intermediate workflow state, child receipts,
and the full signed receipt are written to the local Runx state and receipt
stores; process output identity is committed by the receipt. Inspect proof
through `runx history` and `runx verify` instead of paying to copy it through
every agent response. Paused runs keep their exact `requests` and resume
instructions.

### Structured input documents

Use `-i name=value` for small interactive calls; each value is read by the type
its method declares, a string as written and any other type as JSON. For
generated or reusable input, pass the complete method input object through one
workspace-contained JSON document:

```bash
runx skill ./skills/research#research --inputs request.json --json
cat request.json | runx skill ./skills/research#research --inputs - --json
```

`--inputs` is exclusive with `-i`. A file path is resolved
relative to the invocation workspace and must identify one contained, regular,
UTF-8 file. File and stdin input are capped at 64 MiB, must decode to exactly
one JSON object, and then enter the same selected-method defaulting and type
validation as inline inputs. Runx does not echo a rejected document in its
diagnostic.

This is a control-document transport, not a high-volume execution profile. Put
large immutable content behind the digest-bound artifact/page boundary and put
durable histories behind data cursors rather than carrying either through workflow
context.

`runx skill inspect` also accepts `--inputs file|-`. Inspection binds known
caller inputs to check concrete connector targets under `--run-authority`;
values produced by later steps remain unknown until execution. Every actual
call rechecks its target and current grant before provider execution.

The global link points at `packages/cli` in this checkout. Rebuild with
`pnpm build`; do not reinstall.

## Package Topology

Rust owns the trusted local runtime path. The Rust crate graph is the enforced
boundary map:

- `runx-contracts`: domain-neutral Rust contract types plus shared schema
  emission and reconciliation; domain crates contribute the contracts they own.
- `runx-core`: pure state-machine and policy decisions.
- `runx-parser`: pure skill, method, workflow step, test, and tool manifest
  parsing.
- `runx-receipts`: canonical receipt model, hashing, signatures, and tree
  verification.
- `runx-runtime`: impure local runtime, adapters, exact process invocation and
  supervision, test replay, journals, registry clients, payment gates, MCP,
  and execution.
- `runx-cli`: native `runx` binary over the runtime.

The TypeScript package graph is the client, protocol-extension, wrapper, and
generated-contract layer:

- `@runxhq/contracts`: generated validators and TypeScript types over the
  Rust-owned schema artifacts.
- `@runxhq/cli`: npm distribution wrapper and client presentation around the
  native CLI.
- `@runxhq/client`: finite native invocation and portable response formatting
  over generated contracts.

For the generated package export index, see [docs/api-surface.md](api-surface.md).

`runx-runtime` is the canonical local runtime. It owns local skill, workflow,
test, receipt, history, policy, authority, payment, typed
execution-boundary evidence, MCP, and built-in adapter execution for the native
CLI path.

TypeScript remains for generated contracts, CLI/client wrappers,
cloud/product integrations, host adapters, and protocol helper SDKs over
language-neutral protocols. Host adapters can shape host responses over
the runx host protocol; they do not own local execution. Tool authors target
manifests and process contracts, so they do not need Rust, `runx-core`,
`runx-runtime`, or a fork of the core repository. Source-event ingress, hosted
runtime binding, and catalog/read-model access are separate protocol lanes.

Command-surface ownership:

| Surface | Canonical owner | TypeScript role |
| --- | --- | --- |
| `runx skill` local execution | `runx-runtime::execution` via `runx-cli` | npm launcher/client wrapper |
| `runx test <path>` | Rust test replay | tests and wrapper views |
| receipts and history | Rust receipt store and journal | display/client views |
| policy, authority, payment, x402 | Rust core/runtime policy | published type mirrors and product UX |
| governed data operations | the store connector's native driver | generated types, helper SDKs |
| non-execution extension protocols | lane-specific Rust/cloud owners | generated types, helper SDKs, provider glue |
| skill authoring | `runx-runtime` authoring service plus `skill-build` | npm launcher only |
| marketplace and docs projection | Rust-owned catalog/contracts | generated docs and client views |

Stateful product work should use the governed data-plane shape in
[docs/governed-data-plane.md](governed-data-plane.md): domain skills own
meaning, while the store connector keeps bounded reads, append-only event
writes and key reads.

Workflows call the store's operations, `events.append`, `events.read`,
`key.inspect` and `keys.list`, as operation steps. A step names a stream's own
name and a key, and the runtime places the stream under the package the step
runs in: a package reads and writes its own streams with no naming, and reads
another's only by naming its `package` under a term. With no store instance
confirmed, the streams live in the workspace store, a SQLite file at
`.runx/store.sqlite`; `runx connector use store --parameter url=...` moves them
to another SQLite file, Postgres or Redis. The person's own data uses the
`private` connector, its own file in the runx home, where every call needs a
term.

Large local inputs use `artifact.admit` and `artifact.read`. Admission snapshots
one contained regular file, binds its media type, byte count, and whole-file
digest to an opaque logical-run-scoped reference, and never exposes the host
path. The current total admission ceiling is 512 MiB. Reads return exact
offsets, range and whole-file digests, `next_offset`, and `eof` in pages of at
most 4 MiB using base64, character-safe UTF-8, or JSON-array record framing;
the default page is 1 MiB. The 4 MiB limit is a page ceiling, not a total-file
ceiling. `fs.read` and `fs.read_bundle` share the same containment and hashing
owner for bounded text; they are not alternate large-file transports.

`fs.list` and `fs.search` list and search beneath a root with the project's
ignore files honoured as Git honours them; both skip `.git`, `.runx` and any
nested Git worktree unless the request names a path inside it. `fs.search`
also searches the one file its path names. `git.log`,
`git.show` and `git.diff` read commits with their changed files, one commit or
one file at a revision, and patches between revisions or against the working
tree, all bounded.

`fs.edit` replaces exact text in one file, `{path, edits: [{old, new,
replace_all?}]}` applied in order: each `old` occurs once, or at least once with
`replace_all`, or the call is refused with why. Every change is bound to what it
was written against: `fs.write` and each `fs.apply_bundle` write take the
`expected_digest` a read returned (`content_digest`) for the file they replace,
or `create: true` for a file that must not exist, and a delete may be `{path,
expected_digest}`; `fs.edit` is bound to the content it read. A change whose
file differs from what it names is refused, naming it, and each file is checked
again immediately before its atomic rename. Within `reach: workspace` a change
to a file that exists must name its digest, so an agent never replaces content
it did not read.

Every change a run makes through these tools is journaled in the run's kernel
state: before a bundle's first rename runx records what each file held and the
directories it will create, and after it, how it ended. A bundle a crash left
begun is rolled back by the run's next driver, or by its close, so a bundle is
applied whole or not at all. Each tool's output names its `journal` entry and
each file's `before_digest`.

**Effects and reach.** A run's authority says what the run may change,
`effects`, and where its tools may go, `reach`. A host sets both on a run, and a
method or step in `run.yaml` can only narrow them: `none` is narrower than
`ask`, and `ask` than `allow`; `workspace` is narrower than `inputs`. What
applies to a step is the narrowest of its host's, its method's and its own, a
turn's calls run under their agent step's, and a child run holds what the step
that started it holds. A value wider than the host's narrows nothing. With no
host, a run takes `effects: allow` and `reach: inputs`; that means only that no
host set them, never that a person ran the command.

`reach: workspace` keeps every native file, git, command and authoring tool
inside the workspace. A root outside it, or inside `.git` or `.runx`, is
refused, and no tool reads, writes, lists, searches or shows a secret: an
environment file other than a template (`.env.example`), a private key or
certificate bundle, `.npmrc`, `.netrc`, `.pgpass`, or anything under `.git` or
`.runx`. A patch or commit leaves such files out and names them.
`reach: inputs` reaches the roots the run's inputs name, another repository
included. A process the run starts is not confined by either; it runs with its
user's rights.

**The approval floor.** `effects` is decided on the floor, at any depth, before
a step or call is claimed, so nothing waiting on the person is ever "may have
run". Under `effects: allow` nothing waits. Under `effects: ask` every change
with no approval of its own waits on a person: a step that changes the
workspace or runs a process waits on an approval of gate type `tool` (a native
tool that changes something, `fs.edit`, `fs.write`, `fs.apply_bundle`,
`runx.skill.apply`, `command.execute` or `process.start`, a catalog tool runx
runs as a process, and a workflow's process step), which carries exactly what
executes: for an edit each file with its unified diff and the digests of what
it replaces and becomes, for a command its argv, directory and digest. A
connector operation that writes, whether an agent's turn call made it or a
skill's fixed step reached it, waits on its call's one gate, of gate type
`write`: admission opens it bound to the call's exact intent, carrying the
call's input, and lists every reason the call waits, so a call that may also
consume provider credits with no quote names that reason on the same gate and
the person's one approval answers both. A write that carries an approval of its
own, its operation's declared one or its step's `approval`, keeps that one and
the floor adds none. A package's own `store` state is exempt, unless its step carries an
`approval`. A person who declines it refuses the call. Under `effects: none`
every change is refused, at any depth: every local effect, every provider write
even with its own approval, every `store` write, and every purchase.

A turn's own call never writes to `store`, at any `effects`: a stream is
written by a package's deterministic step, and a turn's call to a `store`
write is refused saying so.

`standing_approvals` on the run's authority are a person's bounded
confirmation, each exactly one of `{"edits": {"paths": [...]}}` (changes to
files under those workspace-relative paths, by whole components; `""` is the
whole workspace), `{"commands": {"argv_prefix": [...]}}` (a command whose argv
begins with those exact tokens; argv has no shell, so a prefix admits nothing
beyond it), `{"operations": {"names": [...]}}` (a write by a connector
operation named `<connector>/<operation>`, or any operation of exactly one
connector, `<connector>/*`) and `{"local": true}` (every local effect the floor
holds). A covered step runs without pausing, and its record and receipt name
the entry that admitted it. A `runx resume` document may carry
`standing_approvals` in the person's lane: runx records the new set, runs every
later step under it, and approves each open floor approval it covers. Standing
approvals never cover a write's own approval.

**What an approval executes.** Every approval gate carries what executes once it
is approved: an operation its input, a local effect what the floor shows, an ask
its `{reason}`. `summary.payload_digest` is that value's stable digest
(`runx.stable-json.v1`); the value rides as `payload` up to 64 KiB and beyond
that as `payload_artifact`, a project artifact holding its canonical JSON. A host
recomputes the digest before it offers Approve.

**Processes that outlive a call.** `process.start {command, args, cwd, env,
tty?: {rows, cols}, max_seconds}` starts a dev server, a watcher or an
interactive program and returns its `process_id` (`runx.process.v1`); the floor
holds it on its argv like a command. Each runs under a supervisor of its own,
the runx binary run again in its own session, which owns the child (in a
pseudo-terminal when `tty` is given, on Unix hosts), writes its output to a
bounded log in the project's runx state, stops it at `max_seconds` (3600 by
default, at most 86400), and records how it exited. It outlives the run that
started it and belongs to that run's actor: any later run of the same actor in
the project reaches it. `process.read {process_id, after?, max_bytes?}` returns
the output after a cursor (`runx.process.output.v1`: stdout and stderr tagged,
or the terminal's output, with the `next` cursor and the bytes the log no longer
keeps), and whether it still runs; `process.write {process_id, input}` types to
its stdin or terminal, control keys included, a write sent again under the same
call applied once; `process.resize {process_id, rows, cols}`; `process.stop
{process_id}` terminates it, then kills it; `process.list` lists them. A start
is recorded, keyed by its call, before its supervisor is spawned, and the
supervisor confirms it under a lock it holds for its life, so the same call
sent again returns the same process and a start no supervisor confirmed is
settled as failed once its lock is free. The person lists, reads and stops
them with `runx process list [--actor ref]`, `runx process read <id> [--after
cursor]` and `runx process stop <id>`.

A `command.execute` a run makes is followed the same way while it runs,
whichever driver runs it: it is recorded as a process of kind `command`, named
by its `call` (`<run id>#<step id>@<attempt>`), its output written to that
process's log as it is read, so `process.read` and `runx process read` follow it
by cursor, and a stop sent to it stops the command. A command takes no input,
and its call's result still carries its whole bounded output.

### Local Process Boundary

Process tools (`tool: { command: ... }`) are trusted host code.
Their exact non-secret configuration is declared through
`environment.required` and `environment.optional`; credentials use the
separate credential contract. Runx controls exact argv, cwd, delivered
environment, stdin, timeouts, bounded output, process groups or Job Objects,
kill-tree behavior, and cleanup.

Receipts record `trusted_host_process`. That is an honest observation, not a
filesystem, network, or syscall-confinement claim. Capability and provider
scopes still govern the native or hosted operations Runx performs on the
skill's behalf; a local subprocess cannot turn those strings into OS
permissions.

Authors declare newly required JavaScript helpers, for example
`tool: {module: inventory.mjs, requires: [parseData]}`. Unsupported declared
requirements fail workflow admission before effects. Runx does not infer helper
calls or use this declaration as a permission; omitting it loses the old-runtime
guard. The baseline `parseUrl` helper predates this declaration.

JavaScript modules use a different boundary. Their worker receives no ambient
OS environment, credentials, workspace path, network API, process API, clock,
or randomness; only the validated in-memory module closure, JSON input, exact
declared non-secret environment, and fixed limits cross the worker protocol.
Native integer inputs outside JavaScript's safe integer range are refused
before module execution rather than silently rounded. Pass exact large IDs or
amounts as strings. Floating-point inputs retain IEEE 754 semantics.

`artifact.store` retains a selected canonical JSON result as the existing
ProjectArtifact descriptor. `artifact.open` checks its current-project root,
size and digest, then returns a local artifact URI for `artifact.read` pages.
Store and open require `reach: inputs` and `artifact.write` / `artifact.read`
scopes respectively. The existing `artifact.read` page tool requires `fs.read`;
it accepts logical-run-scoped URIs admitted by either source. References do not
grant permission. Use immutable result parts and a
compact continuation instead of growing history in CLI input.

## Capability Packs

Runx is the generic execution engine. Product workflows stay outside the runx
CLI and ship as local skills, methods, and tools in the consuming repo.

The intended extension model is:

- `runx` owns generic runtime, thread, outbox, receipt, and handoff machinery
- service repos own their product workflows as local capability packs
- operators execute those workflows through normal skill invocation
- CLI, API, and GitHub-comment triggers all normalize into the same capability
  execution envelope, while the thread stays the review/control object

From inside a product repository, `runx skill ./skills/<workflow>#<method> -i
name=value` resolves that repository's local capability pack. A workflow is
never a privileged engine command: there is no `runx docs ...` or
`runx outreach ...` path inside the engine.

`github-issue-to-pr` follows the same boundary. runx owns the generic source-thread to
scafld to PR machinery; service repos own Slack, Sentry, owner assignment, and
publish policy. See [docs/github-issue-to-pr.md](github-issue-to-pr.md).

## Standalone Skill Packages

The `skill-build` skill authors a package. It inspects the target and catalog,
requests a closed architecture decision and change draft, binds both to the
inspected package digest in native code, stages the exact candidate and
replays its tests, then applies the same bytes transactionally to `target_dir`, which
must stay inside the workspace. It owns no templates and never adds a
placeholder JavaScript module. Nothing needs initializing first: a repository
or the `skills/` directory marks the project.

```bash
runx skill runx/skill-build#build \
  -i objective="Create a bounded documentation decision skill" \
  -i target_dir=skills/docs-demo
```

Without `--drive`, the run stops at `needs_agent`, prints an answers
template and exact `runx resume` command, and leaves the target untouched. Add
`--drive` only when this run should use the configured in-process model
loop. To cold-start without installing Runx first, invoke the same command
through `npx @runxhq/cli`.

Community skills should be authored and published as standalone packages
through this lane. The main `runx` repo is the lane for runx-maintained skills
and runtime code, not the community package catalog.

Prefer declarative workflows composed from native tools and existing skills. When
irreducible deterministic domain computation needs JavaScript, declare a worker
step, `tool: { module: ... }`; the module receives resolved inputs and returns
JSON while Runx owns the process protocol. A frozen second argument carries
exact manifest-declared non-secret environment values when needed. Use a
process tool, `tool: { command: ... }`, only for a real executable or protocol
boundary. See
[Skill Author Runtime Contract](skill-author-runtime-contract.md).

Registry search and install now normalize public trust into three tiers:
`first_party`, `verified`, and `community`. Richer provenance and attestation
metadata still travels with the registry row, but the user-facing install/search
surface stays readable.

`runx registry package <SKILL.md|skill-dir> --json` projects the exact
parser-owned publish artifact without writing a registry row; given a
`connector.yaml` or a directory holding one, it projects a connector instead. Hosted and
third-party publishers should consume that output instead of implementing
their own package-file discovery. A skill package carries each connector it
declares at `connectors/<name>/` (its manifest, the tools its
process operations run, and its `README.md` and `mark.svg` when present), and
its metadata binds every declared connector by `ref`, `version`,
`manifest_digest`, `connector_digest` and `runtime_capability`. Every package
carries `runx.package.json`, which marks the installed package's root.

`runx registry package <connector-dir> --json` projects a connector published
on its own: the same files at the package root, the parsed manifest,
its content digest (over every file, which names the version and which the
registry signs), and its manifest, connector and operation digests. The
connector names itself: its manifest `id` is its whole name.

Connectors are published only to a hosted registry: the runx catalog unless
`--registry <url>` or `RUNX_REGISTRY_URL` names another.

- `runx registry publish <connector-dir>` sends that package; the registry
  reads it natively, publishes it only when the authenticated publisher owns
  the name's namespace, and the CLI checks the recorded ref and content digest
  are the package's. Its version is `sha-` and the first 12 hex digits of the
  content digest.
- `runx registry install <name>[@version] [--digest <content-digest>]`
  adds what is published under that name. For a connector, the version goes to
  the project's added connectors,
  `.runx/added-connectors/<name>/<version>/`. The signed manifest
  must name that connector (never a skill), version and content digest under a
  trusted key scoped to its namespace, and the staged files must load and
  repackage as exactly those files before they are renamed into place. A
  version directory is written once and never rewritten: installing it again
  is `unchanged`, and other files under the same version are refused.
- `runx registry install --records <file or -> [--registry <url>]` installs
  signed skill version records a host already holds, such as an operation's
  closure, in place of a ref: one JSON array of the records the registry
  serves at `/v1/skills/<publisher>/<name>/versions/<version>/record`, from a
  file or stdin as `--inputs` reads its document. Each is verified against the
  keys trusted for the registry `--registry` names (the runx catalog unless it
  names another) and installed into the project exactly as a publish installs
  its closure's records; a record that does not verify stops the install with
  the same typed error as any install. A closure holds only skills: the
  connectors a skill binds travel inside its record's package files.

The packages you write sit beside your source; the packages runx adds sit in
the project directory, `.runx/` (or `RUNX_PROJECT_DIR`). `skills/` holds the
skills you write (`skills/<name>/`, or `skills/<publisher>/<name>/` for your own
copy of a registry skill) and `connectors/` your own connectors
(`connectors/<name>/`). `.runx/added-skills/` holds every skill added from a
registry by `runx registry install`, by running a registry ref or as a
registry step's child (`<publisher>/<name>/<version>/`), each with the
registry's signed manifest as its `signature.json`. That signature is the
package's identity: a run binds it as `<publisher>/<name>` only while it
verifies against the package's exact bytes, and refuses it otherwise, so a
committed package keeps its identity in CI and on a teammate's machine, and
nothing else in the workspace can state one. Install marks both directories
`* -text` in their `.gitattributes`, so a CRLF checkout never changes those
bytes. The core skills this release carries materialize into the runx home,
at `<home>/bundled/<name>/<version>/`, never into the project.
`.runx/added-connectors/` holds installed connectors (`<name>/<version>/`) and
the runtime capability connectors (`store`, `web`, `artifact`) a skill uses,
added from the release and checked against it on every read. Yours take
precedence: a skill or connector of the same name in `skills/` or
`connectors/` is used over what runx added, whatever version or manifest a
reference pins, so a locally edited copy composes with everything that names
it. A connector binding pins the digests it was confirmed for, so an edited
copy is bound again before it holds a credential. The runtime capabilities
are never replaced. A connector has one name, such as `github` or
`google/calendar`. Every other connector, runx-maintained ones included, is a
published package: a published skill carries the connectors it binds, and a
local skill binding a connector outside its own plugin pins that connector's
`manifest_digest` once `runx registry install` has added it. A version
directory is written once; other bytes under it are refused. The runx home
keeps keys, configuration and state, not packages.

Use [skill-catalog.md](skill-catalog.md) for the maintained category list,
catalog search flow, and duplicate-check standard before proposing a new
first-party skill.

## Skill And Execution Profile

Executable skills split authored skill content from execution profiles. A
package's `run.yaml` is its execution profile:

```text
skills/github-issue-to-pr/
  SKILL.md
  run.yaml
```

Direct execution accepts the package directory or the `SKILL.md` inside it. A
lone markdown file is not a skill package.

In a workspace that owns a `skills/` catalog, a root `SKILL.md` is the
workspace operator manual exported to agents; it is not an executable package
boundary around the repository. Executable package discovery starts at
`skills/*/SKILL.md`. A standalone workspace root remains a package, and a root
`run.yaml` explicitly opts a catalog workspace root into package ownership.

Execution profiles use a strict YAML subset: no anchors, aliases, merge keys,
custom tags, multi-document markers, duplicate mapping keys, or unknown profile
fields. `minimum_runx` is required and read before anything else, so a package
that needs a newer runx is refused by name. Keep capability and receipt
mappings explicit in the method that uses them.

A method's result comes from its last step unless `result_from` names its
intentional public result producers. A connector requirement is derived from
the steps that call it, so this package declares no `connectors:` block:

```yaml
minimum_runx: 0.12.0
methods:
  publish:
    description: Publish one post to a submolt, once its exact text is approved.
    result_from: [publish]
    steps:
      - id: publish
        operation: moltbook/post.create
        idempotency_key: $input.idempotency_key
        approval:
          reason: Approve publishing this exact post.
          type: moltbook_post
        result:
          required: [post]
        inputs:
          submolt_name: $input.submolt
          title: $input.title
          content: $input.body
```

A method states its purpose in `description`: what it does and when to use it
rather than the skill's other methods, in a sentence or two. A model chooses a
method by it: the tool a turn offers for the method carries it, a load lists
it, and `registry.search` and `runx list skills` show it. A method without one
is described by its skill's description, so a skill's only method needs none;
each method of a runx-maintained skill with several declares one.

The verb, scopes, target and readbacks come from the connector operation, and
the kernel confirms a write by its declared readbacks before the step
succeeds. The approval request is optional unless the operation requires one.
When present, it is part of the intent digest and the kernel returns a
resumable approval request before dispatch, addressed by the step that asks,
with the gate `operation:<intent digest>`. The
runtime constructs the operator summary from the admitted connector,
operation, target, scopes, payload digest, intent digest, and amount when
present; package code cannot inject a second free-form account of the act. Do
not place an `ask` step next to the same operation; one effect has one approval
owner.

For generic native `fs.write`, `fs.apply_bundle`, and `command.execute` calls,
the tool's exact scopes and containment are sufficient by default. Add an explicit `ask` step only when the domain action
itself requires a human decision, such as publishing a release. Runx does not
infer that requirement from the capability's write-shaped effect.

The host protocol represents every unresolved request with the generic
`needs_agent` run state. CLI JSON and human output normalize a request set made
only of approval requests to `needs_approval`; mixed or model-authored request
sets remain `needs_agent`. Both resume the same sealed checkpoint through
`runx resume` and neither opens an interactive prompt.

The money a payment call moves is derived from its operation's inputs, which
the connector declares, so a step never states an amount. It is part of the
plan digest and appears in the operator summary. Under a money limit, `USDC`
units count as micro-USD and `USD` units as cents. The
provider operation also needs a declared USD quote, including an explicitly
documented zero when it charges no call fee. A known transfer amount does not
establish an unknown provider fee, and a missing quote is refused under the
limit rather than counted as zero.

An operation may declare what its provider charges per call:
`cost: {usd_micro, per: request | success, source: docs | rate_card | observed,
source_url}`. `per: request` is billable once the request may have left;
`per: success` only when the provider answers as the operation accepts. The
price and its basis are part of the operation digest, so a changed price is
confirmed again before it runs; where it was read is not. The quote covers the
whole operation's declared request sequence. Metered paging needs separately
accounted requests; a free reader may page. `usd_micro: 0` explicitly declares
free. Omitting `usd_micro` from a cost declaration describes metered usage whose
USD quote is unknown; omitting the declaration does not establish a free call.
A strict money limit accepts neither unknown case. Without such a limit, declared
unpriced metering uses bounded-use authority or asks once for native approval of
the exact call. An empty use-limit object is not a bound.

`runx skill inspect` shows the declaration. A call's packet carries
`cost: {quoted_usd_micro?, settled_usd_micro?, payer}` when cost is declared;
unknown quotes and settlement stay absent rather than becoming fictional zeroes.
A possibly charged timeout, invalid response or partially completed read retains
its reservation and evidence use. State idempotency and duplicate-effect
tolerance do not cover another charge from the original reservation: reconcile
the result, or admit a new separately bounded call while preserving the original
uncertain attempt. A provider's enforced deduplication must cover the whole call
and its charge, and its implementation must actually send the native key.

`result_from` is not a list of workflow leaves. It returns each selected successful
step's complete declared contract, including packet envelopes. Approvals and
intermediate evidence remain in the separate operator context and signed
receipts. Multiple names are for mutually exclusive result branches or
deliberately combined, non-overlapping contracts; two successful producers may
not emit the same key. Preparation resolves every result producer through its
exact method, tool manifest, or nested skill and rejects a missing semantic
output contract before any step executes. Declare the step's `outputs`;
transport output is never an implicit workflow result. An output with a
`packet` is kept as an artifact of that packet, and `result_as` names the
output a step's whole result becomes, with its packet on that output.

A package's tests live in `tests/<name>.yaml`, one test each, and the data they
read under `fixtures/`. The package should contain only the files the skill
uses at execution time: `SKILL.md`, `run.yaml`, deterministic method modules,
schemas, `tests/`, `fixtures/`, its `evals/`, and narrowly scoped `context/` or
`references/`. Do not add
README/changelog/setup docs, generated state, logs, screenshots, private
provider config, or broad project plans inside a public skill. The public docs
for the package belong in `SKILL.md`; external guides belong under `docs/`.

See [composing-skills.md](composing-skills.md) for composing skills into a workflow and
[publishing.md](publishing.md) for publishing and trust tiers.

Use `code-plan` for bounded change planning, `skill-build` for skill-package
design and improvement, and the named downstream execution lane for mutation.

## Tool Authoring

Reusable Runx mechanics and security boundaries belong in the native Rust tool
catalog. Skills should compose those native tools instead of shipping request,
filesystem, Git, hashing, packet, approval, credential, or receipt wrappers.
Use `runx tool inspect <name> --json` to inspect the contract that both workflows
and managed agents execute.

The repository-level tool package shape remains for a genuinely separate
executable or protocol boundary that should be callable as a tool:

```text
tools/<namespace>/<tool>/
  src/index.mjs
  fixtures/*.yaml
  manifest.json
  run.mjs
```

`manifest.json` is the execution contract and sole owner of the tool name,
source, inputs, defaults, artifact projection, scopes, and governance metadata.
Process code contains only the domain behavior the native catalog cannot
express.

```bash
runx test ./skills/<skill-that-calls-the-tool> --json
```

A tool is proved through the skill that calls it: that skill's tests run the
tool on the path a user runs.

When a `run.mjs` entrypoint is needed, it is a thin process shim and owns no
contract metadata. Loading the package validates the manifest and binds the
digests of its declared source files without rewriting either. Do not create one of these packages merely to
wrap a capability already owned by the native catalog. The declared entrypoint
must run directly on the supported Node runtime; do not probe for build output
or import uncompiled TypeScript.

## Runx-Maintained Packages

The runx-maintained catalog is explicit about why each package is public:

- canonical governed skills: `charge`, `dispute-respond`,
  `skill-build`, `skill-review`, `least-privilege`, `skill-adopt`,
  `policy-draft`, `receipt-audit`, `refund`, `operations-review`, `message-send`, `pay`,
  `invoice-pay`, `nws-weather`, `open-meteo-weather`
- context skills: `brand-voice`, `taste-profile`

Other bundled packages stay in the same `SKILL.md` + `run.yaml` shape, but are
internal, with `catalog.visibility: internal`. Owned workflow stages live below
their public skill at `skills/<skill>/graph/<stage>/run.yaml`, not as root
catalog packages.

For first-party skill proposal work, the core builder bar is explicit:
proposal packets should name the real pain being solved, explain fit against
the current runx catalog, surface maintainer decisions cleanly, and avoid
builder residue or placeholder targets.

See `docs/operator-console.md` for the manager-dashboard model that lets agents
operate projects, workspaces, products, or accounts through the same governed
lanes as the UI.

Each ships as a portable `SKILL.md` plus a colocated execution profile at
`skills/<skill>/run.yaml` when it exposes methods, with its tests beside it in
`tests/`. Bare skill names resolve to the workspace's own skills or
to the core skills this release carries. `runx/<name>` names a core skill and
nothing else, at every depth, so no workspace copy shadows it; it materializes
into the runx home at `<home>/bundled/<name>/<version>/` when first used. Every
other package runs by its `publisher/name[@version]` ref, optionally with
`--registry` and `--digest`, unless your own copy at
`skills/<publisher>/<name>/` answers it; only a trusted, signed registry
package is added to the project, at
`.runx/added-skills/<publisher>/<name>/<version>/`. A ref without a version takes
the newest version this runx reads: when the registry's current version needs
another runx, the newest older version this one reads is used instead, and the
output's `newest_unread` names the version passed over and why. A version named
outright is used as named, or refused when this runx does not read it.

Agent steps can also demand-load skills as context instead of executing them.
Reference reusable judgement, operating procedure, or capability skills from an
`agent` step's `context.skills`:

```yaml
context:
  skills:
    - ../taste-profile
    - registry:runx/taste-profile@1.0.0
```

The runtime injects each referenced `SKILL.md` as a generic
`runx.skill.context` artifact in the agent invocation `current_context`. Local
path refs resolve relative to the package; `runx skill` fetches each registry ref
the project does not hold before the run, into the project's added skills, and
the runtime reads it from there, never from a registry during execution. A
registry skill is installed only as its registry signed it, and is read only
once its `signature.json` verifies at the version the ref pins; its provenance
is what that signature states: its skill id, version and digests. Context
skills are bounded, digest-labeled, and presented to managed agents as
untrusted advisory data.

Skill steps can execute registry skills too, naming the method with `#`:

```yaml
steps:
  - id: build_docs
    skill: registry:acme/docs-build#build
```

`runx skill` fetches each one the project does not hold before the run, from the
runx catalog unless `--registry <url>` or `RUNX_REGISTRY_URL` names another
hosted registry. A registry child is added to the project's
`.runx/added-skills/<publisher>/<name>/<version>/` like any other added skill,
and a package's tests read the registry children its project holds. A
child binds only after its signature verifies at the version its ref pins, so
an installed package never runs an altered or unsigned copy.

Any runnable skill package can also be exposed locally as an MCP tool with:

```bash
runx mcp ./skills/github-issue-triage
```

That MCP surface is a thin facade over the normal runx kernel path, so receipts,
policy, approvals, and resolution requests still behave the same way.

## Receipts

Local receipts are append-only JSON files under `.runx/receipts` unless `RUNX_RECEIPT_DIR` is set. `runx verify <receipt-id>` shows what a receipt says, with its signature's `verified`, `unverified`, or `invalid` status, and verifies the tree it belongs to. Once a skill package and method resolve, a blocked preparation also seals a `blocked` refusal receipt; the JSON failure returns its `receipt_id` and prepared-context digest so rejected input or context admission remains inspectable without echoing the rejected value.

Workflow receipt lineage is an immutable one-way tree. The parent commits each
child receipt ID and exact signed-body digest; a child never names its parent,
so receipt identity stays content-addressed. Every verifier applies one rule to
a tree: each receipt is committed once by its parent, and a tree holds at most
1024 receipts and 64 levels below its root. A receipt two parents commit is
refused, and two different signed bodies under the same receipt ID are
ambiguous and fail verification.

Native `receipt.prove` and `receipt.query` expose redacted metadata and tree
verification. `receipt.read` additionally requires `receipt.claim.read` and
returns an exact retained declared output from the current workspace, after
verifying its receipt tree and signed output digest. An optional RFC 6901
`json_pointer` selects a value; `max_bytes` limits the complete serialized JSON
selection. An oversized selection is refused rather than truncated. The reader
does not load raw invocation inputs, transport diagnostics or credential stores.
Declared outputs can contain sensitive source data; they are returned exactly,
without a redaction that would change their signed meaning.

A workflow receipt returns `not_a_step` with its verified child references; it
does not directly sign an assembled result. A valid step receipt imported
without its matching local run record returns `not_retained`. The reader
neither replays the run nor searches another workspace. A run with
`reach: workspace` cannot access these protected claim bodies. A verified claim
establishes what its signer recorded, not independent truth about a provider
or the absence of secrets in data that was not inspected. A child receipt of
an open turn becomes readable through the receipt store when the existing turn
completion lifecycle publishes it.

Notarize a local receipt with:

```bash
runx notarize ./.runx/receipts/<receipt-id>.json
```

`runx notarize` posts the full sealed receipt to `POST /v1/receipts/notarize`.
The notary counter-seals it and publishes the record, so anyone can read it;
the command prints the public `/r` link and content hash the notary returns.
`runx verify --notary <document> --notary-key <key.pem>` checks the counter-seal. Configure the hosted API with `RUNX_PUBLIC_API_BASE_URL` (default
`https://api.runx.ai`) and authenticate with `RUNX_PUBLIC_API_TOKEN` (or run
`runx login --for publish`). The stored publish credential is separate
from the default operator credential used by Connect and provider effects. Runx
requires HTTPS for non-loopback API origins so the bearer token cannot be sent
to a public plaintext endpoint.

For local hosted dogfood only, point at a loopback API and opt into the private
network escape explicitly:

```bash
RUNX_PUBLIC_API_TOKEN=dev-token runx notarize ./.runx/receipts/<receipt-id>.json \
  --api-base-url http://127.0.0.1:47882 \
  --allow-local-api
```

## Tests

Run a whole skill package to execute each of its `tests/<name>.yaml`, in name
order, through the native runtime:

```bash
runx test ./skills/cve-audit --json
```

Pass a test's path instead when intentionally replaying one test:

```bash
runx test ./examples/hello-world/tests/hello-world-smoke.yaml --json
```

A test names the `method` it runs, the package's default when absent, and
`package: fixtures/<dir>` when it runs a small fixture package under
`fixtures/` instead, such as one composing this skill with others. Everything
under `fixtures/` is staged whole into each test's workspace, with
`fixtures/bin/` executable.

Tests can assert the terminal status, receipt identity and lineage, exact or
subset output, ordered steps, and exact or subset output for named steps.
Prefer semantic subsets over complete snapshots:

```yaml
expect:
  status: sealed
  receipt:
    schema: runx.receipt.v1
    child_receipt_count: 2
  steps:
    - classify
    - act
  step_outputs:
    act:
      subset:
        action_packet:
          data:
        status: awaiting_approval
```

A receipt-dependent test may seed a bounded set of existing receipts from
its own package:

```yaml
setup:
  receipts:
    - fixtures/receipt-store/sha256-example.json
```

Each path must be a normalized package-relative `.json` path. Runx parses and
verifies every seeded receipt under the test's configured signature policy
before placing it in that test's isolated store. Seed data is evidence for the
test, not proof of a provider action; the test must still assert only what
the verified receipts and method output establish.

A test's `host` stands in for what answers the run:

- `act`: the act the host states for the run, which its steps read as
  `$run.act`;
- `answers`: agent turn outputs, by the id of the step that asks;
- `turns`: agent turns the host works call by call, by the id of the step
  that asks. A turn is answered or worked, not both;
- `approvals`: approval decisions, `true` or `false`, by the address of the
  request each decides: an operation step's id or an ask's gate at the root,
  and `<path>#<id>` in a child run, `<path>` being its place under the root,
  such as `send#post`.

Its `providers` stand in for what the run reaches, exactly, with anything else
refused: `http`, `mcp`, `processes` and `hosted`, described below.

An answer or a turn script keyed by an id answers that request in every run of
the test that asks it. A key may instead be an address, such as
`answer/0#answer`, which answers only the request at that place
in the run and wins over an id. An approval decides only the request it
names.

A worked turn makes each call through the turn, as an attached agent would,
then ends with its `output`. A call names a tool the turn offers, by the name
its status lists, and may say what it must return; a call that returns
something else fails the test and is named in its error:

```yaml
host:
  turns:
    answer:
      calls:
        - tool: data_digest
          input: { value: hello, encoding: utf8_text }
          expect: { subset: { outcome: { status: completed } } }
      output: { answer: digested }
```

Supplied `host.answers` are semantic oracles, not inert model stubs. Native
replay fails unless every supplied answer is observable in the root or step
output. For stateful work, use one test to prove the transition and durable
readback; do not depend on test filename order.

A test's `providers.http` can bind exact response bytes without turning a
semantic test into a live-network smoke test:

```yaml
providers:
  http:
    - request: { method: GET, url: https://fixture.runx.invalid/source, body: none }
      response:
        status: 200
        headers: { content-type: text/plain }
        body: hello world
    - request:
        method: POST
        url: https://fixture.runx.invalid/mcp
        body: { json: { operation: status } }
      response:
        status: 200
        headers: { content-type: application/json }
        body: '{"ok":true}'
```

`providers.http` is matched on the exact method, final URL, and explicit body
identity: `body: none` means no body, while `body: {json: ...}` means one
structural JSON value, including literal null. An exchange URL is
canonicalized like the native final URL and cannot contain credentials or a
fragment. A request the test declares more than once is answered by those
exchanges in the order they are declared, and the last answers every later
repeat, so one test can read a resource, change it and read it back. A test
declares only the exchanges its run makes: one no request asked for fails the
test, so a declared write is a write the run sent. Any request no exchange
matches is refused without reaching the network; a test run has no live
fallback. Connector operations still own their real admission, request
rendering, authentication, decoding, custody and readbacks; only transport
response bytes come from the test. `providers.mcp` stands in for a connector's
MCP server the same way, matched on tool, arguments and session headers, and
`setup.credentials` stores the one credential of a listed connector whose
variable the test sets. `providers.processes` stands in for a local connector's
process tool: each entry names its `tool`, exact `arguments` and `result`.
At most 32 calls are admitted, with 64 KiB arguments and 2 MiB result per call;
duplicate tool/argument tuples are refused. When these fixtures are present,
an unmatched process call fails without starting the actual tool. Admission,
credential binding and native result checks still run. This proves fixture
replay, not actual driver behavior; run provider integration checks separately.
The test does not relax the host's process policy or affect ordinary runs.
`providers.hosted` stands in for runx Connect: it
holds the test's grant, renders a hosted operation's declared requests as
Connect does, with its `purchase_id` as the purchase, and sends them
to the test's `providers.http`. Without a `purchase_id`, a request that
sends the purchase fails closed. A test that stops on a request it did not answer names
that request in its status mismatch. No test's `host` or `providers` is
available to skill inputs, environment configuration, or ordinary live runs.

Package replay uses a disposable project-owned workspace below `.runx/test`
and a separate receipt store for every test, then cleans that scratch state.
Tests therefore cannot satisfy each other through filename order or ambient
receipt history. Receipts produced by each test are verified and copied to the
durable `.runx/receipts` store, or to the directory selected by
`--receipt-dir`. An explicit `RUNX_CWD` identifies the owning project but does
not disable test isolation.

## Doctor And Dogfood

For the core first-party skill lane, run:

```bash
pnpm dogfood:core-skills
```

This remains a TypeScript wrapper lane. The native Rust proof for local
orchestration is the Rust CLI/runtime test and fixture suite; wrapper dogfood is
useful only after the same behavior is proven without Node, pnpm, or tsx.

For the default structural verification lane during refactors, run:

```bash
pnpm verify:fast
```

That lane keeps the cheap workspace checks together: OSS typecheck plus the
fast package test surface with the current structural budget and boundary
coverage.

## Build And Pack

```bash
pnpm test tests/cli-package.test.ts
cd packages/cli
npm pack --dry-run --json
```

The package ships only the launcher (`bin/runx`, which must be executable),
`native/supported-platforms.json` and `LICENSE`; the native binary comes from
the platform package that matches the machine.

## Boundary Rules

- `oss/` (this repository) must not import from `cloud/` (the private companion workspace, not part of this checkout).
- State-machine and policy packages remain pure.
- Rust owns trusted local runtime/execution, including exact process
  supervision, receipts, policy, authority, payment, tests, and built-in
  adapters.
- No TypeScript package is a fallback executor for trusted local behavior.
- Tools own their side effects behind language-neutral manifests and process
  contracts; non-execution extension lanes have their own protocol contracts.
- External extension authors must not need Rust, a `runx-core` or
  `runx-runtime` dependency, or a core repository fork.
- CLI, SDK, IDE plugin, host adapter, and MCP entrypoints delegate to method
  contracts or external protocols instead of duplicating the engine.

### Admitted observation time

`control.observed_at` takes no inputs and returns this native invocation's
`observed_at`. It forwards admitted execution metadata; it does not read an
ambient clock or attest to a provider event time. Deterministic JavaScript keeps
its fixed clock. Acquisition skills preserve provider times separately and may
use this native value to label when Runx observed their evidence.
