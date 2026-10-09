# Runtime Throughput

This note defines the OSS runtime performance contract. The target is throughput on runx-controlled overhead: graph planning,
context and output projection, receipt sealing, receipt
store maintenance, and native CLI launch overhead. It does not claim speedups
for external LLMs, network APIs, or user subprocess work.

## Baseline

Capture the local baseline before hot-path changes:

```bash
pnpm perf:runtime:capture -- --output .scafld/perf/oss-runtime-throughput-baseline.json
```

The capture script runs the Rust Criterion benches and records a JSON document
using schema `runx.oss_runtime_throughput.v1`. The JSON stores workload
throughput in iterations per second, per-sample p95/p99 latency, sample count,
the release build/toolchain, hardware identity, the source commit, and a digest
of the exact dirty or clean source tree that produced the result. A dirty-tree
capture is therefore explicit and still content-bound; it is never presented as
evidence for the unmodified commit alone.

## Benchmarks

Rust runtime workloads live in
`crates/runx-runtime/benches/graph_throughput.rs`:

- `graph_planning`
- `graph_receipt_sealing`
- `receipt_store_append`
- `receipt_store_index`
- `native_capability_dispatch`
- `graph_context_to_module`
- `pure_module_cold_start`
- `pure_module_session_reuse`
- `pure_module_large_input`
- `provider_effect_finality`
- `artifact_admission`
- `artifact_page_continuation`
- `twitter_archive_selection`

The runtime-path workloads invoke production APIs rather than benchmark-only
reimplementations. `native_capability_dispatch` runs `data.digest` as a
one-step graph: the path every graph tool step and agent turn call takes,
through admission, dispatch and receipt sealing. The workloads also cover
graph context flowing from a native output into a deterministic module; fresh
and reused module adapters; a near-4 MiB JSON input; and provider-permission
admission through receipt sealing without contacting a live provider.

`provider_effect_finality` is the production authority-to-finality workload. It
executes a connector write that explicitly asks for human approval,
then covers durable attempt state,
provider acknowledgement, identity-bound readback, finality proof, and receipt
sealing. Its first capture is part of the production path baseline shared by
newly introduced workloads.

There are intentionally no standalone `context_projection` or
`output_projection` microbenchmarks. The former implementations were local
benchmark helpers, not runtime code, so optimizing them could not improve the
product. `graph_context_to_module` exercises both production context
materialization and step-output projection through the real graph runner.

The module rows exercise the production Boa worker. `pure_module_cold_start`
creates and retires a real worker session per measured invocation;
`pure_module_session_reuse` invokes a pre-warmed session. A runtime session owns
a lazy pool capped at four worker processes. Sequential work reuses one warm
process; concurrent work acquires separate processes so a timed-out invocation
cannot kill healthy siblings. Each process serves one invocation at a time and
each invocation receives a fresh engine context. The module rows record the
actual worker spawn count and peak active leases in the capture sidecar.

The receipt append scale points measure one new receipt appended after 16 and
128 existing receipts. That detects accidental history-wide reparsing in the
hot path. The index scale points separately measure explicit recovery rebuilds,
which are expected to inspect the bounded receipt set.

The volume paths also run production owners rather than benchmark parsers or
direct database shortcuts. Artifact admission snapshots and hashes an 8 MiB
workspace file through `artifact.admit`; continuation reads it through
`artifact.read`. Twitter selection
loads the checked-in Twitter skill package and runs its real
`selectArchivePage` export over a workspace archive larger than 8 MiB through
the paged JavaScript adapter. Small/large scale points are 256 KiB/8 MiB for
artifacts, 100/1,000 events, and 1,500/12,000 Twitter records. Their growth
metrics detect a return to whole-input copies or history-wide state.

Receipt canonicalization workloads live in
`crates/runx-receipts/benches/receipt_canonicalization.rs`:

- `receipt_canonicalization`
- `receipt_body_json`
- `receipt_full_json`

Process workloads are orchestrated by `scripts/runtime-throughput.mjs` because
they measure process launch overhead rather than Criterion benches:

