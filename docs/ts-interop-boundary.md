# TypeScript interop boundary

This document records the TypeScript and Python package boundary for
the native runtime. It specializes the normative
[Runx system architecture](./architecture/runx-system.md) for language packages.

## Current boundary

Rust is canonical for trusted local runtime and execution: local skill and
graph execution, harness and dogfood execution, receipt sealing and
verification, history, policy and registry configuration, generic authority and
effect admission, exact process invocation and supervision, typed
execution-boundary evidence, built-in adapter execution, and external
execution-adapter supervision. Runx is a permissions broker, not a portable OS
jail: Runx delivers a documented launch baseline plus resolved inputs, declared
environment, and credential material to trusted host processes, then reports
them honestly without a filesystem, network, or syscall-confinement claim.
TypeScript remains for generated contracts, CLI/client wrappers,
cloud/product integrations, host adapters, docs, and narrow helper SDKs over
language-neutral external protocols. Native authoring plus Skill Lab own skill
creation and improvement. TypeScript does not own a parallel authoring service
or a local executor-package fallback for trusted local behavior.

MCP follows the same rule. `rmcp` is an adapter-tier Rust dependency for MCP
protocol behavior, while runx keeps graph state, policy, authority, approvals,
and receipts in the Rust kernel. TypeScript MCP code may survive only as
generated contracts, hosted cloud code, helper SDKs over documented protocol
surfaces, or contract fixtures. It must not execute trusted local fallback
behavior.

The package CLI is a distribution and UX shim, not a second runtime. A usable
installation must be able to execute the Rust `runx` binary without TypeScript
source, tsx, or a local workspace. If the wrapper cannot find a supported
native binary, it should fail closed with installation guidance rather than
falling back to a TypeScript implementation of local behavior.

Four crossing families exist between the TypeScript surface and the
Rust runtime. Each crossing has a contract surface that owns the wire shape.

1. **CLI JSON.** Anything that shells `runx`, including package launchers, the
   LangChain bridge, `runx-py`, user scripts, and CI workflows. The Rust CLI
   owns behavior; the contract is each command's JSON output shape, exit
   codes, and human-output stability.
2. **Published TypeScript contracts.** Anything that imports from
   `@runxhq/contracts`, including host adapters, cloud packages, and external
   TypeScript consumers. The contract is the TypeScript shape that mirrors
   `runx-contracts` Rust types, with fixture cross-validation.
3. **Cloud HTTP contracts.** Anything where the Rust runtime calls cloud, such
   as approval routing, connect/auth, registry, and receipts-store. The
   contract is versioned, documented HTTP endpoints.
4. **Language-neutral extension protocols.** Anything the Rust runtime launches
   or calls outside the trusted local runtime. This is a family of protocols, not
   one catch-all plugin API: skill subprocess ABI, source-event ingress,
   hosted/embedded runtime binding, and tool catalog/read model access are
   distinct lanes. The contract is the lane-specific language-neutral protocol
   and manifest shape, not a TypeScript package API or an in-process Rust trait.
   External authors can implement these protocols in TypeScript, Python, Rust,
   or another language without a core fork.

No fifth boundary is added without updating this document. Each package
disposition is named here. Stable-boundary edits to consume new
`runx-contracts` versions are normal maintenance.

### Shipped vs defined, and current boundary debt

The crossing families above describe the intended boundary. Where specific
work belongs, and the boundary debt that remains, is recorded here rather than
implied.

- **Provider integrations are connector operations.** Connector manifests
  declare the HTTP or MCP implementation behind each operation. New integration
  work lands there instead of adding provider HTTP clients to the kernel.
- **Effects: authority is Rust, domain interpretation stays outside the generic
  kernel.** Admission, capability binding, proof validation, and receipt sealing
  are Rust and stay Rust. Domain network legs such as payment settlement
  provider calls or external-signer calls are not kernel work: they are
  non-deterministic, secret-bearing or network-bearing, and offline-impossible,
  so they belong in connector operations behind the generic effect kernel.
