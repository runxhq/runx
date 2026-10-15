# runx reference

Architecture and contributor reference for the runx OSS repo: the CLI surface,
the trusted Rust local runtime, generated contracts, extension protocols, SDKs,
harness, local receipts, registry, runx-maintained skills, and packaging. For the
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
own key. Every `runx harness` case, inline or fixture file, signs with the public
fixture key whatever signer is configured, so its receipts stay deterministic.
Publishing and hosted verification require a hosted issuer.

Then inspect the emitted receipt. The full walkthrough is in
[getting-started.md](getting-started.md), and the next step is
[skill-to-graph.md](skill-to-graph.md).
For governed code changes, see [issue-to-pr.md](issue-to-pr.md).

## Payment Boundary Proof

Run `runx harness fixtures/skills/mock-pay` for deterministic local simulation and
`runx harness skills/spend/fixtures/hosted-contract.yaml` for the public hosted
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

```bash
runx --help
runx skill /path/to/runx/oss/fixtures/skills/echo -i message=hello --json
cd /path/to/runx/oss
runx skill ./skills/skill-build design -i objective="build sourcey docs skill" --json
```

Recommended flows:

```bash
runx new docs-demo --objective "Create a bounded documentation decision skill"
runx list skills
runx registry search weather --json
runx skill acme/weather@1.0.0 --registry https://runx.example.test --json
runx registry install acme/weather@1.0.0 --registry https://runx.example.test --json
runx skill issue-to-pr -i repository=acme/app -i issue_number=42 -i objective="Fix the failing build"
runx resume <run-id> answers.json
runx mcp --run <run-id>
runx history
runx history <run-id> --json
runx history <receipt-id> --json
runx mcp ./fixtures/skills/echo
runx skill ./skills/skill-build design -i objective="build github review skill"
runx harness ./examples/hello-graph/harness.yaml
runx config set agent.provider openai
runx config set agent.model gpt-6-sol
printf '%s' "$OPENAI_API_KEY" | runx config set agent.api_key --from-stdin
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
terminal; inspect its receipt with `runx history <receipt-id>`. To execute
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

A host ends a run instead with `{"close": {"disposition": "declined"}}`
(a person stopped it) or `"superseded"` (a new attempt replaced it). The close
comes alone. It seals any run that is not sealed and that no process drives,
whether it waits at an act or stopped between steps, from its record alone:
nothing is bound first, so a package that changed since the run started never
traps it open. The run seals with that disposition and the seal reason
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
the same tools beside its skills, over the runs its own skill calls open. The tools are fixed and take a turn's address: `turn_list` shows
everything the run waits on, `turn_status` reads a turn, `turn_call` makes a
call in it and `turn_complete` ends it. A turn's address is the id of the step
that asks it, as an approval's is its gate. A turn that a call opens in a child
skill is addressed by the child's place in the run, as in `answer/0#report`,
and is worked on the same connection. `turn_status` lists the calls a turn may make, one per entry of
its skill's limits (`tools:`, `skills:`, connector operations, and
`ask_human` when the skill declares `asks: true`), with their input schemas.
`ask_human` takes `{reason}`, a yes or no, or `{questions}`: one to three, each
`{id, prompt, options?: [{label, detail?}], free_text?, required?}` with at most
four options, the recommended first; a question without options takes text.
Each call is admitted against the turn's allowance and the run's authority,
executed, and recorded in the run; the turn's receipt names every call it made.
A host pins its own `call_id` on a call, unique within the turn: a call made
again with that id returns the recorded call rather than running twice, and one
naming another tool or input is refused. A completed call returns each packet
output's project artifact reference under `artifacts`, by output name; where a
tool's or skill's input is of a packet type, a later call in the turn may pass
that reference in place of the packet. Runx reads the artifact the earlier call
recorded, checks its digest and the input's packet schema, gives the input its
content, and names the artifact, labeled with the input, in the call's receipt;
a reference no call of the turn returned is refused. A skill call states its `act` beside
the child's inputs, as an operation call does; it becomes the child run's act.
A write names its `idempotency_key`, which a retry reuses. A call whose outcome
runx cannot confirm stays pending on its own retry request, which says whether
sending it again is safe (`resend`: a read, a journaled file change, or a write
its provider deduplicates); a safe one runs again with the same input when `runx resume`
answers it under `approvals` with `{"retry": true}`, and an unsafe one is never
resent: check what it did, then close the run. A bounded retry (one with
`retry_after_seconds`) is runx's own: the next resume after its delay runs it. A call that needs approval stays pending until `runx resume`
answers it under `approvals`, by the address the paused run lists.
`turn_complete` ends a turn with its output once none of its calls is pending,
and the run continues; an act that cannot be done ends with
`{"failure": {"reason": ...}}` instead, which fails its step with that reason,
and a skill call whose child act failed settles failed with it. A run whose host authority sets `max_fanout` or
`max_child_depth` refuses a skill call that would start more child runs in one
turn, or nest them deeper, than it allows. A host whose own agent works the run
sets `agent_bounds`, which keeps the run's file, git, command and authoring
tools inside its workspace and away from secrets (see Agent bounds, below). A run started with `--managed-agent` is worked by the
managed agent: `runx mcp --run` refuses it, and `--managed-agent` cannot take
over a run its host drives.

