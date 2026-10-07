# Rust kernel architecture

Status: historical architecture note. The Rust local runtime has cut over; the
older Rust parity prerequisite specs named by this document are archival context,
not active release blockers.

This document captures the architectural decisions that the Rust parity
specs depend on. The goal is to make the choices explicit once, so each spec
can reference them rather than rederive them.

## 1. Position

The Rust kernel started as conformance evidence for the TypeScript trusted
kernel, but the local runtime cutover is now the operating boundary. For trusted
local execution, Rust is the canonical owner once a command is advertised by
the native CLI or runtime. That includes graph execution, harness and dogfood
execution, receipt sealing and verification, policy and registry configuration,
authority admission, effect-family authority, permission delivery, and local
built-in adapter execution. Local process sources are trusted host code: Runx
controls their exact invocation and delivered grant, but does not claim portable
syscall or filesystem confinement.
TypeScript remains for generated contracts,
CLI/client wrappers, cloud/product integrations, host adapters, authoring
tooling, docs, conformance tests, and helper SDKs over language-neutral
protocols. It is not a runtime-local or adapters fallback for trusted local
behavior.

Rust ownership of orchestration does not require extension authors to write
Rust. Tool authors write `cli-tool` or `javascript` tools against
language-neutral manifests and process contracts; they must not require
`runx-runtime`, `runx-core`, or a fork of the core repo. Source ingress, hosted
runtime binding, and tool catalog/read-model are separate extension lanes.
Rust crates exist to:

- Prove behavioral parity through a shared fixture suite while a domain is
  still in the dual-tree window.
- Make TypeScript kernel drift explicit (intentional fixture refresh required).
- Provide the native local CLI/runtime path for skill, graph, harness, receipt,
  history, policy, authority, effect-family admission, permission delivery, and
  process supervision.

Rust is not a second source of truth for cut-over surfaces; it is the source of
truth. If Rust and TypeScript disagree on a fixture before a surface cuts over,
the active cutover spec decides whether the fixture is still a TypeScript
oracle or a Rust contract fixture.

MCP is native at the runtime boundary but not the kernel itself. The accepted
cross-repo decision is recorded in
`../../docs/mcp-native-runtime-boundary.md`: `rmcp` owns MCP protocol semantics
inside the adapter tier, while runx owns graph state, admission, authority,
approvals, pause/resume, receipts, and run identity.

## 2. Pure kernel scope

"Pure kernel" in this document means exactly what the existing boundary check
enforces, not what the trusted-kernel doc lists. `oss/scripts/check-boundaries.mjs`
defines `pureCoreDomains = ["parser", "policy", "state-machine"]` and forbids
those domains from importing `fs`, `child_process`, `http`, `net`, and the
other node IO modules.

This document was written during the TypeScript-to-Rust kernel migration. The
trusted-kernel TypeScript paths it describes are now historical; `runx-core`
owns the active state-machine and policy behavior.

- `executor` (369 lines)
- `marketplaces` (245 lines)
- `parser` (1658 lines)
- `state-machine` (667 lines)
- `policy` (retired from TypeScript and Rust-owned)

The plan ported state-machine + policy into Rust, which is 100% of
what `pureCoreDomains` enforces today. The remaining pure-by-imports domains
(`executor`, `marketplaces`, `parser`) are candidates for follow-up parity
specs; their boundary status would need to be added to `pureCoreDomains`
before a Rust port is meaningful.

The other five domains (`artifacts`, `config`, `knowledge`, `receipts`,
`registry`) use node modules and would need TS-side purification or a
clean split into pure-decision + impure-IO halves before they could move
to Rust.

So the scope of this plan is narrow but defensible: it covers exactly what
the existing repo defines as pure. Future scope expansion is a sequence of
explicit follow-up specs, not a vague "port the kernel" promise.

## 3. Target crate graph

The future workspace under `oss/crates/`:

