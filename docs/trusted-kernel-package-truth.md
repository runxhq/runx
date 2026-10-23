# Trusted kernel package truth

Status: current. The normative owner is
[Runx system architecture](./architecture/runx-system.md). This document records
Rust verification and dependency policy; language-package decisions live in
[TypeScript interop boundary](./ts-interop-boundary.md).

## Runtime authority

Rust owns local package parsing, policy and authority, graph execution, harness
execution, receipts, registry/configuration, native capabilities, and process
supervision. `runx-core` contains pure policy and transition algebra;
`runx-runtime` performs effects. CLI and MCP skill calls use the same runtime
orchestrator and skill front. Neither transport reconstructs execution from
output JSON or implements its own approval/agent continuation.

Operator judgment belongs to OSS or owning-product skills. Generic pure policy
does not choose pull requests, source triage, or operator outreach. Cloud
receives authenticated observations and executes bounded provider calls;
using hosted credentials does not transfer local workflow ownership.

## Verification

`pnpm rust:check` runs Cargo formatting, Clippy, workspace tests, crate
dependency guards, `cargo-deny`, and the `runx-core` public API snapshot. CI and
release checks also exercise feature combinations and doctests. A fixture-only
pass is not evidence that a CLI journey or adapter works.

Kernel fixtures under `fixtures/kernel/` are generated through the native Rust
evaluator by `scripts/generate-kernel-parity-fixtures.ts`. The TypeScript tool
is a harness, not a policy oracle. Review fixture input and expected-result
diffs deliberately. Do not regenerate a failing expected result solely to
match an implementation change. Surviving TypeScript contract helpers require
cross-language conformance; scope matching has a differential corpus covering
namespace boundaries, wildcards, empty segments, and punctuation.
That corpus uses the development-only `runx-core` example `kernel_eval_batch`
to evaluate all cases in one process through the same Rust evaluator. The
driver is excluded from the published crate; direct CLI edge checks remain.

Policy executable-name normalization treats backslashes as path separators on
every host. For example, `C:\Tools\node.exe -e ...` normalizes to `node` and is
denied under strict inline-code policy.

`fixtures/cli-parity` records native command journeys. Wrapper tests must run
the native binary and assert its observable contract. Missing binaries or
unsupported lanes must fail clearly; no hidden JavaScript executor may fill in.

## Dependency policy

`crates/deny.toml` and the crate-graph check enforce the dependency direction:

- Pure crates (`runx-contracts`, `runx-core`, `runx-parser`, `runx-receipts`,
  and `runx-sdk`) do not acquire async runtimes, HTTP clients/servers, MCP
  frameworks, or alternate YAML backends.
- `runx-runtime` owns side-effect dependencies behind their feature flags:
  `reqwest`/`rustls` for adapter HTTPS, `tokio` for async supervision, and
  `rmcp` for MCP. They do not move into pure crates. The CLI consumes these
  services rather than growing a second transport or supervision stack.
- `runx-js-worker` is the isolated deterministic JavaScript engine, shipped as
  one versioned distribution with the CLI. It carries no provider authority.
- `runx-x402` contains inert protocol presentation; provider network calls and
  payment state do not enter it or the generic kernel.
- `serde_norway` is the parser backend; `serde_yml` and `serde_yaml` are not
  approved alternatives.

Check current manifests and deny rules before changing feature placement.
Historical Rust-port plans explain earlier transitions; they are not current
permission to reintroduce an alternate local implementation.