Hosted and replay drivers may bind `runx skill` or `runx resume` with
`--package-digest` and `--execution-closure-digest`. Runx recomputes both at the
native execution boundary, persists them in pause checkpoints, and rejects a
resume whose supplied bindings disagree with that checkpoint. These flags bind
execution; they are never skill inputs. The closure digest includes the
immutable Runx release identity, so a pending continuation cannot silently
cross a runtime upgrade. Runx-generated run ids and durable graph state bind
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
are built-in providers, each reading its own key variable when `agent.api_key`
is unset; `openai` speaks OpenAI's Responses API. Runx names no model of its
own: every provider sets `agent.model`, and a managed run refuses without one.
Any other provider sets `agent.dialect` (`chat_completions`,
`anthropic_messages` or `openai_responses`) and `agent.endpoint_url` too, and
reads only `agent.api_key` or `RUNX_AGENT_API_KEY`; a local OpenAI-compatible
server is such a provider, speaking `chat_completions`. A provider's refusal
reaches the receipt as its status and its own error message, never its raw
body or the key.

Prepared context never asks for performative approval: it binds the selected
skill, inputs, execution closure, and drift guards into the receipt. A
consequential action stops once at its owning boundary. An `operation` step
accepts an optional typed `approval` request, and an operation the connector
marks `approval: required` always needs one; the kernel then suspends the call
for one exact host-attested human decision. Otherwise the admitted grant is
sufficient and no performative gate is invented. Use an explicit graph approval only when no
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
`trace` for graph runs, the terminal `closure`, and the `receipt_id` needed for
independent inspection:

