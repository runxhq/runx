# @runxhq/client

The Node client for the native Runx runtime. Hosts choose a binary and explicit
workspace paths; Rust owns preparation, execution, continuation and receipts.
This package has no agent loop, persistence adapter, scheduler or hosted gateway.

```ts
import { createRunxClient, executionBindingFromInspection, hostEnvironmentFrom } from "@runxhq/client";

const runx = createRunxClient({
  command: "/usr/local/bin/runx",
  cwd: "/work/session/workspace",
  projectDir: "/work/session/project",
  home: "/work/session/home",
  environment: hostEnvironmentFrom(process.env),
});
const inspection = await runx.skills.inspect({ skillPath: "/skills/research" });
const output = await runx.skills.run({
  skillPath: "/skills/research",
  binding: executionBindingFromInspection(inspection),
  inputs: { question: "What changed?" },
  receiptDir: "/work/session/receipts",
});
```

The host creates these directories and selects its credential and signing
configuration. `environment` in client configuration is trusted host input;
per-invocation `environment` contains skill variables and cannot replace native
controls or process-loading variables. Nothing inherits `process.env` implicitly.
`hostEnvironmentFrom` copies only machine interoperability variables from its
explicit input; add required native signer or verifier controls deliberately.
Directory separation is not an operating-system sandbox.

## Operations

| Operation | Native command |
| --- | --- |
| `skills.inspect({ skillPath, runner? })` | `skill inspect` |
| `skills.run({ skillPath, inputs?, receiptDir, binding?, runner?, act?, context?, runId?, runAuthority? })` | `skill` |
| `runs.resume({ runId, resolutions, receiptDir, binding? })` | `resume` |
| `turns.server({ runId, receiptDir })` | `mcp --run` (started by your MCP client) |
| `packages.materialize({ skillPath })` | `registry package` |
| `harness.run({ skillPath, receiptDir })` | `harness` |
| `receipts.verify({ receipt, trust? })` | `verify --receipt - --trust home\|configured` |

Each operation accepts `signal`. Configuration sets `timeoutMs`, combined
`maxOutputBytes`, `maxInputBytes`, a total `maxArtifactBytes` budget and
`terminationGraceMs` (one second by default).
A run's inputs and a resume's resolutions travel to native on stdin, never in a
file or on argv. Input keys are never converted into CLI flags. `act` states the
act the run is invoked with, `context` gives it the caller's current and
historical context (which informs the run and never grants authority), and
`runId` pins its submission identity, so a retry continues the same run.
The resume packet passes to native unchanged; the client does not assign actors,
answer approvals, retry a mutation or reinterpret a checkpoint.

When a run returns `needs_agent` on an agent act, the act's request carries its
`allowance`: the connector operations, tools, child skills and asks the agent
may call. `turns.server` returns the command, arguments, environment and
working directory an MCP stdio client starts to serve the run's open turns.
Its tools are fixed (`turn_list`, `turn_status`, `turn_call` and
`turn_complete`) and take a turn's address, so a turn that a call opens in a
child skill is worked on the same connection. `turn_status` returns a
`TurnStatusContract`: the turn's act and the calls it may make, each a
`TurnToolContract` with a name, target and input schema that a host can
present to its model as a typed tool. Each call returns a `TurnCallContract`.
All three come from `@runxhq/contracts`. Runx admits, executes and records
every call under the run's authority. A write names its `idempotency_key`. A
pending call lists what it waits on. The host answers an approval through
`runs.resume` under `approvals`, by its address. `turn_complete` ends a turn
with its output once none of its calls is pending, and the run continues.

Inspection is uncached. An immutable deployment may cache at its owning host.
An execution binding pins runner, package and execution closure, and native
revalidates it at invocation. Mutable local paths must be inspected again.

Skill output preserves the native status and semantic outcome, including
`needs_approval`, `payment_required` and a sealed failure returned on exit 1.
Digest-bound requests, spilled request sets, oversized results and diagnostics
are hydrated under the configured project directory's `artifacts` root. Artifact
reads check regular files, size, content address and identity; malformed or
missing evidence rejects the invocation. A sealed output must name its receipt
and closure. Hydrated receipts pass the native-generated schema and identity
checks. Cryptographic verification and trust decisions require
`receipts.verify`; hydration alone does not establish trust.

`RunxClientError.reason` distinguishes process, exit, contract and artifact
failures. Native run outcomes are returned as data, including failed outcomes;
they are not interchangeable with transport failure. POSIX cancellation and
timeouts terminate the process group, including surviving descendants. Windows
uses `taskkill /T /F`; its process-tree behavior still requires Windows CI proof.

## Portable response formatting

```ts
import { toOpenAiResponse } from "@runxhq/client/adapters";
const response = toOpenAiResponse(output);
```

The adapters export also provides Anthropic, Vercel AI, LangChain and CrewAI
response formatting. It preserves native output and imports no Node runtime
modules or framework SDKs. It does not invent run-state inspection or implement
an assistant harness. `@runxhq/langchain` separately wraps a finite client call
as a LangChain tool and leaves pending outcomes to the host.

## Status

`@runxhq/client` is published with each runx release from 0.10.0, at the
version that release names. Its own tests drive the process and artifact
boundary against a scripted stand-in for the native binary; runx Cloud's
hosted runs drive it against the released CLI.