```
runx-contracts    pure public contracts: CLI JSON, host protocol, receipts,
                    registry/tool records, act assignment
                    deps: serde, sha2
                    deferred deps: serde_json/thiserror only when concrete code
                                   needs them outside tests

runx-core         pure decisions: state-machine, policy, scope, grant attenuation
                    deps: runx-contracts as needed, serde, thiserror as needed,
                          serde_json for private deterministic JSON canonicalization
                    posture: std default; no_std deferred to a follow-up spec

runx-parser       pure: YAML -> AST -> intermediate representation
                    deps: runx-contracts, runx-core, serde, serde_norway,
                          serde_json, regex, thiserror
                    posture: public raw object subtrees use
                             `runx_contracts::JsonValue`; execution
                             semantics use `runx_contracts::execution`;
                             grant admission uses `runx_core::policy`

runx-receipts     pure: receipt model, hashing helpers, verification rules
                    deps: runx-contracts, serde, sha2

runx-sdk          library: blocking CLI-backed SDK v0; future async path
                    deps: runx-contracts only in v0
                    explicit non-dep: runx-core in v0

runx-runtime      impure: filesystem, subprocess, network, built-in adapter
                  execution, MCP, typed execution-boundary evidence and
                  process supervision
                    default features: none
                    opt-in features: async-http, cli-tool, mcp,
                                     mcp-http-server, agent, catalog
                    deps: runx-contracts, runx-core, runx-parser,
                          runx-receipts; async/network dependencies require
                          explicit adapter-tier exception specs

runx-cli          binary: argument parsing, presentation, exit codes
                    includes: thin `runx new` entry into the runtime-owned
                              Skill Lab authoring service
                    deps: runx-runtime (long-term)
                    current: native local command surface for advertised Rust
                             commands; npm package is selector/client wrapper
```

Pure crates (`runx-contracts`, `runx-core`, `runx-parser`, `runx-receipts`,
and the CLI-backed v0 `runx-sdk`) depend only on each other when that coupling
is needed plus parsing, hashing, and serde-style support crates. `runx-parser`
depends on `runx-contracts` for JSON and execution-semantic boundary types and
on `runx-core` for grant policy, so parser parity exercises the same typed Rust
surfaces the runtime consumes.
`runx-sdk` is special: it is pure library code plus a blocking CLI client, but
it is not part of the trusted kernel and must not depend on `runx-core` in v0.
Crates below the runtime line own all side effects. The boundary between pure
and impure is enforced both by dependency direction and by `cargo-deny`/lint
rules (see section 10).

Parser parity uses `serde_norway` as the YAML backend. Raw object subtrees use
`runx_contracts::JsonValue`, and execution semantics are validated into the
`runx_contracts::execution` types so parser fixtures detect drift against the
contracts crate instead of carrying a duplicate local model.

Order of operations is committed, not loose. Pure crates ship first:

1. `rust-contracts-bootstrap`: crate graph, placeholder reservation versioning, and
   `runx-contracts` placeholder guardrails. This is the pre-kernel execution
   gate.
2. `runx-core` (this plan): state-machine + policy parity.
3. `runx-contracts`: public JSON/host/receipt contract parity for SDK/runtime.
   Follow-up spec.
4. `runx-parser`: pure YAML/AST/IR parity. Implemented through
   `rust-parser-parity` for graphs, skills, runner manifests, tool manifests,
   and skill installs.
5. `runx-receipts`: pure receipt model + verification rules. Follow-up spec.

Only after the initial pure set passes parity does any impure crate begin:

6. `runx-sdk` CLI-backed v0 can ship once its consumed `runx-contracts`
   subset and CLI JSON cases are fixture-backed.
7. `runx-runtime` skeleton with one impure adapter execution path ported
   as a runtime feature (cli-tool first; MCP last because rmcp + tokio +
   process and protocol semantics are the hardest cross-language surface).
8. `runx-cli` native binary, gated by `rust-cli-feature-parity-matrix`.
9. `runx-sdk` native-runtime feature, gated by the same runtime and CLI
   feature-parity evidence.

Each step is its own design pass. MCP cannot jump the queue.

`runx-sdk` has a special early path: a CLI-backed Rust SDK can ship before
`runx-runtime` exists, as long as it calls the authoritative `runx` binary and
only consumes documented JSON output from `runx-contracts`. That early SDK is
a blocking client wrapper and host-protocol type layer, not a native runtime.
Once `runx-runtime` exists, a separate spec may add the async SDK path. The
likely shape is `runx-sdk` exposing async APIs by default with a `blocking`
facade feature or sibling facade module, but that is not part of v0. SDK v0 is
allowed to block because it is only a subprocess-backed bridge to the current
CLI.

contracts-first-ordering: `runx-contracts` owns host protocol, capability
execution, idempotency hashes, and consumed JSON contract types before SDK
Phase 2. `runx-sdk` may depend on
`runx-contracts` in CLI-backed v0, but it must not duplicate contract-owned
types or hash helpers.