```json
{
  "schema": "runx.skill_run.v1",
  "status": "sealed",
  "skill_name": "example",
  "run_id": "run_example_123",
  "receipt_id": "sha256:...",
  "closure": { "disposition": "closed" },
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

`result` is the selected runner's normalized output or the declared contract of
a graph's terminal successful producer. When a guard or an authority refused a
step, `refusal` carries why (`code` and `message`) beside what the steps
produced; a failed run's `result` is its failure. Neither is part of the signed
receipt, whose seal records the typed `disposition` and, when the
disposition alone does not say why, a `reason` from a closed vocabulary. `trace` deliberately carries
references rather than step payloads. Intermediate graph state, child receipts,
and the full signed receipt are written to the local Runx state and receipt
stores; process output identity is committed by the receipt. Inspect proof
through `runx history` and `runx verify` instead of paying to copy it through
every agent response. Paused runs keep their exact `requests` and resume
instructions.

### Structured input documents

Use `-i name=value` for small interactive calls; each value is read by the type
its runner declares, a string as written and any other type as JSON. For
generated or reusable input, pass the complete runner input object through one
workspace-contained JSON document:

```bash
runx skill ./skills/research research --inputs request.json --json
cat request.json | runx skill ./skills/research research --inputs - --json
```

`--inputs` is exclusive with `-i`. A file path is resolved
relative to the invocation workspace and must identify one contained, regular,
UTF-8 file. File and stdin input are capped at 64 MiB, must decode to exactly
one JSON object, and then enter the same selected-runner defaulting and type
validation as inline inputs. Runx does not echo a rejected document in its
diagnostic.

This is a control-document transport, not a high-volume execution profile. Put
large immutable content behind the digest-bound artifact/page boundary and put
durable histories behind data cursors rather than carrying either through graph
context.

The global link points at `packages/cli` in this checkout. Rebuild with
`pnpm build`; do not reinstall.

## Package Topology

Rust owns the trusted local runtime path. The Rust crate graph is the enforced
boundary map:

- `runx-contracts`: domain-neutral Rust contract types plus shared schema
  emission and reconciliation; domain crates contribute the contracts they own.
- `runx-core`: pure state-machine and policy decisions.
- `runx-parser`: pure skill, graph, runner, and tool manifest parsing.
- `runx-receipts`: canonical receipt model, hashing, signatures, and tree
  verification.
- `runx-runtime`: impure local runtime, adapters, exact process invocation and
  supervision, harness replay, journals, registry clients, payment gates, MCP,
  and execution.
- `runx-cli`: native `runx` binary over the runtime.
- `runx-sdk`: blocking CLI-backed SDK over stable contracts.

The TypeScript package graph is the client, protocol-extension, wrapper, and
generated-contract layer:

- `@runxhq/contracts`: generated validators and TypeScript types over the
  Rust-owned schema artifacts.
- `@runxhq/cli`: npm distribution wrapper and client presentation around the
  native CLI.
- `@runxhq/client`: finite native invocation and portable response formatting
  over generated contracts.
- `@runxhq/extension-sdk`: helpers for external process tools.
- `@runxhq/langchain`: optional LangChain tool formatting over the public client.

For the generated package export index, see [docs/api-surface.md](api-surface.md).

`runx-runtime` is the canonical local runtime. It owns local skill, graph,
harness, receipt, history, policy, authority, payment, typed
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
| `runx harness <fixture.yaml>` | Rust harness replay | tests and wrapper views |
| receipts and history | Rust receipt store and journal | display/client views |
| policy, authority, payment, x402 | Rust core/runtime policy | published type mirrors and product UX |
| governed data operations | the store connector's native driver | generated types, helper SDKs |
| non-execution extension protocols | lane-specific Rust/cloud owners | generated types, helper SDKs, provider glue |
| skill authoring and `runx new` | `runx-runtime` authoring service plus `skill-build` | npm launcher only |
| marketplace and docs projection | Rust-owned catalog/contracts | generated docs and client views |

Stateful product work should use the governed data-plane shape in
[docs/governed-data-plane.md](governed-data-plane.md): domain skills own
meaning, while the store connector keeps bounded reads, append-only event
writes and projection reads.

Graphs call the store's operations, `events.append`, `events.read`,
`projection.read` and `heads.list`, usually through the `data-store` skill.
Each call's `data_source_ref` names a logical source inside the workspace's
store. With no store instance confirmed, that is the workspace store, a SQLite
file at `.runx/store.sqlite`; `runx connector use store --parameter url=...`
moves the workspace's streams to another SQLite file, Postgres or Redis.

Large local inputs use `artifact.admit` and `artifact.read`. Admission snapshots
one contained regular file, binds its media type, byte count, and whole-file
digest to an opaque invocation-scoped reference, and never exposes the host
path. The current total admission ceiling is 512 MiB. Reads return exact
offsets, range and whole-file digests, `next_offset`, and `eof` in pages of at
most 4 MiB using base64, character-safe UTF-8, or JSON-array record framing;
the default page is 1 MiB. The 4 MiB limit is a page ceiling, not a total-file
ceiling. `fs.read` and `fs.read_bundle` share the same containment and hashing
owner for bounded text; they are not alternate large-file transports.

`fs.list` and `fs.search` list and search beneath a root with the project's
ignore files honoured as Git honours them; both skip `.git`, `.runx` and any
nested Git worktree unless the request names a path inside it. `git.log`,
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
again immediately before its atomic rename. Within agent bounds a change to a
file that exists must name its digest, so an agent never replaces content it
did not read.

Every change a run makes through these tools is journaled in the run's kernel
state: before a bundle's first rename runx records what each file held and the
directories it will create, and after it, how it ended. A bundle a crash left
begun is rolled back by the run's next driver, or by its close, so a bundle is
applied whole or not at all. Each tool's output names its `journal` entry and
each file's `before_digest`.

**Agent bounds.** A run whose authority sets `agent_bounds`, as a host does for
a run its own agent works in, keeps every native file, git, command and
authoring tool inside its workspace. A root outside it, or inside `.git` or
`.runx`, is refused, and no tool reads, writes, lists, searches or shows a
secret: an environment file other than a template (`.env.example`), a private
key or certificate bundle, `.npmrc`, `.netrc`, `.pgpass`, or anything under
`.git` or `.runx`. A patch or commit leaves such files out and names them. An
operator's run is unchanged and reaches the roots its inputs name, another
repository included. A process the run starts is not confined by this; it runs
with its user's rights.

**The approval floor.** In the same run, at any depth, every step that changes
the workspace or runs a process waits on an approval of gate type `tool`
before it is claimed: a native tool that changes something (`fs.edit`,
`fs.write`, `fs.apply_bundle`, `runx.skill.apply`, `command.execute`), a catalog tool runx
runs as a process, and a graph's process step, whether an agent's turn call
made it or a skill's graph reached it. Its gate carries exactly what executes:
for an edit each file with its unified diff and the digests of what it replaces
and becomes, for a command its argv, directory and digest. A person who
declines it refuses the call. A connector operation is governed by its own
declared approval, whatever implements it, and the floor never holds one.

`standing_approvals` on the run's authority are a person's bounded
confirmation, each exactly one of `{"edits": {"paths": [...]}}` (changes to
files under those workspace-relative paths, by whole components; `""` is the
whole workspace), `{"commands": {"argv_prefix": [...]}}` (a command whose argv
begins with those exact tokens; argv has no shell, so a prefix admits nothing
beyond it) and `{"local": true}` (every step the floor holds). A covered step
runs without pausing, and its record and receipt name the entry that admitted
it. A `runx resume` document may carry `standing_approvals` in the person's
lane: runx records the new set, runs every later step under it, and approves
each open floor approval it covers. Standing approvals never cover an
operation's own declared approval.

**What an approval executes.** Every approval gate carries what executes once it
is approved: an operation its input, a local effect what the floor shows, an ask
its `{reason}`. `summary.payload_digest` is that value's stable digest
(`runx.stable-json.v1`); the value rides as `payload` up to 64 KiB and beyond
that as `payload_ref`, a project artifact holding its canonical JSON. A host
recomputes the digest before it offers Approve.

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

JavaScript modules use a different boundary. Their worker receives no ambient
OS environment, credentials, workspace path, network API, process API, clock,
or randomness; only the validated in-memory module closure, JSON input, exact
declared non-secret environment, and fixed limits cross the worker protocol.

## Capability Packs

Runx is the generic execution engine. Product workflows stay outside the runx
CLI and ship as local skills, runners, and tools in the consuming repo.

The intended extension model is:

- `runx` owns generic runtime, thread, outbox, receipt, and handoff machinery
- service repos own their product workflows as local capability packs
- operators execute those workflows through normal skill invocation
- CLI, API, and GitHub-comment triggers all normalize into the same capability
  execution envelope, while the thread stays the review/control object

From inside a product repository, `runx skill ./skills/<workflow> <runner> -i
name=value` resolves that repository's local capability pack. A workflow is
never a privileged engine command: there is no `runx docs ...` or
`runx outreach ...` path inside the engine.

`issue-to-pr` follows the same boundary. runx owns the generic source-thread to
scafld to PR machinery; service repos own Slack, Sentry, owner assignment, and
publish policy. See [docs/issue-to-pr.md](issue-to-pr.md).

## Standalone Skill Packages

`runx new <name> --objective <outcome>` is a thin client of the canonical
`skill-build` lane. It inspects the target and catalog, requests a closed
architecture decision and change draft, binds both to the inspected package
digest in native code, stages and harnesses the exact candidate, then applies
the same bytes transactionally, by default to `skills/<name>`. It runs exactly
as `runx skill runx/skill-build` would. The command owns no templates and never
adds a placeholder JavaScript module. Nothing needs initializing first: a
repository or the `skills/` directory marks the project.

```bash
runx new docs-demo --objective "Create a bounded documentation decision skill"
```

Without `--managed-agent`, the command stops at `needs_agent`, prints an answers
template and exact `runx resume` command, and leaves the target untouched. Add
`--managed-agent` only when this run should use the configured in-process model
loop. To cold-start without installing Runx first, invoke the same command
through `npx @runxhq/cli`.

Community skills should be authored and published as standalone packages
through this lane. The main `runx` repo is the lane for runx-maintained skills
and runtime code, not the community package catalog.

Prefer declarative graphs composed from native tools and existing skills. When
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
declares at `connectors/<publisher>/<id>/` (its manifest, the tools its
process operations run, and its `README.md` and `mark.svg` when present), and
its metadata binds every declared connector by `ref`, `version`,
`manifest_digest`, `connector_digest` and `runtime_capability`. Every package
carries `runx.package.json`, which marks the installed package's root.

`runx registry package <connector-dir> --json` projects a connector published
on its own: the same files at the package root, the parsed manifest,
its content digest (over every file, which names the version and which the
registry signs), and its manifest, connector and operation digests. Its
publisher is the namespace the nearest `plugin.json` declares.

Connectors are published only to a hosted registry: the runx catalog unless
`--registry <url>` or `RUNX_REGISTRY_URL` names another.

- `runx registry publish <connector-dir> [--version <label>]` sends that
  package; the registry reads it natively, publishes it under the authenticated
  publisher's namespace, and the CLI checks the recorded ref and content digest
  are the package's. A missing label is `sha-` and the first 12 hex digits of
  the content digest.
- `runx registry install <publisher>/<id>[@version] [--digest <content-digest>]`
  adds what is published under that name. For a connector, the version goes to
  the project's connectors directory,
  `connectors/<publisher>/<id>/<version>/`. The signed manifest
  must name that connector (never a skill), version and content digest under a
  trusted key scoped to its publisher, and the staged files must load and
  repackage as exactly those files before they are renamed into place. A
  version directory is written once and never rewritten: installing it again
  is `unchanged`, and other files under the same version are refused.

A project keeps every package it runs in two directories beside its source.
`skills/` holds the skills you write (`skills/<name>/`), every skill added
from a registry by `runx registry install`, by running a registry ref or as a
registry step's child (`skills/<owner>/<name>/<version>/`), and the core skills this
release carries, added as `skills/runx/<name>/<version>/` when first used.
`connectors/` holds your own connectors (`connectors/<id>/`), installed ones
(`connectors/<publisher>/<id>/<version>/`) and the runtime capability
connectors (`store`, `web`, `vault`, `artifact`) a skill uses, added as
`connectors/runx/<id>/<version>/` from the release and checked against it on
every read. Every other connector, runx-maintained ones included, is a
published package: a published skill carries the connectors it binds, and a
local skill names one by its publisher (`runx/github`) once
`runx registry install` has added it. A version directory is written once;
other bytes under it are refused. The runx home keeps keys, configuration and state, not packages.

Use [skill-catalog.md](skill-catalog.md) for the maintained category list,
catalog search flow, and duplicate-check standard before proposing a new
first-party skill.

## Skill And Execution Profile

Executable skills split authored skill content from execution profiles. A
package's `run.yaml` is its execution profile:

```text
skills/issue-to-pr/
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
fields. Keep capability and receipt mappings explicit in the runner that uses
them.

