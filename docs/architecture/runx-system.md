# Runx system architecture

Status: normative. This document defines the ownership and execution boundaries
for Runx OSS. Historical migration notes may explain how the repository arrived
here, but they do not override this contract.

## Product model

Runx is an operator runtime. A skill is the durable operating manual and bounded
program that teaches a human and an acting agent how to perform one operation.
The runtime supplies reusable execution capabilities, authority, deterministic
worker isolation, process supervision, receipts, replay, and provider
boundaries. Product-owned skills retain domain
judgment. Runx Cloud is a hosted control plane and provider-execution service;
it is not the owner of local operator workflows.

The dependency direction is one way:

    portable contracts
          |
          v
    parser + pure policy/authority + receipts
          |
          v
    runtime orchestration + capabilities + supervised adapters
          |
          v
    CLI and generated language bindings
          |
          v
    skills and product-owned operator packages

    hosted Connect grant -> bounded provider driver -> provider

There is one production owner for every contract and behavior. Catalog views,
CLI commands, SDKs, generated schemas, exported agent shims, and documentation
consume those owners; none is a parallel implementation.

## Repository ownership

- `runx-contracts` owns domain-neutral portable Rust wire types and schema
  mechanics. Payment skills consume the same generic authority, provider
  operation, approval, and receipt contracts. `runx-x402` is inert protocol
  presentation; it owns no provider execution or payment orchestration.
- `runx-core` owns pure policy, authority, and state-transition algebra.
- `runx-parser` owns all pure package parsing and validation. It returns one
  aggregate validated package representation; consumers do not reparse package
  Markdown or YAML.
- `runx-receipts` owns canonical receipt/proof encoding and verification.
- `runx-runtime` owns filesystem loading, execution, typed native capability
  registration, exact process invocation and supervision, adapter lifecycles,
  effects, typed execution-boundary evidence, and receipt emission.
- `runx-cli` owns argument parsing and presentation only. It calls runtime
  services and does not implement a second executor, parser, credential loader,
  authoring engine, or provider client.
- `@runxhq/contracts` is a generated or mechanically checked binding over the
  portable contract owner.
- `@runxhq/extension-sdk` owns the narrow process tool helpers required by
  genuine external extensions. It does not recreate Runx.
- `skills/*` and product-local skill trees own operator knowledge, declarative
  composition, harnesses, and irreducible deterministic domain computation.
- `runx/cloud` owns hosted control-plane state, credential custody, grant
  resolution, and bounded provider API execution only.

Forbidden dependency arrows include parser-to-runtime, core-to-runtime,
contracts-to-runtime, receipts-to-runtime, CLI-to-package-source parsing,
skill modules-to-filesystem/network/process APIs, OSS operator logic-to-a Cloud
checkout, and Cloud-to-local operator workflow ownership.

## Skill knowledge contract

`SKILL.md` is a substantive operating manual, not a terse tool index. It must
give a human and an acting agent the context that changes how they operate:

- what the operation achieves and when this is the correct lane;
- the domain model and distinctions an unfamiliar operator would otherwise
  miss;
- the evidence required before judgment or mutation;
- the safe workflow and where deterministic work ends and judgment begins;
- what authority and approval mean in operator language;
- failure, stop, escalation, retry, and recovery conditions;
- how to interpret outputs, receipts, readback, and unresolved state; and
- when and why to route to a declared adjacent skill.

`run.yaml` owns runner composition, typed inputs and outputs, effects, scopes,
approvals, harness cases, and skill references. It must not repeat the manual.
Package JavaScript exists only for deterministic domain computation the graph
and native capability plane cannot express cleanly. It never exists merely to
perform HTTP, filesystem, subprocess, credential, packet, or receipt mechanics
that Runx already owns.

The current skill's complete manual is digest-bound into the acting context and
resume envelope. Declared `context_skills` contribute bounded catalog summaries
until invoked; the invoked skill then supplies its own complete manual. A chain
must neither hide the target instructions nor preload and duplicate every
manual.

## Execution lanes

Each target has one meaning and one authority boundary:

- **Graph:** deterministic composition, dependencies, branches, guards, and
  recovery. A graph author cannot choose an effect
  owner or gain provider/process authority by naming inputs.
- **Agent step:** bounded judgment under the current manual and an explicit
  allowed-tool set. It yields `needs_agent` by default. In-process managed-agent
  execution requires fresh per-run consent and a visible round budget;
  configured credentials never imply consent.
- **Native capability:** trusted, product-neutral Rust behavior registered from
  one typed definition that owns schema, defaults, dispatch, authority, effect,
  and catalog metadata.
- **Worker tool (`tool: { module: ... }`):** an isolated
  `(JSON, { environment }) -> JSON` computation. It has no ambient filesystem,
  network, process environment, credential, host clock, or host randomness
  authority; only exact manifest-declared non-secret values cross its typed
  worker protocol.
