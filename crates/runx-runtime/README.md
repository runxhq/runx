# runx-runtime

Native Rust runtime for governed runx execution.

This crate owns the canonical local orchestration path for Rust-backed runx:
skill execution, graph execution, harness replay, host reporting, exact process
invocation and supervision, typed execution-boundary evidence, receipts,
history projection, adapters, and domain-free effect orchestration. Pure
parser, core, contract, receipt, and domain crates remain upstream.

Current slice:

- parses a skill's runners, each a graph of agent, tool, operation, ask and
  skill steps, with `runx-parser`
- plans sequential transitions with `runx-core`
- runs worker tools through the dedicated `runx-js-worker`; trusted host
  processes remain behind the `process-tool` feature
- emits receipts and validates the parent receipt tree with
  `runx-receipts`
- exposes native skill, doctor, list, history, MCP, registry, config, policy,
  tool, and dev command support through `runx-cli`

The worker lane is part of the core runtime. It has no Node or CLI fallback and
exposes no host authority. Adapter families that can cross
host or provider boundaries remain feature gated:

- `process-tool`
- `mcp`
- `mcp-http-server`
- `agent`
- `catalog`
- `store`
- `async-http`

The CLI enables `process-tool`, `catalog`, `mcp`, `mcp-http-server`, `agent`,
and `store`. `process-tool` enables `async-http` transitively.

The generated catalog-profile capability snapshot lives at
`fixtures/tool-catalogs/native-capabilities.snapshot.json`. It pins the
runtime-owned roster, scopes, effect and approval posture, packet binding, and
execution boundary for the explicit `catalog`/`process-tool`/`async-http` feature
set. Run `pnpm capabilities:snapshot:generate` only for an intentional contract
change; `pnpm catalog:check` is the freshness wall.

## Doctor

The native Rust doctor API is wired into `runx-cli` for the read-only
diagnostic surface. It must not shell out to npm or TypeScript for canonical
local behavior.

This crate currently ports the read-only fixture-backed diagnostics:

- `runx.tool.fixture.missing`
- `runx.skill.fixture.missing`
- `runx.structure.file_budget.exceeded`
- `runx.structure.cross_package_reach_in`

Deferred doctor families remain owned by follow-up slices:

- `runx doctor --fix` repair writes
- diagnostic catalog, `--list-diagnostics`, and `--explain`
- official skills lock freshness
- tool manifest stale source and schema hashes
- packet index diagnostics
- graph packet path validation
- receipt proof health
- policy health