A runner's result comes from its last step unless `result_from` names its
intentional public result producers:

```yaml
connectors:
  moltbook:
    operations: [post.create]
runners:
  publish:
    result_from: [publish]
    steps:
      - id: publish
        operation: moltbook/post.create
        idempotency_key: $input.idempotency_key
        approval:
          reason: Approve publishing this exact post.
          type: moltbook_post
        result_fields: [post]
        input:
          submolt_name: $input.submolt
          title: $input.title
          content: $input.body
```

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
the runner's exact scopes and containment are sufficient by default. Add an explicit `ask` step only when the domain action
itself requires a human decision, such as publishing a release. Runx does not
infer that requirement from the capability's write-shaped effect.

The host protocol represents every unresolved request with the generic
`needs_agent` run state. CLI JSON and human output normalize a request set made
only of approval requests to `needs_approval`; mixed or model-authored request
sets remain `needs_agent`. Both resume the same sealed checkpoint through
`runx resume` and neither opens an interactive prompt.

Payment capabilities also set the typed `amount: {units, unit}` input. It is
part of the plan digest and appears in the operator summary; it is not copied
into the provider payload or inferred from provider-specific JSON.

`result_from` is not a list of graph leaves. It returns each selected successful
step's complete declared contract, including packet envelopes. Approvals and
intermediate evidence remain in the separate operator context and signed
receipts. Multiple names are for mutually exclusive result branches or
deliberately combined, non-overlapping contracts; two successful producers may
not emit the same key. Preparation resolves every result producer through its
exact runner, tool manifest, or nested skill and rejects a missing semantic
output contract before any step executes. Declare `run.outputs`,
`artifacts.wrap_as`, or `artifacts.named_emits`; transport output is never an
implicit graph result. `artifacts.wrap_as` names a step's whole result as one
artifact, with `artifacts.packet` its packet; `artifacts.named_emits` names
fields of the result as artifacts, with `artifacts.packets` theirs.

