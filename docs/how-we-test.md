# How We Test

Runx has two local test lanes: a fast loop for package-adjacent work and a full
workspace suite for release confidence.

Rust runtime work has four explicit gates:

| Gate | Purpose | Command shape |
| --- | --- | --- |
| Local fast | Tight edit loop for nearby package/runtime changes. | `pnpm verify:fast` or a focused `cargo test --manifest-path crates/Cargo.toml -p <crate> ...` |
| CI fast | Deterministic semantic and boundary checks that should run on every review. | `pnpm boundary:check`, `pnpm typecheck`, focused Rust contract/runtime tests |
| Heavy | Perf, fanout, MCP, external-process, and oracle checks that are useful before release or risky runtime changes. | `pnpm stress:runtime:*`, `pnpm perf:runtime:check -- --baseline <path>`, `runtime quality` workflow |
| Soak | Long-running replay/stress loops that should be invoked intentionally, never hidden inside the default workspace test. | Repeated stress commands under an external runner with captured JSON output |

On macOS 26, complete the
[Developer Tools permission prerequisite](../CONTRIBUTING.md#macos-developer-tools-permission)
before investigating a Rust build or test process that appears to stall.

Do not hide heavy or soak work inside `cargo test --workspace` or `pnpm test`.
The normal loop should fail fast; replay and stress gates should produce
machine-readable output that can be archived with the spec or CI run.

## Fast Loop

Use this while editing core runtime, harness, parser, policy, or nearby tests:

```bash
pnpm test:fast
```

`test:fast` uses `vitest.fast.config.ts`. It includes package tests plus
coverage for surviving TypeScript package boundaries.

For canonical local runtime behavior, prefer the Rust lane directly. Authority,
receipt, harness, registry, and policy-config changes need Rust coverage. The
real-payment boundary is proved as hosted provider contracts; deterministic
local payment behavior is limited to the three mock skill packages:

```bash
cargo test --manifest-path crates/Cargo.toml -p runx-cli --test integration
runx harness skills/mock-pay
runx harness skills/spend/fixtures/hosted-contract.yaml
```

For one file:

```bash
pnpm vitest run tests/examples/hello-world.test.ts
```

## Full Suite

Use this before review or when changing CLI packaging, dist output, package
exports, or cross-package TypeScript wrapper behavior:

```bash
pnpm test
```

`pnpm test` runs `scripts/test-workspace.mjs`. With no explicit target, it runs
the workspace suite except `tests/cli-package.test.ts`, then runs
`tests/cli-package.test.ts` in a second pass with:

```bash
RUNX_VITEST_BATCH=cli-package
```

That ordering is intentional. `cli-package.test.ts` rebuilds and inspects
package output, so isolating it avoids races with tests that import from the
same dist trees.

To run the CLI package test directly:

```bash
RUNX_VITEST_BATCH=cli-package pnpm vitest run tests/cli-package.test.ts
```

## Fixtures

Use checked-in fixtures when a behavior should remain stable:

- `fixtures/skills/` for reusable skill packages
- `fixtures/graphs/` for graph execution shapes
- `fixtures/harness/` for harness-level contracts
- `examples/` for public docs examples that should also be executable

Prefer small fixtures with one purpose. If an example appears in docs, add a
test or harness so the docs fail loudly when the runtime shape changes.

Harness replay is owned by Rust. The fixture registry lives in
`runx_runtime::harness::list_cases()`, and the
`runx-harness-fixture-oracles` binary consumes that same registry for checks,
regeneration, and summary output:

```bash
pnpm fixtures:harness:check
pnpm fixtures:harness:summary
```

The summary path emits one JSON record per case with status, elapsed time,
receipt id, receipt digest, and failure classification.

## Runtime Stress

Adapter and fanout stress gates are explicit scripts:

```bash
pnpm stress:runtime:mcp
pnpm stress:runtime:cli-tool
pnpm stress:runtime:fanout
```

These commands exercise MCP server wiring, CLI-tool process supervision, and
fanout ordering/concurrency.
They are heavy gates, not the default local loop.

The `runtime quality` workflow is the canonical automated heavy lane. It runs
weekly, on explicit dispatch, and before a release build. One Ubuntu job reuses
its Cargo cache and runs the official-skill audit, receipt ownership checks,
the three stress commands, and a focused performance capture. The capture is
bound to the exact source commit, enforces worker spawn and parallel fanout
budgets, and is uploaded as a retained artifact before enforcement. Exact
four-way saturation remains a deterministic barrier-backed runtime test rather
than a scheduler-sensitive benchmark assertion.

Absolute timing comparisons still require two reports captured on comparable
hardware. Do not compare a developer-laptop baseline with a hosted runner or
turn a noisy cross-machine number into a release claim. Use
`perf:runtime:check` with a hardware-matched baseline when the runtime hot path
changes; use the scheduled artifact for exact-commit evidence and trend
inspection.

## Packaged Release Candidate

The release gate tests the extracted archive, not a source-tree substitute.
Each native release runner invokes:

```bash
node scripts/smoke-release-candidate.mjs \
  --runx-bin <extracted-runx> \
  --expected-version <version>
```

The smoke proves that the archive contains its adjacent JavaScript worker and
that the packaged CLI can execute a nested signed-registry skill, pass only a
declared workspace environment variable into frozen JavaScript context,
deliver a complete over-one-megabyte `SKILL.md` with a matching digest,
preserve an opaque provider scope, close one consequential action with one
host-attested human approval while rejecting the same decision from the agent
answer lane, and stop an active JavaScript run on interruption. The Windows
release target relies on the dedicated Windows
host-job lifecycle gate for the interruption invariant; every other packaged
invariant runs against every extracted archive before any channel publishes.

Release preparation separately queries GitHub's check runs for the exact
candidate commit. Both the aggregate `checks` job and `gitleaks` must have
completed successfully. A green branch, a different commit, or a successful
archive version print is not release evidence.

## Adding Tests

Use package-local tests for package internals and `tests/` for cross-package
wrapper behavior. Trusted local skill, graph, harness, receipt, policy,
authority, registry, config, and payment behavior needs a Rust test or a
TS-free Rust CLI fixture. TypeScript tests may wrap those paths, but they
should not be the only proof.

For docs examples, keep the test focused on the public command or runtime path
the docs promise. The hello-world and hello-graph tests are the reference shape.