There is no `runx-authoring` crate. Skill authoring has one service in
`runx-runtime`, composed by `skill-lab`; `runx new` is a thin CLI entry into
that lane. Neither `runx-sdk` nor a TypeScript package owns a second authoring
implementation. The old TypeScript package split is useful history, not a
forcing function for Cargo crates.

There is also no `runx-adapters` crate in the initial Rust shape. Built-in
adapter execution lives under `runx-runtime` feature flags (`async-http`,
`cli-tool`, `mcp`, `mcp-http-server`, `agent`, `catalog`) until a family has an
independent publishing story. The extension author's stable surface is a tool
manifest and process contract, plus optional helper SDKs; it is not a Rust
crate dependency or a core fork.

There is no umbrella `runx` crate in the initial Rust shape. The `runx` crate
name is already taken by an unrelated crate, and the installable user-facing
surface is `runx-cli` with a binary named `runx`. If the name ever becomes
available or transferred, an umbrella crate can be proposed in a separate spec;
until then consumers depend on the specific crate they need.

### Runtime buckets

The runtime crate is the impure owner, but it should not read as one flat bag
of side effects. Current modules map to these implementation buckets:

- Service construction: `config`, `credentials`, `registry`, `http`,
  `receipts::paths`, `receipts::signing`, and the `RuntimeOptions` entrypoint.
  These modules resolve environment, credentials, registry roots, HTTP clients,
  receipt paths, and signing policy. They should become typed service facets
  that are constructed near runtime entrypoints, not leaf adapters.
- Harness execution: `execution::harness`, `execution::orchestrator`,
  `execution::runner`, and `execution::skill_front`. These modules open the
  governed execution boundary, run graphs or skills, and seal receipts.
- Adapter invocation: `adapter`, `adapters::*`, `agent_invocation`, and
  `process_invocation`. These modules resolve an
  invocation, deliver admitted authority, run or supervise a process/protocol
  call, capture output, and return projected evidence to the harness.
- Receipt and event projection: `receipts`, `journal`, `host`, `redaction`,
  and the receipt tree verifier. These modules own durable evidence and the
  projections that feed history, hosted ingestion, and fixture oracles.
- Authority algebra: `runx-core::policy` plus runtime consumers in
  `execution::runner::authority`, `approval`, effect-family adapters, and
  `credential_grant_policy` tests. Runtime code may record authority evidence,
  but checkable attenuation belongs in typed policy primitives.
- CLI presentation: `runx-cli` owns argument parsing, output, and exit codes.
  It may call runtime services, but should not recreate runtime semantics.
- Dev and testing: `dev`, `doctor`, project initialization, `tool_catalogs`, harness
  fixtures, adapter oracles, throughput scripts, and stress gates. These are
  product surfaces for authors and maintainers, not hidden fallback runtimes.

These buckets are not proposed Cargo crates. They are the internal ownership
map for `runx-rust-runtime-architecture-lift-v1`: split only where the split
creates a clearer dependency or capability boundary.

Current runtime lift shape:

- `runx-runtime::services` owns small service facets for workspace environment,
  receipts, process invocation, and adapter invocation. These facets are passed where
  they are needed; there is no catch-all harness context.
- `runx-runtime::adapter_pipeline` owns the shared adapter lifecycle:
  resolve, admit, invoke, capture, project, and seal. CLI-tool, JavaScript,
  and agent paths use the same lifecycle instead of parallel process stories.
- `runx-runtime::lifecycle` owns internal harness, decision, act, receipt,
  abnormal-seal, verification, and publication events before they project into
  local journal/history rows.
- `runx_runtime::harness::list_cases()` is the single Rust harness replay case
  registry. The oracle binary consumes that registry for check, regeneration,
  and JSON summary output.
- Stress gates are explicit scripts (`stress:runtime:mcp`,
  `stress:runtime:cli-tool`, `stress:runtime:fanout`). They are not hidden inside the default fast loop.

### Volume-independent execution boundary

Payload size does not create a new execution lane or grant more authority. The
runtime uses three transports with different ownership:

- control input is one typed JSON object; the CLI may read that object from a
  contained `--inputs` file or stdin up to 64 MiB, but the selected runner and
  worker retain their normal type and resource limits;
