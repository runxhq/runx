# runx reference

Architecture and contributor reference for the runx OSS repo: the CLI surface,
the trusted Rust local runtime, generated contracts, extension protocols, SDKs,
harness, local receipts, registry, official skills, and packaging. For the
overview and quickstart, start at the [README](../README.md).

The npm CLI package is `@runxhq/cli` and exposes the `runx` binary.

## Your First Skill In 5 Minutes

Start with the checked-in hello-world skill:

```bash
cargo build --manifest-path crates/Cargo.toml -p runx-cli
export RUNX_RECEIPT_DIR="$(mktemp -d)"
crates/target/debug/runx skill examples/hello-world \
  --message "hello from docs" \
  --json
```

On macOS 26, complete the
[Developer Tools permission prerequisite](../CONTRIBUTING.md#macos-developer-tools-permission)
if the initial build stalls.

When no signer is configured, local `runx skill` and inline `runx harness` runs
sign with the runx home's own key; harness fixture replay signs with the public
fixture key so its receipts stay deterministic. Publishing and hosted
verification require a hosted issuer.

Then inspect the emitted receipt. The full walkthrough is in
[getting-started.md](getting-started.md), and the next step is
[skill-to-graph.md](skill-to-graph.md).
For governed code changes, see [issue-to-pr.md](issue-to-pr.md).

## Payment Boundary Proof

Run `runx harness skills/mock-pay` for deterministic local simulation and
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
runx skill /path/to/runx/oss/fixtures/skills/echo --message hello --json
cd /path/to/runx/oss
runx skill ./skills/skill-build design --objective "build sourcey docs skill" --json
```

Recommended flows:

```bash
runx init
runx init -g --prefetch official
runx new docs-demo --objective "Create a bounded documentation decision skill"
runx list skills
runx registry search sourcey --json
runx skill sourcey/sourcey@1.0.0 --registry https://runx.example.test --project . --json
runx add sourcey/sourcey@1.0.0 --registry https://runx.example.test --to ./skills --json
runx skill issue-to-pr --fixture /path/to/repo --task-id task-123
runx resume <run-id> answers.json
runx turn <run-id>
runx history
runx history <receipt-id> --detail --json
runx mcp serve ./fixtures/skills/echo
runx skill ./skills/skill-build design --objective "build github review skill"
runx harness ./fixtures/harness/echo-skill.yaml
runx config set agent.provider openai
printf '%s' "$OPENAI_API_KEY" | runx config set agent.api_key --from-stdin
```

`runx resume` continues a paused run from its run record. A sealed run is
terminal; inspect its receipt with `runx history <receipt-id> --detail`. To execute
the work again, invoke the original `runx skill` command and retain both
receipts if you need to compare the outcomes. The current CLI has no standalone
`replay` or run `diff` command.

The resume file keeps agent work and human authorization distinct:

```json
{
  "answers": {
    "agent.example.output": {
      "result": {}
    }
  },
  "approvals": {
    "deliver": {
      "approved": true,
      "reason": "I authorize this exact provider action."
    }
  }
}
```

Only entries under `approvals` carry host-attested human approval provenance.
The local CLI cannot authenticate a person from JSON: by placing a decision
there, the host asserts that a human approved the exact pending gate. An agent
must never author or promote that entry; it returns the pending approval to its
human or to an integrated host that authenticates the decision. Never place a
consequential decision under `answers`; that section records agent or
caller-supplied work and cannot resolve an approval gate. Runx rejects an agent
response presented to an approval request. An approval names a request the run
is paused on, by the address the paused run lists; one naming any other
request is refused, so no approval reaches a request a person has not seen,
and a resumed run checks that each request it re-asks is the one it paused on.
The pending run's `answers_template` contains the exact request ids to resolve.

`runx turn <run-id>` serves the agent turns a paused run waits on as MCP tools
over stdio. The tools are fixed and take a turn's address: `turn_list` shows
everything the run waits on, `turn_status` reads a turn, `turn_call` makes a
call in it and `turn_complete` ends it. A turn's address is its act's request
id. A turn that a call opens in a child skill is addressed by the child's place
in the run, as in `answer/0#agent.report.output`, and is worked on the same
connection. `turn_status` lists the calls a turn may make, one per entry of
its skill's limits (`tools:`, `skills:`, connector operations, and
`ask_human` when the skill declares `asks: true`), with their input schemas.
Each call is admitted against the turn's allowance and the run's authority,
executed, and recorded in the run; the turn's receipt names every call it made.
A write names its `idempotency_key`, which a retry reuses. A call whose outcome
runx cannot confirm stays pending on its own retry request until `runx resume`
answers that request with `{"retry": true}`, which runs the call again with the
same input. A call that needs approval stays pending until `runx resume`
answers it under `approvals`, by the address the paused run lists.
`turn_complete` ends a turn with its output once none of its calls is pending,
and the run continues. A run whose delegation sets `max_fanout` or
`max_child_depth` refuses a skill call that would start more child runs in one
turn, or nest them deeper, than it allows. A run started with `--managed-agent` is worked by the
managed agent: `runx turn` refuses it, and `--managed-agent` cannot take over a
run its host drives.

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
typed at the invocation boundary; deterministic-module limits and any limit hit
are copied into the receipt's unsigned metadata.