- `native_cli_launch`
- `cli_file_input`

These rows include `spawn_count`. The native launch gate requires
`spawn_count <= 1` and no p99 regression above the declared budget.
Deterministic JavaScript owns a runtime-scoped bounded worker pool with a fresh
engine context per invocation. The pool ceiling is four processes. Each process
has a 160 MiB working-set ceiling, so the explicit aggregate JavaScript
working-set budget is 640 MiB; the contract exposes both values rather than
hiding the multiplication behind an invocation-count constant.

Process rows are measured from release binaries built in
`crates/target/runx-perf/release`. The perf harness intentionally does not reuse
`crates/target/debug/runx`, because that binary may be stale or built from a
different local checkout state. Each capture asks Cargo to refresh those release
probe binaries before measuring so an existing perf artifact cannot silently
stand in for the current checkout. The native launch row performs three unmeasured
warm-up launches before collecting samples so p99 gates track steady local launch
overhead rather than first-touch page-cache noise.

`cli_file_input` creates a valid temporary skill, invokes the release binary
through the canonical `runx skill ... --inputs input.json` surface, executes a
native digest, and seals the ordinary local receipt. It therefore measures the
real contained document reader and selected-runner path; it is not the separate
`kernel eval --input` command dressed up as skill-input evidence.

## Fanout Execution

Fanout defaults to the host's available parallelism, capped at 64. Set
`RUNX_MAX_FANOUT_CONCURRENCY` in `RuntimeOptions.env` or the process environment
to restrict that ceiling. The runtime uses parallel capacity only for isolated
JavaScript or explicitly isolated adapter lanes. Native run steps, tool and
host resolution, effect-authority inputs, and adapters without isolation stay
serial.

## Runtime Boundaries

The hot-path runtime changes keep ownership narrow:

- `runx-core` remains the pure decision layer for graph planning, scope
  admission, credential binding, and authority proof metadata.
- `runx-runtime` owns mutable execution indexes, step dispatch, subprocess
  supervision, receipt linking, receipt store indexing, and journal projection.
- `runx-receipts` owns canonical byte output, body/full digesting, proof
  verification, and receipt tree resolution.
- TypeScript packages remain generated contracts, host/client wrappers,
  language-neutral extension helpers, and cloud/product code. Native authoring
  plus Skill Lab own skill creation; deleted authoring and executor packages do
  not remain as runtime bridges.
- Arbitrary CLI and user subprocesses are not pooled.

The shared Rust process supervisor is intentionally private to
`runx-runtime`. It owns only process lifecycle mechanics: environment/cwd
application, stdin writing, bounded stdout/stderr capture, timeout signaling,
process-group cleanup, duration, and owned temporary-path cleanup. Adapter-specific
policy, redaction, protocol parsing, and receipt projection stay in their
adapter modules.

Containment has no extra process or shell hop. Unix uses the existing process
group primitive. Windows alone links the safe Job Object wrappers needed to
create a child suspended, assign it before execution, terminate the whole tree,
and reap it when Runx exits abruptly. The catalog sweep only enforces the outer
deadline; it does not duplicate runtime ownership with `taskkill`, PowerShell,
or a JavaScript anchor.

## Limits

The 2x gate applies to deterministic graph planning state-machine overhead. Production context-to-module execution has a no-regression gate;
receipt canonicalization and store maintenance use a 1.75x throughput gate
plus a measured growth-shape budget. Session gates track spawn count and p99
regression. Runx does not emit allocation metrics until the harness has a real
production-path allocator observation; an invented zero is not evidence. The
growth-exponent gate applies only where the harness captures explicit small and
large scale points: receipt-store append/index, artifact continuation, and
Twitter archive selection. Admission, canonicalization, and
sealing have throughput gates but no invented scaling metric. These gates do
not claim an end-to-end speedup when wall time
is dominated by external models, remote APIs, user subprocess work, package
manager startup, or operating-system process startup.