Public catalog packages must keep examples in standalone fixtures, not inline
manifest harness blocks. The package should contain only the files the skill
uses at execution time: `SKILL.md`, `run.yaml`, deterministic runner files,
schemas, fixtures, and narrowly scoped `context/` or `references/`. Do not add
README/changelog/setup docs, generated state, logs, screenshots, private
provider config, or broad project plans inside a public skill. The public docs
for the package belong in `SKILL.md`; external guides belong under `docs/`.

See [skill-to-graph.md](skill-to-graph.md) for composing skills into a graph and
[publishing.md](publishing.md) for publishing and trust tiers.

Use `work-plan` for bounded change planning, `skill-build` for skill-package
design and improvement, and the named downstream execution lane for mutation.

## Tool Authoring

Reusable Runx mechanics and security boundaries belong in the native Rust tool
catalog. Skills should compose those native tools instead of shipping request,
filesystem, Git, hashing, packet, approval, credential, or receipt wrappers.
Use `runx tool inspect <name> --json` to inspect the contract that both graphs
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
`defineTool()` from `@runxhq/extension-sdk` may transport the already
materialized input object and structured failures over the local process
protocol; it does not declare or regenerate the manifest.

```bash
runx tool build --all --json
runx harness ./skills/<skill-that-calls-the-tool> --json
```