Credential redaction and untrusted-data admission are also distinct. Exact
values delivered by Runx enter the run-scoped taint set and are scrubbed at
every output boundary. External configuration and provider output can contain
raw material Runx never delivered, so those narrow boundaries additionally
reject exact secret-field names before the data can enter receipts. That check
is not a general text heuristic and never removes URLs or unrelated operator
diagnostics.

With `agent.provider` and `agent.api_key` configured, the CLI can resolve
managed agent work directly. Deterministic tools, approvals, and required human
inputs keep their existing local behavior. `anthropic`, `openai` and `gemini`
are built-in providers, each reading its own key variable when `agent.api_key`
is unset and using its default model (`claude-opus-5-5`, `gpt-6-sol`,
`gemini-3-pro-preview`) when `agent.model` is unset. Any other provider, and
any configured `agent.endpoint_url`, also sets `agent.model`; another provider
sets `agent.dialect` (`chat_completions` or `anthropic_messages`) and
`agent.endpoint_url` too, and reads only `agent.api_key` or
`RUNX_AGENT_API_KEY`.

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
account with `runx credential set <connector> --credential <name>
--from-stdin` or `runx connector use`. See
[Credentials](credentials.md) for the storage contract.

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
receipt, whose seal records the typed `disposition` and `reason_code`. `trace` deliberately carries
references rather than step payloads. Intermediate graph state, child receipts,
and the full signed receipt are written to the local Runx state and receipt
stores; process output identity is committed by the receipt. Inspect proof
through `runx history` and `runx verify` instead of paying to copy it through
every agent response. Paused runs keep their exact `requests` and resume
instructions.

### Structured input documents

Use per-key `--input` and `--input-json` flags for small interactive calls. For
generated or reusable input, pass the complete runner input object through one
workspace-contained JSON document:

```bash
runx skill ./skills/research research --inputs request.json --json
cat request.json | runx skill ./skills/research research --inputs - --json
```

`--inputs` is exclusive with every per-key input form. A file path is resolved
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

Sourcey is the reference shape for this model: from inside the Sourcey repo,
`runx skill ./skills/outreach status --issue ...` resolves the local
`skills/outreach` capability pack. `outreach` is not a privileged engine
command, and there is no privileged `runx docs ...` path inside the engine.

`issue-to-pr` follows the same boundary. runx owns the generic source-thread to
scafld to PR machinery; service repos own Slack, Sentry, owner assignment, and
publish policy. See [docs/issue-to-pr.md](issue-to-pr.md).

## Standalone Skill Packages

`runx new <name> --objective <outcome>` is a thin client of the canonical Skill
Lab build lane. It inspects the target and catalog, requests a closed
architecture decision and change draft, binds both to the inspected package
digest in native code, stages and harnesses the exact candidate, then applies
the same bytes transactionally. The command owns no templates and never adds a
placeholder JavaScript module:

```bash
runx new docs-demo --objective "Create a bounded documentation decision skill"
```

Without `--managed-agent`, the command stops at `needs_agent`, prints an answers
template and exact `runx resume` command, and leaves the target untouched. Add
`--managed-agent` only when this run should use the configured in-process model
loop. To cold-start without installing Runx first, invoke the same command
through `npx @runxhq/cli`.