- immutable local content is admitted once by the runtime as a digest-bound
  artifact of at most 512 MiB and consumed through exact pages that default to
  1 MiB and may be configured up to 4 MiB; a deterministic module receives
  records and continuation metadata, not a workspace path or ambient
  filesystem access;
- durable histories stay in the data plane and move through bounded
  `after_version`/keyset cursors or projections, never through an accumulating
  graph-context array.

`WorkspaceFile` is the single runtime owner for relative-path validation,
canonical containment, regular-file checks, bounded reads, streaming digest,
and source-copy detection. `fs.read`, `fs.read_bundle`, structured CLI input,
and local-artifact admission compose that owner rather than maintaining
parallel path resolvers or hashing loops. The artifact service owns immutable
snapshot identity and page framing above it. Package code owns only an
irreducible record transform and cannot select a larger profile, widen a page,
or reopen the source path.

## 4. `runx-core` public API stance

`runx-core` is library-only and not published in this phase. Its public API is
shaped for two consumers:

- Fixture-runner tests (internal to this monorepo).
- Future internal consumers (`runx-parser`, `runx-receipts`,
  `runx-runtime`, `runx-cli`).

Stability rules during the parity phase:

- The public surface is unstable. No SemVer guarantee. The crate version
  stays `0.0.x`.
- Every module is `pub mod` only if a fixture or sibling crate consumes it.
  Internal helpers stay private.
- Re-exports at crate root follow the owned contract shape: one module per
  policy family (`state_machine`, `policy`, `policy::authority_proof`,
  `policy::scope`).
- Naming preserves runx vocabulary. `admit_local_skill` matches
  `admitLocalSkill`. No invented aliases.

Publication to crates.io follows section 14.

## 5. Error and decision model

TypeScript policy returns discriminated decision objects, for example
`{ status: "approved", grant }` or `{ status: "rejected", reason }`.

Rust mirrors this shape with enums, not `Result`:

```rust
pub enum AdmissionDecision {
    Approved(LocalAdmissionGrant),
    Rejected { reason: AdmissionRejectionReason },
}
```

Rationale:

- `Result<Grant, Reason>` would imply rejection is exceptional. In policy code
  it is a normal, expected outcome.
- Fixture JSON encodes both arms uniformly via the discriminator field.
- Callers can `match` exhaustively; new variants are breaking changes that
  surface at compile time.

Rejection reasons are typed enums (`AdmissionRejectionReason`, effect-family
rejection reasons, and similar owners), not free-form strings. The reason value in
fixtures uses the serde-renamed enum variant name.

Panics are forbidden in `runx-core`. The workspace `Cargo.toml` already denies
`clippy::panic` and `clippy::unwrap_used` for the launcher; the same lints
apply to all pure crates.

## 6. Serde conventions

Fixture JSON is the cross-language contract. Conventions:

- All public types derive `serde::Serialize` and `serde::Deserialize`.
- Struct fields use `#[serde(rename_all = "camelCase")]` to match TypeScript
  emit. This is the default for the whole crate.
- Tagged unions use `#[serde(tag = "status")]` or `#[serde(tag = "kind")]`
  matching the discriminator field name from TypeScript. Per-union choice is
  documented next to the type.
- Enum variants without payloads use `#[serde(rename_all = "kebab-case")]` to
  match TS string union values such as `"in-progress"`, `"on-failure"`.
- Optional fields use `Option<T>` with `#[serde(skip_serializing_if = "Option::is_none")]`
  so fixture JSON matches TypeScript's omitted-key behavior.
- No `#[serde(default)]` on required fields. Missing fields are errors, not
  silently zero-valued.
- Deduplicated arrays preserve first-seen insertion order from the TypeScript
  oracle. Rust ports use `Vec` plus insertion-preserving deduplication
  (`IndexSet` or equivalent) for serialized arrays such as `requestedScopes`
  and `grantedScopes`, not `HashSet` or `BTreeSet`.

A single `crates/runx-core/src/serde_conventions.rs` module documents these
rules in code comments and exposes a tiny test that round-trips a few golden
values, so the rules are not just prose.

## 7. Platform-sensitive behavior

The retired TypeScript policy module used Node path semantics for executable
normalization. Node's path module is OS-aware: on Windows it treats `\` as a
separator, on POSIX it does not.

Decision: fixtures use POSIX semantics only. Executable names that contain
backslashes are normalized as if the separator were `/`, regardless of host
OS. Rationale:

- Fixtures are deterministic if and only if path semantics are platform-free.
- Cross-platform `node:path` behavior produces different results for the same
  input string between Windows and POSIX runners; this would make fixtures
  host-dependent.
- The Rust port implements its own `posix_basename` helper rather than using
  `std::path::Path`, which is platform-aware.

If a real runtime consumer ever needs Windows-aware path handling, that lives
in `runx-runtime`, not in `runx-core`.

This also makes strict CLI-tool inline-code admission deterministic across
hosts. A command such as `C:\Tools\node.exe` normalizes to `node` everywhere,
so inline `-e`/`--eval` style invocations are denied consistently instead of
being bypassed on POSIX runners because backslashes were treated as ordinary
filename characters.

A TypeScript-side change is in scope for the fixtures spec: replace the
`node:path` import with a small `posixBasename` helper. This makes the kernel
truly side-effect-free (it currently imports a node-only module) and aligns
both languages on the same semantics. Flag for review.

## 8. Standard library posture

`runx-core` uses `std` by default and does not gate `no_std` from day one.

Rationale:

- No concrete consumer needs `no_std` today.
- `serde_json` with `no_std` requires `alloc` and a specific feature dance
  that adds friction to every dependency add. The kernel is small (<2,000
  lines once ported); retrofitting `no_std` later if a real embedded or
  embedded-WASM consumer materializes is a half-day of work.
- WASM works fine with `std`. The hypothetical "in-browser preview" path
  does not require `no_std`.

If a kernel-adjacent crate (`runx-receipts` for signing helpers in embedded
contexts, for example) ever ships a real `no_std` requirement, that decision
is revisited as a follow-up spec; it does not constrain this plan.

## 9. MSRV and edition

- Edition: 2024.
- Resolver: 3.
- MSRV: 1.97.0, expressed by the `crates/Cargo.toml` workspace
  `[workspace.package]` block.
- Repository, CI, and release builds use the single root `rust-toolchain.toml`
  pin. That reproducible build toolchain is deliberately separate from the
  oldest compiler version the public crates support.
- MSRV bumps are spec-level changes, not silent dependency updates.

## 10. Rust-side boundary enforcement

The TypeScript boundary script ([scripts/check-boundaries.mjs](../scripts/check-boundaries.mjs))
forbids node APIs from `policy` and `state-machine` packages. The Rust side
needs the same discipline. Layered enforcement:

1. **Dependency direction**: `runx-core/Cargo.toml` lists only
   `runx-contracts`, `serde`, `serde_json`, and narrow test-only tools.
   `serde_json` is allowed for private deterministic JSON canonicalization,
   but public APIs expose `runx_contracts::JsonValue`, not
   `serde_json::Value`. `runx-parser` also exposes parser raw object subtrees
   as `runx_contracts::JsonValue` and validates execution metadata into the
   `runx_contracts::execution` types. No `tokio`, `reqwest`, `hyper`, `clap`,
   `rmcp`.
   Enforced by dependency direction and by the workspace `cargo-deny`
   default ban. The ban is intentionally stricter than "pure crates only"
   because `cargo-deny` is workspace-scoped in this track; adapter/runtime-tier
   exceptions must be introduced by an explicit spec before the dependency is
   removed from `deny.toml`.

2. **API surface lint**: `cargo-public-api` snapshots the public API. A diff
   against the snapshot in CI flags accidental surface growth.

