# Deterministic module engine decision

Status: accepted for implementation
Decision evidence: `docs/architecture/deterministic-module-engine.json`
Decision digest: `sha256:65d48defd9aab3d602d7c31ae105be3e3add6ea448a1a5ca623b114a88eac5e0`

## Decision

Runx's deterministic JavaScript lane will use
[Boa 0.21.1](https://docs.rs/boa_engine/0.21.1/boa_engine/) inside a dedicated
`runx-js-worker` process. A runtime session owns a lazy pool of at most four
supervised workers. Each worker executes one invocation at a time, so its wall
timeout is an independent process-kill boundary; sequential work reuses the
warm worker, while concurrent branches acquire separate workers. Every
invocation still receives a fresh engine context and a validated in-memory
module bundle. There is no Node, shell, CLI-adapter, or permission-flag
fallback.

This selects an engine and a containment design together. Boa is not loaded
into the main Runx process and is not treated as the process-security boundary.
The worker supervisor must enforce per-worker memory, per-invocation CPU time,
the aggregate pool cap, queue bounds, environment clearing, a non-workspace
current directory, protocol framing, and replacement after a fault. Boa
supplies the inner language boundary: no host APIs, a fixed clock, engine work
limits, fresh contexts, and an embedded module loader.

## Required behavior

The main runtime sends only protocol version, invocation id, entrypoint,
validated module bytes, JSON input, and limits over length-delimited standard
input/output. It never sends a workspace path, package path, credential,
provider grant, environment map, command, or network policy.

The worker:

- resolves only normalized relative `.js` and `.mjs` imports from the supplied
  bundle;
- rejects absolute paths, traversal, URLs, bare specifiers, `node:` modules,
  native modules, and imports not present in the validated bundle;
- exposes no filesystem, network, process, environment, credential, timer,
  host-clock, or host-randomness API;
- fixes `Date` to the invocation's deterministic clock contract;
- returns only JSON-compatible values through a bounded response frame;
- destroys the context after each result or error;
- causes the supervisor to replace only the affected worker after timeout,
  memory breach, protocol fault, panic, or crash. Healthy sibling workers
  continue. A typed module or execution rejection keeps that process only
  because its engine context is discarded and the next invocation receives a
  new one.

Production cutover is blocked until hostile modules prove those properties on
darwin-arm64, darwin-x64, linux-arm64, linux-x64, and win32-x64. The current
Node JavaScript adapter must be deleted in the same cutover; it is not retained
as compatibility behavior.

## Evidence and trade-offs

Three architecture classes were compared from digest-bound probe sources. The
machine-readable decision holds exact commands, source and artifact digests,
platform evidence, measurements, and rejected-alternative reasoning.

| Candidate | Fresh invocation | Probe RSS | Artifact | Result |
| --- | ---: | ---: | ---: | --- |
| Boa native worker | 118 μs | 10.7 MB | 10.6 MB | Selected |
| rquickjs native worker | 87 μs | 3.5 MB | 1.5 MB | Rejected |
| Boa in no-WASI Wasm | 194 μs | 219.6 MB | 6.7 MB guest + 11.9 MB host | Rejected |

The numbers are comparison probes, not product throughput claims. The native
Boa probe also fixed time to zero, exposed no `process`, `fetch`, or `require`,
stopped an infinite loop through the engine limit, resolved a relative module
from memory, and created 1,000 fresh contexts. Its Rust source checked for all
five Runx release targets.

The rquickjs probe was fastest and smallest, but its C engine made target
toolchains part of Runx's release and security surface. Linux arm64 and Windows
x64 cross-checks were blocked at the C compiler layer, and the evaluated
binding did not yet prove deterministic clock or hostile-module behavior on all
targets. That is the wrong portability risk for a core lane.

The no-WASI Wasm probe compiled Boa to `wasm32-unknown-unknown` with a
deterministic entropy backend. The resulting module had zero imports and ran in
fresh Wasmtime stores and instances. That is a strong structural host boundary,
but it added a compilation/runtime layer and measured roughly twenty times the
RSS of native Boa. Wasm remains a defensible future option if the threat model
changes; it is not justified for this worker now.

Implementation harnessing raised the worker thread stack ceiling from 1 MiB to
4 MiB after Boa aborted while parsing a valid 19 KB deterministic module. The
1 MiB setting was therefore not a safe containment boundary: it allowed normal
package structure to crash the worker before a governed error could be emitted.
The 4 MiB ceiling remains runtime-owned and bounded; Boa's separate recursion
limit, the 64 MiB JavaScript heap, the worker address-space ceiling, and all
source, input, output, job, and wall-time limits remain unchanged.

Javy and Extism's JavaScript PDK were investigated but do not satisfy the
required no-WASI candidate class: their official execution models require
WASI. They were therefore not mislabeled as the Wasm alternative in the final
comparison.

## Why this is the narrow choice

This decision does not make arbitrary JavaScript a native capability and does
not promote package-specific computation into Rust. It supplies one reusable,
authority-free computation lane for cases where declarative workflows and native
capabilities cannot express the domain transformation cleanly. Provider work,
local commands, and agent judgment remain separate execution kinds with their
own authority contracts.