- **Process tool (`tool: { command: ... }`):** intentional trusted host code
  with explicit command, arguments,
  declared environment, timeout, and output policy. Runx supervises the
  process and records the boundary but does not claim filesystem, network, or
  syscall confinement. Bundled tool manifests and their local
  source closure are parser-owned skill-package truth, not a second runtime or
  registry scan.
- **Connector operation:** a supervised HTTP, MCP, or Connect request under
  typed authority and effect contracts. Hosted credentials remain
  opaque.

Harness fixtures are test data interpreted by the runtime harness. They are not
an execution source and cannot become selectable production behavior.

Static step inputs and graph context are materialized once into the invocation
map for every target kind. A static/context name collision is a validation
error. Missing or type-invalid context fails at the producing edge. No target
kind gets a private context projection path.

CLI and MCP calls share `execution::orchestrator::LocalOrchestrator` and
`execution::skill_front`. Preparation admits an immutable package and source
closure once for that invocation; a continuation revalidates its binding.
Typed run disposition controls success, refusal, approval, and agent handoff.
An arbitrary JSON `status` field is output data, never execution authority.
Continuation state binds the original input, package identity, and exact
resolution answers; transport fronts do not reconstruct a second workflow.

## Deterministic module boundary

Deterministic JavaScript runs in a dedicated, versioned `runx-js-worker`
process behind a length-delimited protocol. The runtime sends only a validated
in-memory module bundle, entrypoint, JSON input, and fixed limits. It never sends
a workspace path, skill path, environment map, credential, provider grant, or
ambient input source.

The CLI and worker are one protocol-coupled runtime distribution. Release and
operator build paths build them together, and the runtime rejects a mismatched
worker version before decoding or executing an invocation. It never guesses
across worker response shapes.

There is no Node or shell fallback. Each invocation receives a fresh
engine context. Imports resolve only through normalized relative `.js`/`.mjs`
paths in the validated bundle. Bare specifiers, `node:` modules, URLs, absolute
paths, traversal, symlinks, native modules, and imports outside the bundle are
rejected.

The worker exposes ECMAScript plus one frozen Runx helper:
`Runx.parseUrl(value)`. It performs deterministic absolute-URL parsing and
returns `href`, `origin`, `protocol`, and `hostname`. Browser and Node globals
are not implied; adding another helper requires a reusable domain-independent
need and one runtime-owned implementation.

The fixed global surface is installed with native engine builders. Runx does
not parse a bootstrap script for every invocation; package source is the only
JavaScript parsed on the execution hot path.

Runtime-owned ceilings are 4 MiB source, 4 MiB input, 4 MiB output, 64 MiB
JavaScript heap, 4 MiB JavaScript stack, 30 seconds wall time, and 4,096 queued
jobs. Wall time defaults to two seconds and a runner may select 1 through 30
seconds; other package limits may narrow but never widen the runtime ceiling.
The worker starts with a cleared environment, a non-workspace current
directory, no inherited handles, process memory supervision, and no host APIs.
A timeout, memory fault, protocol error, crash, or polluted stdout fails the
invocation and discards the worker.

The same hostile-module contract is required on `darwin-arm64`, `darwin-x64`,
`linux-arm64`, `linux-x64`, and `win32-x64`. A supported release target cannot
silently lose official JavaScript skills.

## Native capability boundary

A native capability has one typed definition containing its stable id, input,
output, defaults, summary, owner, effect class, authority rule, effect-owned
approval mode, and executor. Schema generation, deserialization, inspection, search, managed
agent exposure, and dispatch all originate from that definition. Parallel
string maps or effect registries are not contract owners.

Behavior is promoted to native only when it enforces a runtime/security
invariant or has at least two independent skill or SDK consumers. Package-only
validation and domain-specific transforms stay in the owning skill. Reusable
HTTP, schema, file, data, receipt, and effect mechanics are native candidates.

Caller paths are always interpreted under an invocation-owned workspace or
runtime-issued opaque handle. Absolute roots, fixture roots, traversal, and
symlink escapes are rejected. Generic command execution is credential-free and
runs under exact process supervision. Authenticated HTTP destinations are
derived from the resolved grant; caller input can only narrow that set.

Split code where responsibilities, authority, or independently changing
contracts separate. Keep ordered admission, invocation, and sealing visible in
the concrete runner. File length alone does not justify a forwarding module,
a new trait, or an extra hot-path dispatch.

## Effect and finality boundary

The registered native capability owns the effect classification, admission,
approval policy, execution boundary, and finality preparation. Graph, skill,
runner, and tool manifests do not redeclare `mutation`, `mutating`, or an
`effect_family`; those parallel claims inevitably drift from the code that can
actually perform the effect. Catalog approval metadata is an operator-facing
promise, not an enforcement source.

