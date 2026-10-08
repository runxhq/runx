# runx Rust Crates

This workspace contains the Rust packages that back the `runx` distribution:
contracts, kernel decisions, parser, receipts, the native runtime, the CLI
binary, and the blocking SDK. Architectural authority lives in
[`docs/architecture/runx-system.md`](../docs/architecture/runx-system.md); the
language package boundary lives in
[`docs/ts-interop-boundary.md`](../docs/ts-interop-boundary.md).

## Commands

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo package --workspace --allow-dirty
node ../scripts/check-rust-kernel-parity.mjs
node ../scripts/check-rust-crate-graph.mjs
```

The compiler and workspace Clippy lints (see [`Cargo.toml`](Cargo.toml)) deny `unsafe_code`,
`unwrap_used`, `expect_used`, `panic`, `dbg_macro`, `print_stdout`,
`print_stderr`, `todo`, `unimplemented`, and `wildcard_imports` across every
crate. Contract fixtures are enumerated by their owning native generators and
tests; no source-text parser defines Rust semantics. Adapter-tier dependencies
(async runtimes, HTTP clients, MCP protocol crates) require an explicit spec
before they may be added to `deny.toml`.

## Layout

- `runx-cli`: native `runx` binary. Hand-rolled dispatcher across `harness`,
  `connect`, `config`, `policy`, `kernel`, `doctor`, `list`, `history`, `mcp`,
  `tool`, `registry`, `skill`, plus project/router plumbing. Activation
  versus the npm CLI is documented by the current
  [TypeScript interop boundary](../docs/ts-interop-boundary.md).
- `runx-contracts`: pure public contracts for JSON, host protocol, receipts,
  registry/tool records, act assignment, harness spine, generic authority and
  effect finality.
- `runx-core`: pure decisions. State-machine parity and policy parity
  (admission, authority proof, retry, graph-step scope, generic
  authority subset).
- `runx-parser`: pure YAML → AST → IR parity for graphs, skills, runners, tool
  manifests, and skill installs. Raw object subtrees use
  `runx_contracts::JsonValue`.
- `runx-receipts`: pure receipt model, canonical hashing, and tree
  verification with an adversarial unit matrix.
- `runx-runtime`: impure runtime. Owns filesystem, exact process invocation and
  supervision, journals, registry clients, harness replay, doctor, dev loop,
  project initialization, generic authority gating, and the adapter set. Adapter
  families are opt-in features: `async-http`, `process-tool`, `mcp`,
  `mcp-http-server`, `agent`, `catalog`, and `store`.
  The `async-http` feature owns runtime HTTP with reqwest over rustls, disables
  redirect following, and uses bounded request/connect timeouts. `process-tool`
  enables `async-http`; defaults keep the runtime dependency-light.
- `runx-sdk`: blocking CLI-backed Rust SDK v0. Depends on `runx-contracts`
  only; explicit non-dep on `runx-core` and `runx-runtime`.
- `runx-x402`: pure x402 v2 presentation, bounded header codecs, and exact
  Runx invocation-echo validation. Depends on `runx-contracts` only and owns
  no provider verification, settlement, credentials, network, or runtime I/O.

Pure crates (`runx-contracts`, `runx-core`, `runx-parser`, `runx-receipts`,
the v0 `runx-sdk`, and `runx-x402`) carry no async, HTTP, or process-spawn
dependencies. The runtime crate owns those.

For kernel parity, run `pnpm rust:check` from `oss/` or
`node ../scripts/check-rust-kernel-parity.mjs` from `oss/crates/`. Install
optional tools with `cargo install cargo-deny` and
`cargo install cargo-public-api --version 0.51.0`, plus
`rustup toolchain install nightly --profile minimal`.

Commit the single workspace lockfile at `crates/Cargo.lock`; the workspace
contains a binary and publishable libraries.