Community skills should be authored and published as standalone packages
through this lane. The main `runx` repo is the first-party lane for official
skills and runtime code, not the community package catalog.

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
parser-owned publish artifact without writing a registry row. Hosted and
third-party publishers should consume that output instead of implementing
their own package-file discovery. A skill package carries each connector it
declares at `connectors/<publisher>/<id>/` (its manifest, the tools its
process operations run, and its `README.md` and `mark.svg` when present), and
its metadata binds every declared connector by `ref`, `version`,
`manifest_digest`, `connector_digest` and `runtime_capability`. Every package
carries `runx.package.json`, which marks the installed package's root.

`runx registry connector package <connector-dir> --json` projects a connector
published on its own: the same files at the package root, the parsed manifest,
and its manifest, connector and operation digests. Its publisher is the
namespace the nearest `plugin.json` declares.

Use [skill-catalog.md](skill-catalog.md) for the maintained category list,
catalog search flow, and duplicate-check standard before proposing a new
first-party skill.

## Skill And X Model

Executable skills split authored skill content from execution profiles. `run.yaml`
is the runx execution profile file; the short name is public compatibility for
existing skill packages, but docs and code should describe it as the execution
profile:

```text
skills/sourcey/
  SKILL.md
  run.yaml
```

Direct execution accepts the package directory or `SKILL.md` inside it. Flat
`foo.md` skill files are no longer a supported execution surface.

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

See `../docs/skill-profile-model.md` for resolution rules, publication modes, trust tiers, MCP export, and composite skill behavior.

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
runx dev --lane deterministic --json
runx dev --lane repo-integration --json
```

When a `run.mjs` entrypoint is needed, it is a thin process shim and owns no
contract metadata. `runx tool build` is read-only: it validates the source
manifest and reports hashes derived from the manifest and its declared source
files without rewriting either. Do not create one of these packages merely to
wrap a capability already owned by the native catalog. The declared entrypoint
must run directly on the supported Node runtime; do not probe for build output
or import uncompiled TypeScript.

## Official Packages

The official catalog is explicit about why each package is public:

- canonical governed skills: `charge`, `dispute-respond`,
  `skill-build`, `skill-review`, `least-privilege`, `skill-adopt`,
  `policy-draft`, `receipt-audit`, `refund`, `ops-desk`, `send-as`, `spend`,
  `weather-forecast`
- branded provider skills: `nitrosend`, `nws-weather-forecast`, `stripe-pay`,
  `x402-pay`
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
governance metadata. Bare skill names resolve only to local workspace skills or
locked first-party official shorthand. Third-party registry execution uses the
explicit `owner/name@version` form, optionally with `--registry` and
`--digest`, and only trusted signed registry packages are materialized into the
runnable cache. Official skills are registry-backed and cached locally on first
acquisition. The npm CLI package no longer needs to ship the official runtime
skill bodies for normal execution.

Agent graphs can also demand-load skills as context instead of executing them.
Put reusable judgement, operating procedure, or capability skills in the local
registry, then reference them from an `agent` step with `context_skills`:

```yaml
context_skills:
  - ../taste-profile
  - registry:runx/taste-profile@1.0.0
```

The runtime injects each referenced `SKILL.md` as a generic
`runx.skill.context` artifact in the agent invocation `current_context`. Local
path refs resolve relative to the graph; registry refs require
`RUNX_REGISTRY_DIR` and are read from the local registry, not fetched remotely at
execution time. Explicit local/file registry sources are treated as user-owned
local material: they are digest-checked, but they do not require hosted registry
signatures. Context skills are bounded, digest-labeled, and presented to managed
agents as untrusted advisory data.

Graph steps can execute local-registry skills too:

```yaml
steps:
  - id: build_docs
    skill: registry:runx/sourcey
    runner: sourcey