A scoped grant answers **may this principal call this provider operation?** It
does not pretend a human approved the exact act. Most bounded provider
mutations need only admitted grant authority. When the owning skill decides an
act is consequential, it supplies an optional typed `approval` request to
`provider.mutate`. The provider effect hashes that request into the exact plan,
asks the host for one human decision, and refuses agent-authored approval.
There is no generic mutation flag and no adjacent graph gate for the same act.

Generic local capabilities do not impose human approval merely because they
write a file, execute a command, or send a bounded HTTP request. Their declared
scopes, workspace/target containment, and host grant are the permission
boundary. When a domain workflow genuinely needs a human decision and no
effect owns it, one explicit `ask` step may guard that exact action;
its verified receipt is bound to the governed step. The decision does not
become ambient authority for nested work.

An unresolved approval suspends the run after sealing its checkpoint; it is not
a failure or a blocked graph. The host protocol keeps the generic
`needs_agent` resolution envelope, while CLI JSON and operator output present an
approval-only request set as `needs_approval`. The caller or an integrated host
records the human decision under `approvals` and continues the same execution
through `runx resume`. Runx never opens a blocking terminal prompt, and there
is no interactive/non-interactive mode split. Exact paid-job authority may
satisfy the same approval slot when its principal, operation, plan, and
continuation bindings match; it is preauthorization, not a global auto-approval
bypass.

A consequential provider mutation therefore follows one chain:

    resolved authority
      -> exact effect-owned approval when explicitly required
      -> idempotent attempt
      -> provider acknowledgement
      -> identity-bound independent readback
      -> finality or explicit recovery state
      -> sealed receipt

Reads and drafts do not acquire performative approval gates. A receipt claims
only what the provider evidence proves. Ambiguous outcomes remain pending and
recoverable; they are never converted into success by retries or prose.

Payment subset comparison is compile-time exhaustive. Payment-marked or
payment-targeted work cannot route around payment admission, sequential
execution, receipt-before-success, or non-replay guarantees by selecting
another registered family.

## Authoring and extension boundary

Skill Lab and `runx new` call one authoring service and one closed,
digest-bound change contract. Design is judgment; inspect, validate, diff,
write, and harness execution are native mechanics. No template generator creates a
placeholder `.mjs`. No package writes outside its declared target. A no-op
design produces no file churn.

External extensions use stable manifests and wire protocols. They do not link
the runtime, fork the executor, or acquire a second authoring framework.

## Cloud boundary

All public skills, end-user and domain-operator commands and UX, local host
loops, queues, schedules, default local state, and operator orchestration live
in Runx OSS or the owning product repository. Using a hosted connector does not
move ownership into Cloud. Cloud may resolve an opaque grant and perform a
bounded provider call; the local runtime retains the workflow, decision state,
and receipt chain unless the user explicitly chooses a hosted operator service.

If a native operator surface is missing, add the reusable capability to OSS.
Never extend a Cloud dogfood script as a substitute.

Skill-declared provider scopes are opaque capability identifiers to the parser,
CLI, Connect grant transport, grant resolver, and receipt path. Those layers
preserve the exact ordered strings without delimiter parsing, a
provider-specific allowlist, an alias table, or translation. String-only host
boundaries carry the list as JSON, so punctuation, whitespace, order, and
duplicates cannot change its meaning. A concrete provider driver may map a
capability it actually implements to the provider's OAuth scopes and bounded
API operation; an unknown operation remains unsupported rather than being
silently broadened or rewritten.

Cloud separates authenticated source observations, verified immutable receipt
archives, and mutable tenant projections. Receiving a webhook is not governed
execution and never manufactures an execution signature. Receipt queries bind
the tenant before fetching an archived body. Hosted-run list filtering and
keyset pagination happen in storage before hydration; full maintenance scans
consume bounded pages. Registry publication, hosted-run domain rules, and
atomic effect state have distinct feature owners. Shared protocol code stays
below those features.

## Performance contract

Performance is measured by named, replayable workloads rather than line-count
intuition. Baselines cover graph planning and context projection, native
dispatch, deterministic module cold and warm execution, large payloads,
provider-effect transitions, receipt sealing/storage, and process launch.

The deterministic JavaScript adapter reuses supervised workers. Arbitrary CLI commands are
never pooled. Every workload records throughput, p99 latency, allocation or
resource signals where available, and process spawn count. Release gates compare
against a captured baseline; performance exceptions require explicit evidence,
not a silent budget increase.

## Replacement rule

A replacement and deletion land in the same governed phase. Runx does not keep
compatibility parsers, unsafe JavaScript fallbacks, duplicate authoring SDKs,
alias exports, dual effect drivers, or Cloud-owned local operator paths. When a
cutover cannot preserve the contract, it stops rather than shipping a weaker
parallel path.