A tool is proved through the skill that calls it: that skill's harness
fixtures run the tool on the path a user runs.

When a `run.mjs` entrypoint is needed, it is a thin process shim and owns no
contract metadata. `runx tool build` is read-only: it validates the source
manifest and reports hashes derived from the manifest and its declared source
files without rewriting either. Do not create one of these packages merely to
wrap a capability already owned by the native catalog. The declared entrypoint
must run directly on the supported Node runtime; do not probe for build output
or import uncompiled TypeScript.

## Runx-Maintained Packages

The runx-maintained catalog is explicit about why each package is public:

- canonical governed skills: `charge`, `dispute-respond`,
  `skill-build`, `skill-review`, `least-privilege`, `skill-adopt`,
  `policy-draft`, `receipt-audit`, `refund`, `ops-desk`, `send-as`, `spend`,
  `weather-forecast`
- branded provider skills: `nws-weather-forecast`, `stripe-pay`, `x402-pay`
- context skills: `brand-voice`, `taste-profile`

Other bundled packages stay in the same `SKILL.md` + `run.yaml` shape, but are
internal by default. Internal packages must declare why they remain bundled:
`graph-stage`, `runtime-path`, `harness-fixture`, or `context`. Owned graph
stages live below their public skill at `skills/<skill>/graph/<stage>/run.yaml`,
not as root catalog packages.