3. **Forbidden imports**: a lightweight build-time check (a `build.rs` is
   overkill; a CI step running `cargo +nightly rustdoc -Z unstable-options`
   or a simple grep over the crate's compiled deps tree) ensures
   `std::process`, `std::fs`, `std::net`, `std::time::SystemTime`,
   `std::env` are not referenced from `runx-core/src`. The grep is fragile
   but acceptable as a defense-in-depth signal alongside cargo-deny.

4. **Boundary check in TS**: continues to apply. The existing
   `pnpm boundary:check` is the source of truth for TS-side enforcement; the
   Rust checks above mirror it on the Rust side.

`cargo-deny` configuration lives at `oss/crates/deny.toml` and is referenced
from CI.

## 11. Property and differential testing

Fixture-only testing covers known cases. For state-machine logic with several
enums and fanout sync semantics, the long tail is large.

Plan:

- Phase 1 (fixtures spec): checked-in fixtures only. Enough to prove the
  contract works.
- Phase 2 (state-machine spec): add `proptest` strategies for graph state
  transitions. Same generated inputs run through both languages via a
  TypeScript subprocess invoked from a Rust integration test, or vice versa.
  Differential failures pin a counterexample as a new fixture.
- Phase 3 (policy spec): proptest strategies for admission and scope
  narrowing inputs.

Differential testing is optional in early phases but is the only realistic
way to catch parity drift on combinatorial state spaces. Each parity spec
declares whether it adopts it.

## 12. Dual-tree migration record

Before cutover, parity was staged:

- **Phase A (advisory)**: Rust parity runs in CI through
  `scripts/check-rust-kernel-parity.mjs`, but failure is a warning.
  TypeScript developers can break fixtures and regenerate them. A failing
  Rust check produces a CI annotation but does not block merge. The local
  command is `pnpm rust:check`.
- **Phase B (blocking)**: After 5 clean kernel-touching PRs land green in
  Phase A, Rust parity blocks merge. Every PR that touches
  retired state-machine or policy fixture behavior must either pass Rust parity
  or include an intentional fixture refresh (regenerated via the parity script).

Calendar time is not the trigger. If the kernel doesn't churn for weeks, the
soak proves nothing; if it churns daily, calendar time is too coarse. PR
count maps to actual exposure.

Expected cost after promotion: roughly +4 to +8 hours per kernel-touching
PR for the dual-tree update (TS change, fixture regen, Rust port, Rust
clippy/proptest update, public-API snapshot bump). Budget this explicitly;
do not pretend it is free.

This staged policy is historical. The Rust workspace is now blocking in CI and
release validation, and retired TypeScript runtime-local surfaces are not
maintained as a second implementation.

### TS sunset trigger

The dual tree is not the destination. The gating oracle for retiring
TypeScript implementations is `rust-cli-feature-parity-matrix`. Once that
matrix passes against a Rust runtime candidate, the cutover is triggered.

Sunset order (each step is its own cutover spec, not implicit):

1. Replace TS state-machine consumers with `runx-core::state_machine`.
   Completed; the TypeScript state-machine subtree is retired.
2. Replace TS policy consumers with `runx-core::policy`.
   Completed; the TypeScript policy subtree is retired.
3. Port and delete `parser`, `executor`, `marketplaces` (pure-by-imports
   trusted-kernel domains).
4. Port impure trusted-kernel domains (`artifacts`, `config`, `knowledge`,
   `receipts`, `registry`) and delete the TypeScript runtime-local/adapters
   fallback. Each trusted local domain requires TS-side purification or a
   pure/impure split first. Tools remain supported through their
   language-neutral manifests and process contracts, not through TypeScript
   executor internals.
5. Keep npm `@runxhq/cli` as a platform-aware selector that resolves and execs
   the Rust binary without requiring TypeScript source or tsx at runtime.
6. Move `runx-sdk` from CLI-backed mode to `native-runtime` once the runtime
   cutover is complete. Until then, the SDK remains a Rust client over the
   authoritative CLI and shared `runx-contracts` types. TypeScript helper SDKs
   may remain as clients over CLI JSON, generated contracts, cloud HTTP, or
   ratified language-neutral protocol lanes.

## 13. CLI cutover position

`crates/runx-cli` is now the native local command surface for the Rust-backed
commands it advertises. Help text is a contract: if a form appears in
`runx --help`, it must either execute through Rust or fail closed before any
hidden TypeScript fallback.

The npm `@runxhq/cli` package remains a platform-aware launcher/client wrapper,
but local execution semantics move through `runx-runtime`, not through
`@runxhq/runtime-local` or `@runxhq/adapters`. Installed package usage must be
useful without a checked-out TypeScript workspace. If the native binary is
missing or unsupported, the launcher fails closed with installation guidance;
it must not import a hidden TypeScript local runtime fallback.

New native command forms still require parity evidence:

- `fixtures/cli-parity` exists and covers every current command, subcommand,
  flag, exit code, JSON output shape, human-output promise, receipt behavior,
  execution-boundary evidence, adapter path, and documented workflow.
- `runx-core`, `runx-parser`, and `runx-receipts` exist and pass parity.
- A `runx-runtime` crate exists with at least one impure adapter execution path
  ported.
- A command-specific cutover or hardening spec proposes the move.

Kernel parity is not CLI parity. A Rust state-machine or policy port can prove
that pure decisions match TypeScript, but it does not prove that the executable
CLI is a drop-in replacement. Native CLI candidates must run against the
fixture matrix and pass one-to-one feature parity before npm wrappers may
present them as canonical.

The one-to-one CLI matrix belongs in `fixtures/cli-parity/` and is governed by
the `rust-cli-feature-parity-matrix` spec. The matrix is intentionally broader
than kernel parity. It derives its command inventory from native CLI help and
binds each advertised command to an explicit parity annotation and executable
case. Do not maintain a second handwritten command inventory here. Payment
surfaces use clean v1 names only. `spend` is the canonical buyer contract;
`charge`,
`refund`, and `settle-invoice` are the other canonical contracts, while
`x402-pay` and `stripe-pay` are discoverable hosted facades. Their OSS graphs
use the generic provider mutation/readback boundary. Quote, reservation, rail
SDKs, custody, ledgers, recovery, and finality are private hosted concerns.
`mock-charge`, `mock-pay`, and `mock-refund` are internal local simulators that
always state that no money moved. Legacy payment aliases and local lifecycle
stage graphs are not restored.

## 14. Package publishing ownership

Cargo manifests and the workspace lockfile are the only version and
publishability source of truth. Do not duplicate live crate versions in
architecture prose. The release workflow may publish only the crates and
artifacts explicitly selected by the current release profile, in workspace
dependency order.

## 15. Path-inconsistency note

The repo-root `docs/trusted-kernel-package-truth.md` is a superseded migration
record. The active OSS ownership boundary is
`docs/ts-interop-boundary.md`; `docs/trusted-kernel-package-truth.md` summarizes
its Rust package implications.

## 16. Open questions intentionally deferred

- Whether to adopt `bon` or another builder crate for the larger value types.
- Whether to expose a C ABI for non-Rust hosts (likely no, but not decided).
- Whether the Rust port targets a single big crate or splits state-machine
  and policy into their own crates from day one. Current decision: single
  crate. Split is a follow-up if the public surface grows too large.
- Whether a `runx-macros` crate is ever justified. There is no macros crate
  placeholder now. Procedural macros need a separate spec because they add
  build complexity and are easy to overuse.

## 17. Current Rust Kernel Status

For advertised native runtime and CLI paths, Rust owns local parsing, policy,
authority, configuration, graph execution, receipt, registry, tool, adapter,
and process-supervision decisions. `crates/runx-contracts` carries the
domain-neutral portable wire contracts and shared schema mechanics; domain
crates contribute their own contract artifacts. Checked-in fixtures are
conformance evidence, not an alternate implementation.
Tool authors target language-neutral manifests and process contracts and do not
need Rust or a core fork to add integrations.

## 18. Rust implementation quality bar

The Rust port must read like Rust, not like TypeScript mechanically translated
into Rust syntax. For still-dual surfaces, the source of truth for behavior is
the approved parity fixture or TypeScript oracle named by the active spec. For
cut-over local surfaces, Rust is the behavioral source of truth. Rust's own API
and style conventions own the implementation shape in either case.
Fixture and wire parity are mandatory where the spec requires them; internal
names, module boundaries, and helper structure should actively improve when the
TypeScript shape is awkward or less idiomatic in Rust.

Code shape rules:

- Use Rust 2024 idioms: `let else`, `matches!`, `is_some_and`,
  `Option::then_some`, exhaustive `match`, and iterator combinators where they
  simplify control flow. Do not write clever iterator pipelines when a short
  loop is clearer.
- Follow Rust API Guidelines naming: modules, functions, and values use
  `snake_case`; types, traits, and enum variants use `UpperCamelCase`; error
  types use verb-object-error naming when an error type is needed.
- Public APIs preserve runx vocabulary but not TypeScript casing:
  `admitLocalSkill` becomes `admit_local_skill`, not a generated alias.
- Prefer small value types, enums, and `match` over stringly typed records.
  `serde_json::Value` is allowed in fixture tests, but not in the public
  `runx-core` API.
- Use `BTreeMap` for serialized maps whose key order reaches fixture JSON.
  Do not use `HashMap` in `runx-core` unless the value never crosses a
  serialization boundary and the spec explains why.
- Keep helpers private by default. A function becomes `pub` only when a
  fixture runner or sibling crate needs it. Do not use `pub use *`.
- Avoid macro-heavy abstractions. `derive` is fine; bespoke macros require a
  spec-level justification.
- Avoid builder crates and fluent builders in `runx-core` unless a type has a
  real optional-field explosion. Plain structs and constructors are preferred.
- Avoid clone-driven design. Small enums can derive `Copy`; larger values are
  borrowed by slice/reference where straightforward. Cloning at fixture or
  serde boundaries is acceptable.
- Keep modules scoped. A Rust source file above roughly 350 lines or a
  function above roughly 60 logical lines needs a short comment in the spec
  receipt explaining why it is still the clearest shape.

Error and failure rules:

- No `unsafe`, `panic!`, `todo!`, `unimplemented!`, `dbg!`, `unwrap`, or
  `expect` in `runx-core` production code.
- No `anyhow`, `eyre`, `Box<dyn Error>`, or dynamic error erasure in
  `runx-core` public APIs. Policy decisions are normal enum values, not
  errors. Actual validation errors use concrete enums or structs, with
  `thiserror` preferred when deriving `Display` and `std::error::Error` keeps
  the implementation smaller and clearer.
- Tests may return `Result` and use `?`; fixture loaders should not rely on
  `unwrap` or `expect`.

Async and blocking rules:

- `runx-contracts`, `runx-core`, `runx-parser`, and `runx-receipts` do not
  depend on `tokio`, `async-trait`, HTTP clients, or subprocess libraries.
- `runx-runtime` owns process management, network IO, MCP, permission delivery,
  built-in adapter execution, and adapter concurrency. It may own an async
  runtime or MCP/HTTP protocol crate only after a spec records the security
  rationale and updates `crates/deny.toml`; until then the workspace ban is
  deliberate.
- Process ownership is kernel-backed before blocking external child code can
  run: Unix executions start in a dedicated process group; Windows executions
  start suspended, join a per-execution Job Object, and only then resume. Every
  Windows adapter spawn is also beneath an outer Runx kill-on-close job so
  abrupt CLI termination cannot orphan synchronous, worker, or MCP processes.
  PID snapshots, `taskkill`, PowerShell tree walks, and skill-local process
  wrappers are not containment mechanisms.
- `runx-runtime` defaults to no adapter features. Built-in protocol host
  families are opt-in: `async-http`, `cli-tool`, `mcp`, `mcp-http-server`,
  `agent`, and `catalog`.
- `runx-sdk` v0 is explicitly a blocking CLI-backed client and depends on
  `runx-contracts`, not `runx-core` or `runx-runtime`. A future async SDK path
  requires its own spec and contract fixtures.
- TypeScript helper SDKs may exist outside the trusted runtime as clients over
  generated contracts, CLI JSON, cloud HTTP, or ratified language-neutral
  protocol lanes. They must not embed a trusted local executor.
- `runx-cli` may bridge into the async runtime once it is native, but must not
  bypass `runx-runtime` by calling pure crates directly for runtime behavior.

Workspace policy:

- Commit the single workspace lockfile at `crates/Cargo.lock`. This workspace
  contains the `runx-cli` binary plus publishable library crates, so the lock
  file is part of reproducible CI.
- `cargo-nextest` runs the workspace integration suite in CI; doctests run
  separately because nextest does not execute them.

Enforcement:

- `cargo fmt --all --check` is required.
- `cargo clippy -p runx-core --all-targets -- -D warnings` is required.
- Rust semantic rules are compiler-owned: workspace lints and Clippy reject
  panic/debug shortcuts, wildcard imports, unsafe code, and warning drift.
  Contract and parser fixture coverage is owned by the native generators and
  Rust tests that execute those contracts; JavaScript does not parse Rust
  source to infer semantics or coverage.
- `scripts/check-rust-crate-graph.mjs` checks crate membership, publishability,
  and dependency direction. Dependency relaxation is an architecture change.
- `cargo-public-api` snapshots ensure "just make it pub" does not become the
  easy escape hatch.

This bar deliberately avoids `clippy::pedantic` as a global deny. High-signal
lints are required; style churn is not.

## References

- [docs/trusted-kernel-package-truth.md](trusted-kernel-package-truth.md)
  (repo-root docs)
- [oss/scripts/check-boundaries.mjs](../scripts/check-boundaries.mjs)
- [oss/crates/runx-core/src/state_machine.rs](../crates/runx-core/src/state_machine.rs)
- [oss/crates/runx-core/src/policy](../crates/runx-core/src/policy)
- [oss/crates/runx-cli/src/main.rs](../crates/runx-cli/src/main.rs)