```

This uses the same explicit local-registry rule: set `RUNX_REGISTRY_DIR`, sync or
publish the skill into that registry first, and treat `.runx/registry-step-skills`
as generated runtime cache rather than source.

Any runnable skill package can also be exposed locally as an MCP tool with:

```bash
runx mcp serve ./skills/sourcey
```

That MCP surface is a thin facade over the normal runx kernel path, so receipts,
policy, approvals, and resolution requests still behave the same way.

## Receipts

Local receipts are append-only JSON files under `.runx/receipts` unless `RUNX_RECEIPT_DIR` is set. `runx history <receipt-id> --detail` verifies a receipt's signature and surfaces `verified`, `unverified`, or `invalid` status. Once a skill package and runner resolve, a blocked preparation also seals a `blocked` refusal receipt; the JSON failure returns its `receipt_id` and prepared-context digest so rejected input or context admission remains inspectable without echoing the rejected value.

Graph receipt lineage is an immutable one-way DAG. The parent commits each
child receipt ID and exact signed-body digest; a child never names its parent.
This keeps receipt identity content-addressed and
allows the same child proof to participate in more than one graph without a
store collision. Runtime tree resolution collapses only identical repeated
child receipts into one DAG node; two different signed bodies under the same
receipt ID remain ambiguous and fail verification.

Publish a local receipt to the hosted notary with:

```bash
runx publish ./.runx/receipts/<receipt-id>.json
```

`runx publish` posts the full sealed receipt to `POST /v1/receipts/notarize`
with `publish: true`, then prints the public `/r` link and content hash returned
by the notary. Configure the hosted API with `RUNX_PUBLIC_API_BASE_URL` (default
`https://api.runx.ai`) and authenticate with `RUNX_PUBLIC_API_TOKEN` (or run
`runx login --for publish`). The stored publish credential is separate
from the default operator credential used by Connect and provider effects. Runx
requires HTTPS for non-loopback API origins so the bearer token cannot be sent
to a public plaintext endpoint.

For local hosted dogfood only, point at a loopback API and opt into the private
network escape explicitly:

```bash
RUNX_PUBLIC_API_TOKEN=dev-token runx publish ./.runx/receipts/<receipt-id>.json \
  --api-base-url http://127.0.0.1:47882 \
  --allow-local-api
```

## Workspace Policy

Projects can opt into stricter local process-tool admission with
`.runx/config.json`:

```json
{
  "policy": {
    "strict_process_tool_inline_code": true
  }
}
```

When enabled, local execution rejects known inline interpreter and shell eval
forms such as `node -e`, `python -c`, and `bash -lc`. Move the program into a
checked-in script file and invoke that file instead.

## Harness

Run a whole skill package to execute its inline `run.yaml` cases and every
conventional `fixtures/*.yaml` case through the native runtime:

```bash
runx harness ./skills/business-ops --json
```

Pass a fixture YAML path instead when intentionally replaying one standalone
case:

```bash
runx harness ./fixtures/harness/echo-skill.yaml --json
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
fixture files, inline runner cases and dev fixtures:

- `act`: the act the caller states for the run, which its steps read as
  `$act`;
- `answers`: agent act outputs, by request id;
- `turns`: agent turns the caller works call by call, by request id. An act
  is answered or worked, not both;
- `approvals`: approval decisions, `true` or `false`, by the address of the
  request each decides: an operation step's id or an ask's gate at the root,
  and `<path>#<id>` in a child run, `<path>` being its place under the root,
  such as `send#post`;

An answer or a turn script keyed by an id answers that request in every run of
the case that asks it. A key may instead be an address, such as
`answer/0#agent.answer.output`, which answers only the request at that place
in the run and wins over an id. An approval decides only the request it
names.
- `http_exchanges`, `mcp_calls` and `provider_responses`: what the run's
  requests reach, described below.

A worked turn makes each call through the turn, as an attached agent would,
then ends with its `output`. A call names a tool the turn offers, by the name
its status lists, and may say what it must return; a call that returns
something else fails the case and is named in its error:

```yaml
caller:
  turns:
    agent.answer.output:
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
variable the case sets. A case that stops on a request it did not answer names
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
pnpm build
pnpm test tests/cli-package.test.ts
cd packages/cli
npm pack --dry-run --json
```

The package must include `dist/index.js` and `dist/index.d.ts`, and `dist/index.js` must be executable.

## Boundary Rules

- `oss/` (this repository) must not import from `cloud/` (the private companion workspace, not part of this checkout).
- State-machine and policy packages remain pure.
- Rust owns trusted local runtime/execution, including exact process
  supervision, receipts, policy, authority, payment, harness, and built-in
  adapters.
- TypeScript runtime-local and adapters packages must not be fallback
  executors for trusted local behavior.
- Tools own their side effects behind language-neutral manifests and process
  contracts; non-execution extension lanes have their own protocol contracts.
- External extension authors must not need Rust, a `runx-core` or
  `runx-runtime` dependency, or a core repository fork.
- CLI, SDK, IDE plugin, host adapter, and MCP entrypoints delegate to runner
  contracts or external protocols instead of duplicating the engine.