For first-party skill proposal work, the core builder bar is explicit:
proposal packets should name the real pain being solved, explain fit against
the current runx catalog, surface maintainer decisions cleanly, and avoid
builder residue or placeholder targets.

See `docs/operator-console.md` for the manager-dashboard model that lets agents
operate projects, workspaces, products, or accounts through the same governed
lanes as the UI.

Each ships as a portable `SKILL.md` plus a colocated execution profile at
`skills/<skill>/run.yaml` when it exposes deterministic runners or inline harness
coverage. Upstream skills that runx does not own keep their execution profiles
under `bindings/<owner>/<skill>/run.yaml` with adjacent `binding.json`
governance metadata. Bare skill names resolve to the workspace's own skills or
to the core skills this release carries, which are added under
`skills/runx/<name>/<version>/` when first used. Every other package runs by
its `owner/name[@version]` ref, optionally with `--registry` and `--digest`;
only a trusted, signed registry package is added to the project, at
`skills/<owner>/<name>/<version>/`.

Agent graphs can also demand-load skills as context instead of executing them.
Reference reusable judgement, operating procedure, or capability skills from an
`agent` step with `context_skills`:

```yaml
context_skills:
  - ../taste-profile
  - registry:runx/taste-profile@1.0.0
```

The runtime injects each referenced `SKILL.md` as a generic
`runx.skill.context` artifact in the agent invocation `current_context`. Local
path refs resolve relative to the graph; `runx skill` fetches each registry ref
the project does not hold before the run, into the project's skills, and the
runtime reads it from there, never from a registry during execution. Explicit
local/file registry sources are treated as user-owned
local material: they are digest-checked, but they do not require hosted registry
signatures. Context skills are bounded, digest-labeled, and presented to managed
agents as untrusted advisory data.

Graph steps can execute local-registry skills too:

```yaml
steps:
  - id: build_docs
    skill: registry:acme/docs-build
    runner: build
```

This uses the same explicit local-registry rule: set `RUNX_REGISTRY_DIR`, sync or
publish the skill into that registry first. A registry child is added to the
project's `skills/<owner>/<name>/<version>/` like any other added skill.

Any runnable skill package can also be exposed locally as an MCP tool with:

```bash
runx mcp ./skills/issue-triage
```

That MCP surface is a thin facade over the normal runx kernel path, so receipts,
policy, approvals, and resolution requests still behave the same way.

## Receipts

Local receipts are append-only JSON files under `.runx/receipts` unless `RUNX_RECEIPT_DIR` is set. `runx history <receipt-id>` verifies a receipt's signature and surfaces `verified`, `unverified`, or `invalid` status. Once a skill package and runner resolve, a blocked preparation also seals a `blocked` refusal receipt; the JSON failure returns its `receipt_id` and prepared-context digest so rejected input or context admission remains inspectable without echoing the rejected value.

Graph receipt lineage is an immutable one-way tree. The parent commits each
child receipt ID and exact signed-body digest; a child never names its parent,
so receipt identity stays content-addressed. Every verifier applies one rule to
a tree: each receipt is committed once by its parent, and a tree holds at most
1024 receipts and 64 levels below its root. A receipt two parents commit is
refused, and two different signed bodies under the same receipt ID are
ambiguous and fail verification.

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

## Harness

Run a whole skill package to execute its inline `run.yaml` cases and every
conventional `fixtures/*.yaml` case through the native runtime:

```bash
runx harness ./skills/cve-audit --json
```

Pass a fixture YAML path instead when intentionally replaying one standalone
case:

```bash
runx harness ./examples/hello-graph/harness.yaml --json
```

Fixtures can assert the terminal status, receipt identity and lineage, exact or
subset output, ordered graph steps, and exact or subset output for named graph
steps. Prefer semantic subsets over complete snapshots:

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

A receipt-dependent fixture may seed a bounded set of existing receipts from
its own package:

```yaml
setup:
  receipts:
    - fixtures/receipt-store/sha256-example.json
```