New production-path workloads compare repeat captures against
`.scafld/perf/oss-runtime-production-path-baseline.json`, require at least 85%
of baseline throughput, and permit at most 25% p99 variance. Provider finality
requires zero child-process spawns, and native artifact work holds no worker
session at all; the canonical CLI input path permits one CLI process. The wider tolerance reflects real durable
filesystem and process variance; it does not excuse removing attempt
persistence, bounded continuation, readback, finality, or receipt sealing from
the measured paths.

## Gates

`.github/workflows/runtime-quality.yml` owns the automated heavy lane. It runs
weekly, on explicit dispatch, and as a release prerequisite. The lane captures
`native_capability_dispatch` and `pure_module_session_reuse` from the exact
checked-out commit, then enforces one-process serial reuse. The digest-bound
JSON report is uploaded
before enforcement so a failed check remains diagnosable; it is not checked
into the repository.

Timing regressions must be compared on like hardware. The workflow therefore
does not pretend that a developer-machine baseline is valid on a hosted runner.
For a hot-path change, compare the exact candidate report with a baseline
captured on the same host class using the commands below.

Later phases compare against the Phase 1 baseline:

```bash
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-throughput-baseline.json --workloads graph_planning --min-throughput-ratio 2.00
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-throughput-baseline.json --workloads graph_context_to_module --min-throughput-ratio 1.00 --max-p99-regression 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-throughput-baseline.json --workloads receipt_canonicalization,graph_receipt_sealing --min-throughput-ratio 1.50
```

The check command exits non-zero when any requested workload misses its declared
throughput ratio.

The S-tier final gate captures all runtime-owned workloads into
`.scafld/perf/oss-runtime-s-tier-final.json` and compares them against
`.scafld/perf/oss-runtime-s-tier-baseline.json`:

```bash
pnpm perf:runtime:capture -- --output .scafld/perf/oss-runtime-s-tier-final.json --workloads graph_planning,native_capability_dispatch,graph_context_to_module,pure_module_cold_start,pure_module_session_reuse,pure_module_large_input,provider_effect_finality,artifact_admission,artifact_page_continuation,twitter_archive_selection,receipt_canonicalization,graph_receipt_sealing,receipt_store_append,receipt_store_index,native_cli_launch,cli_file_input
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads graph_planning --min-throughput-ratio 2.00 --max-p99-regression 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads native_capability_dispatch,graph_context_to_module --min-throughput-ratio 1.00 --max-p99-regression 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads receipt_canonicalization,graph_receipt_sealing --min-throughput-ratio 1.75
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads receipt_store_append,receipt_store_index --min-throughput-ratio 1.75 --max-growth-exponent 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads provider_effect_finality --min-throughput-ratio 0.85 --max-p99-regression 1.25 --max-spawn-count 0
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads artifact_admission --min-throughput-ratio 0.85 --max-p99-regression 1.25
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads artifact_page_continuation,twitter_archive_selection --min-throughput-ratio 0.85 --max-growth-exponent 1.10 --max-p99-regression 1.25
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads native_cli_launch --max-spawn-count 1 --max-p99-regression 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads cli_file_input --min-throughput-ratio 0.85 --max-p99-regression 1.25 --max-spawn-count 1
```

## Evidence ownership

Generated performance reports are the sole owner of measured values, hardware,
toolchain, source commit, tree digest, sample count, p50/p95/p99, throughput,
growth, and process metrics. This document defines workloads and gates; it does
not copy generated numbers into a second surface that can silently drift after
the next capture. Release notes and articles may quote a named, digest-bound
report, but must not present those figures as current after its source digest
changes.

The hostile-module suite remains a separate release gate. It exercises the
runtime-owned 4 MiB source/input/output ceilings, 64 MiB aggregate JavaScript
heap, 4 MiB JavaScript stack, two-second default and 30-second maximum wall
limit, 4,096-job limit, and each platform's process-tree lifecycle controls. Linux
additionally permits 1 GiB of virtual
address space so glibc's uncommitted per-thread arenas do not consume the real
heap budget; Windows retains a 160 MiB working-set ceiling. Virtual address
space and committed working memory are deliberately not presented as the same
limit.