- **Provider clients belong outside the kernel.** GitHub PR creation, provider
  outcome observation, and source-thread publication are owned by adapters or
  product workflows, not by in-kernel provider clients. When provider calls are
  wired for real, they belong in connector operations rather than as
  token-bearing kernel `reqwest` clients. Payment's inert rail dispatcher
  and HTTP clients were also removed; real rails are rebuilt as generic effect-family
  adapters behind the kernel. The deterministic halves these feed are pure and
  correctly stay in the kernel; only the network legs move.
- **One local execution path.** Native CLI and MCP skill calls use the same
  `LocalOrchestrator` and skill front for preparation, invocation, pause/resume,
  and typed results. The Rust managed-agent loop is opt-in; host-driven judgment remains the default.
  Cloud's `@runx/mcp` exposes hosted registry/auth/control-plane operations;
  submitted hosted execution invokes the native worker boundary. It is not a
  duplicate local MCP skill executor.

## OSS package dispositions

| Package | Disposition |
| --- | --- |
| `@runxhq/contracts` | Publishes portable packet definitions and the generated TypeScript view of Rust-owned wire schemas, maintained with fixture cross-validation. |
| `@runxhq/extension-sdk` | Owns only the process tool helpers needed by genuine external extensions. It does not author skills or own trusted local execution. |
| `@runxhq/cli` | Stays as a platform-aware npm launcher that resolves and execs the Rust binary. It must remain useful from an installed package without TypeScript sources and must fail closed instead of falling back to TypeScript local execution. It also carries the drift-free cold-start: `npx @runxhq/cli new <name> --objective <outcome>` downloads the launcher and enters the same canonical Skill Build authoring lane without a prior Runx install. The launcher owns no templates or authoring logic. |
| `@runxhq/core` | Deleted. Its registry/config/parser remnants were not a shipped execution boundary; live OSS code uses Rust crates, generated contracts, tool-local modules, or explicit protocol packages instead. Cloud uses its owning feature packages and the small `@runx/protocol` contract/utility subpaths. |
| `@runxhq/client` | Typed finite Node invocation of the native CLI, with bounded processes and artifact decoding. The `./adapters` subpath formats native results without Node or framework imports. The previous host-adapters package and unimplemented HostBridge run-state contract are removed. |
| `@runxhq/langchain` | Optional LangChain tool formatting over `@runxhq/client`; it does not own subprocess transport or continuation. |
| `runx-py` | Stays as a thin Python client over `runx` CLI JSON output. |

Cloud packages remain TypeScript. The Rust runtime consumes cloud through the
cloud HTTP contracts. Local registry and policy configuration remains
Rust-owned when exercised by the native CLI. Cloud/product integrations, host
adapters, and protocol helper SDKs can remain TypeScript as long as they stay
on one of the contract surfaces above or the cloud-owned
`@runx/protocol` helper package. External integration authors target the correct
language-neutral protocol lane; they must not need Rust, `runx-core`,
`runx-runtime`, or a fork of the core repository to ship an extension.

## Test ownership

Language-owned unit tests stay with their implementation. Contract and parity
tests use durable fixtures under `fixtures/`. End-to-end tests should spawn the
`runx` binary and assert stdout, exit codes, and JSON instead of importing
TypeScript internals. Trusted local graph, harness, receipt, authority,
registry/policy config, and payment behavior needs Rust coverage or a TS-free
Rust CLI fixture before wrapper tests can count as proof. Extension-lane tests
prove protocol conformance by spawning or simulating the external process
through the lane's documented wire contract, not by importing TypeScript
runtime-local internals.

Kernel fixture generation calls the native Rust evaluator. TypeScript fixture
tools are serialization and conformance harnesses, not alternate local policy
implementations. Surviving cross-language wire helpers, such as scope matching,
require differential coverage against the native owner.