Each path must be a normalized package-relative `.json` path. Runx parses and
verifies every seeded receipt under the fixture's configured signature policy
before placing it in that case's isolated store. Seed data is evidence for the
case, not proof of a provider action; the fixture must still assert only what
the verified receipts and runner output establish.

A case's `caller` stands in for the host and the world. It is one shape in
fixture files and inline runner cases:

- `act`: the act the caller states for the run, which its steps read as
  `$act`;
- `answers`: agent act outputs, by the id of the step that asks;
- `turns`: agent turns the caller works call by call, by the id of the step
  that asks. An act is answered or worked, not both;
- `approvals`: approval decisions, `true` or `false`, by the address of the
  request each decides: an operation step's id or an ask's gate at the root,
  and `<path>#<id>` in a child run, `<path>` being its place under the root,
  such as `send#post`;
- `http_exchanges`, `mcp_calls` and `provider_responses`: what the run's
  requests reach, described below.

An answer or a turn script keyed by an id answers that request in every run of
the case that asks it. A key may instead be an address, such as
`answer/0#answer`, which answers only the request at that place
in the run and wins over an id. An approval decides only the request it
names.

A worked turn makes each call through the turn, as an attached agent would,
then ends with its `output`. A call names a tool the turn offers, by the name
its status lists, and may say what it must return; a call that returns
something else fails the case and is named in its error:

```yaml
caller:
  turns:
    answer:
      calls:
        - tool: data_digest
          input: { value: hello, encoding: utf8_text }
          expect: { subset: { outcome: { status: completed } } }
      output: { answer: digested }
```

Supplied `caller.answers` are semantic oracles, not inert model stubs. Native
replay fails unless every supplied answer is observable in the root or step
output. For stateful work, use one graph fixture to prove the transition and
durable readback; do not depend on fixture filename order.

Native HTTP fixtures can bind exact response bytes without turning a semantic
test into a live-network smoke test:

```yaml
caller:
  http_exchanges:
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

`http_exchanges` is matched on the exact method, final URL, and explicit body
identity: `body: none` means no body, while `body: {json: ...}` means one
structural JSON value, including literal null. An exchange URL is
canonicalized like the native final URL and cannot contain credentials or a
fragment. A request the case declares more than once is answered by those
exchanges in the order they are declared, and the last answers every later
repeat, so one case can read a resource, change it and read it back. A case
declares only the exchanges its run makes: one no request asked for fails the
case, so a declared write is a write the run sent. Any request no exchange
matches is refused without reaching the network; a harness run has no live
fallback. Connector operations still own their real admission, request
rendering, authentication, decoding, custody and readbacks; only transport
response bytes come from the harness. `mcp_calls` stands in for a connector's
MCP server the same way, matched on tool, arguments and session headers, and
`setup.credentials` stores the one credential of a listed connector whose
variable the case sets. `provider_responses` stands in for runx Connect: it
holds the case's grant, renders a hosted operation's declared requests as
Connect does, with its `invocation_id` as the paid invocation, and sends them
to the case's `http_exchanges`. Without an `invocation_id`, a request that
sends the paid invocation fails closed. A case that stops on a request it did not answer names
that request in its status mismatch. No caller fixture is available to
skill inputs, environment configuration, or ordinary live runs.

Package replay uses a disposable project-owned workspace below `.runx/harness`
and a separate receipt store for every case, then cleans that scratch state.
Cases therefore cannot satisfy each other through filename order or ambient
receipt history. Receipts produced by each case are verified and copied to the
durable `.runx/receipts` store, or to the directory selected by
`--receipt-dir`. An explicit `RUNX_CWD` identifies the owning project but does
not disable harness isolation.

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
  supervision, receipts, policy, authority, payment, harness, and built-in
  adapters.
- No TypeScript package is a fallback executor for trusted local behavior.
- Tools own their side effects behind language-neutral manifests and process
  contracts; non-execution extension lanes have their own protocol contracts.
- External extension authors must not need Rust, a `runx-core` or
  `runx-runtime` dependency, or a core repository fork.
- CLI, SDK, IDE plugin, host adapter, and MCP entrypoints delegate to runner
  contracts or external protocols instead of duplicating the engine.
