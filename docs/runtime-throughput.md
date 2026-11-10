# Runtime Throughput

This note defines the OSS runtime performance contract. The target is throughput on runx-controlled overhead: workflow planning,
context and output projection, receipt sealing, receipt
store maintenance, and native CLI launch overhead. It does not claim speedups
for external LLMs, network APIs, or user subprocess work.

## Baseline

Capture the local baseline before hot-path changes:

```bash
pnpm perf:runtime:capture -- --output .scafld/perf/oss-runtime-throughput-baseline.json
```

The capture script runs the Rust Criterion benches and the process probes and
records a JSON document using schema `runx.oss_runtime_throughput.v1`. A
Criterion workload is a CPU benchmark, recorded as Criterion reports it: the
mean per iteration, the mean's confidence interval, its throughput in iterations
per second and its sample count. Criterion's samples are batch averages, so no
percentile is ever taken from them. A request workload is timed request by
request at a held arrival rate, and records its p50, p95 and p99 from those
individual timings, with how many requests completed, failed and timed out. The
JSON also stores the release build/toolchain, hardware identity, the source
commit, each journey's structural measurements, and a digest of the exact dirty
or clean source tree that produced the result. A dirty-tree
capture is therefore explicit and still content-bound; it is never presented as
evidence for the unmodified commit alone.

## Benchmarks

Rust runtime workloads live in
`crates/runx-runtime/benches/workflow_throughput.rs`:

- `workflow_planning`
- `workflow_receipt_sealing`
- `receipt_store_append`
- `receipt_store_index`
- `native_capability_dispatch`
- `workflow_context_to_module`
- `pure_module_cold_start`
- `pure_module_session_reuse`
- `pure_module_large_input`
- `provider_effect_finality`
- `artifact_admission`
- `artifact_page_continuation`
- `twitter_archive_selection`

The runtime-path workloads invoke production APIs rather than benchmark-only
reimplementations. `native_capability_dispatch` runs `data.digest` as a
one-step workflow: the path every workflow tool step and agent turn call takes,
through admission, dispatch and receipt sealing. The workloads also cover
workflow context flowing from a native output into a deterministic module; fresh
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
product. `workflow_context_to_module` exercises both production context
materialization and step-output projection through the real workflow engine.

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

They're request workloads. Each request is due at a held arrival rate, 10 a
second unless `--arrival-rate` says otherwise, one at a time, and its latency
runs from when it was due to when it ended. A request that runs past its slot
delays the next, and the next one's latency counts that wait, so a stall shows
in every request it holds up instead of averaging away. Failed and timed-out
requests are counted, and a request workload with any is refused. These rows
record `spawn_count`: the child processes runx spawned for one request, as the
runtime counts them at its shared spawn point and writes them where
`RUNX_PERF_COUNTS_PATH` names, read from the untimed warm-up requests so the
timed ones carry no counting. A launch and a native digest spawn none, so their
gates require `spawn_count` 0 and no p99 regression above the declared budget.
Deterministic JavaScript owns a runtime-scoped bounded worker pool with a fresh
engine context per invocation. The pool ceiling is four processes. Each process
has a 160 MiB working-set ceiling, so the explicit aggregate JavaScript
working-set budget is 640 MiB; the contract exposes both values rather than
hiding the multiplication behind an invocation-count constant.

Process rows are measured from release binaries built in
`crates/target/runx-perf/release`. The perf test intentionally does not reuse
`crates/target/debug/runx`, because that binary may be stale or built from a
different local checkout state. Each capture asks Cargo to refresh those release
probe binaries before measuring so an existing perf artifact cannot silently
stand in for the current checkout. The native launch row performs three unmeasured
warm-up launches before collecting samples so p99 gates track steady local launch
overhead rather than first-touch page-cache noise.

`cli_file_input` creates a valid temporary skill, invokes the release binary
through the canonical `runx skill ... --inputs input.json` surface, executes a
native digest, and seals the ordinary local receipt. It therefore measures the
real contained document reader and selected-method path; it is not the
development kernel evaluator dressed up as skill-input evidence.

## Runtime Boundaries

The hot-path runtime changes keep ownership narrow:

- `runx-core` remains the pure decision layer for workflow planning, scope
  matching, and authority derivation.
- `runx-runtime` owns mutable execution indexes, step dispatch, subprocess
  supervision, receipt linking, receipt store indexing, and journal projection.
- `runx-receipts` owns canonical byte output, body/full digesting, proof
  verification, and receipt tree resolution.
- TypeScript packages remain generated contracts, host/client wrappers,
  language-neutral extension helpers, and cloud/product code. Native authoring
  plus `skill-build` own skill creation; deleted authoring and executor packages do
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

The 2x gate applies to deterministic workflow planning state-machine overhead. Production context-to-module execution has a no-regression gate;
receipt canonicalization and store maintenance use a 1.75x throughput gate
plus a measured growth-shape budget. Session gates track spawn count and peak
leases. Runx does not emit allocation metrics until the test has a real
production-path allocator observation; an invented zero is not evidence. The
growth-exponent gate applies only where the test captures explicit small and
large scale points: receipt-store append/index, artifact continuation, and
Twitter archive selection. Admission, canonicalization, and
sealing have throughput gates but no invented scaling metric. These gates do
not claim an end-to-end speedup when wall time
is dominated by external models, remote APIs, user subprocess work, package
manager startup, or operating-system process startup.

New production-path workloads compare repeat captures against
`.scafld/perf/oss-runtime-production-path-baseline.json` and require at least
85% of baseline throughput; a request workload among them permits at most 25%
p99 variance. Provider finality
requires zero child-process spawns, and native artifact work holds no worker
session at all; the canonical CLI input path spawns no process. The wider tolerance reflects real durable
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
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-throughput-baseline.json --workloads workflow_planning --min-throughput-ratio 2.00
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-throughput-baseline.json --workloads workflow_context_to_module --min-throughput-ratio 1.00
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-throughput-baseline.json --workloads receipt_canonicalization,workflow_receipt_sealing --min-throughput-ratio 1.50
```

The check command exits non-zero when any requested workload misses a bound
it was given. A Criterion workload is bounded by its mean, through
`--min-throughput-ratio`, and by `--max-growth-exponent` where it has scale
points. A request workload is bounded by `--max-p99-regression` and
`--max-spawn-count`. A bound that doesn't apply to a workload's kind fails that
workload, so a check never passes because a figure is absent.

`verify-quality` also holds each of performance.md's eight journeys to the
work budget its row states, from the structural measurements the capture's
`journeys` holds for it: store writes per call, settlement cost at call 1,000,
queries per page, bytes copied, and the rest. These are checked apart from
timing, since they hold at any speed. A journey with no measurement fails, and
so does any budget whose measurement is missing, so the gate fails every
journey whose owner hasn't added its probe yet.

A probe counts what the runtime counted, never what it timed. The runtime
counts its work as it does it (`runx_runtime::work_counts`): the processes it
spawns, its network requests, the package sources it assembles and verifies,
by identity, the kernel store's committed writes and the rows it reads, the
receipt directory entries it lists and the receipt index entries it reads and
writes, the operations it dispatches to providers, and whether a store
transaction was open across one, each time it takes a run whole, replaying
the run's record or sealing every call into its receipt, an artifact's bytes
at each point they're copied: into the artifact store, out of it, to a
worker, and into a page a read returns, and the rows a key listing visits on
`private`, the ones it passes over among them, the pages of a search, and
the events read from a key's history. A listing steps the recency index
itself and keeps the types it was asked for, so the rows it visits are the
rows it counts. A CLI process writes these where `RUNX_PERF_COUNTS_PATH`
names at exit, a bench reads them in process, and a unit test reads them in a
process of its test binary that runs that test alone.

- **Journey 1, cold start.** The capture launches `runx skill inspect` on one
  skill in workspaces of 1, 10, 100 and 1,000 alike packages, each a new
  process after one discarded launch. It records the processes started (the
  launch and each child it spawned), the network requests, the most times one
  package identity was verified, and the package sources read at each size,
  which must be the same at every size.
- **Journey 2, warm execution.** The runtime bench runs 1, 10, 100 and 1,000
  provider calls in one run on its warm provider runtime, each call an
  operation over the local transport, and checks every call was sent and
  dispatched. A call's store writes are the run's writes past the one-call
  run's, over its calls past the first, so a constant cost reads the same at
  every size. The bench records that at 10, 100 and 1,000 calls, the most
  times per call a call took the run whole, and the provider calls a
  transaction was held open across, with each run's raw counts beside them.
- **Journey 3, composition.** The runtime bench runs a script that calls a
  native leaf skill 1, 10, 100 and 1,000 times, one after another, and the
  same at the bottom of scripts nested 1, 3 and 5 deep, once and ten times.
  A call's record reads (the rows the kernel store reads), store
  writes and runs taken whole are a run's past the smaller run's, over the
  calls between them, which must be the same at 10, 100 and 1,000 children
  and at depth 1, 3 and 5. A run of one child whose provider read waits on a
  gate, beside 15 native children, has a host that opens the gate only once
  every other child has sealed: the driver hands a call's reports to its host
  and records it in one step, one call at a time, so a sibling handed back
  after the slow child was delayed by it, and none may be. A run under a
  `max_concurrency` of 8 calls 2 child scripts at once, each declaring a
  concurrency of 8 and making 8 provider calls at once, and each waits until
  8 are in flight: the most in flight is the run's peak, which must reach its
  `max_concurrency`, as nested scripts keep their concurrency, and must not
  pass it, as they draw from the run's.
- **Journey 4, long runs.** The runtime bench makes runs of 10, 100 and
  1,000 calls and children as a caller does, each in a workspace of its own:
  a script's tool calls, its skill calls, each a child run, and the calls a
  host makes through its turn session at a turn whose first call waits on a
  person. Each run then holds on a person. It records the store rows a read
  from the cursor at the run's end reads, those a resume reads to where the
  run holds again, and at the turn those its last call read, which journey
  3's budget reads, at each size. A resume binds the run from its record,
  which is its recovery, and the bench records the most times it read any one
  event of the run's streams. At a turn of each size, a
  read of the fixture chat connector is held in a transport the bench
  controls until it has reached it and another thread has cancelled it, and
  the bench records the rows the cancelled call read. It
  appends a receipt to stores of 16, 128 and 1,000, and records the
  directory entries it listed and the index entries it read and wrote.
- **Journey 5, memory.** The runtime bench lays down a personal memory of
  100, 10,000 and 100,000 entries, each in a fresh home, through the store's
  own append: a third of them the person's words, a tenth corrected, a
  twentieth disabled, some observations superseded and a seventh dated, half
  of those ended, placed by their distance from the newest, and the same ten
  standing rules at the oldest depths at every size, eight that apply, one
  ended and one disabled. It runs `runx/memory#index`, with and without a
  focus on a word every entry holds, and `#search` as any caller does. It
  checks the index leads with the eight rules that apply and no other, and
  records the rows the index's listings visited and the most the pages it
  read could hold with the disabled and ended rows within them, at each
  size, the pages a search read per stream and the history rows it read, and
  the same two for the index's focus.
- **Journey 7, artifacts.** The runtime bench takes artifacts of 1 KiB, 1 MiB
  and 64 MiB. It reads each whole with `artifact.read` in 64 KiB pages and
  records the most any page read from the store past the bytes it returned.
  It has a module count each one's records as a JSON array, page by page, and
  records the bytes copied into the store, read from it and handed to the
  worker, over the artifact's bytes, which must be the same at every size.

The S-tier final gate captures all runtime-owned workloads into
`.scafld/perf/oss-runtime-s-tier-final.json` and compares them against
`.scafld/perf/oss-runtime-s-tier-baseline.json`:

```bash
pnpm perf:runtime:capture -- --output .scafld/perf/oss-runtime-s-tier-final.json --workloads workflow_planning,native_capability_dispatch,workflow_context_to_module,pure_module_cold_start,pure_module_session_reuse,pure_module_large_input,provider_effect_finality,artifact_admission,artifact_page_continuation,twitter_archive_selection,receipt_canonicalization,workflow_receipt_sealing,receipt_store_append,receipt_store_index,native_cli_launch,cli_file_input
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads workflow_planning --min-throughput-ratio 2.00
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads native_capability_dispatch,workflow_context_to_module --min-throughput-ratio 1.00
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads receipt_canonicalization,workflow_receipt_sealing --min-throughput-ratio 1.75
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads receipt_store_append,receipt_store_index --min-throughput-ratio 1.75 --max-growth-exponent 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads provider_effect_finality --min-throughput-ratio 0.85 --max-spawn-count 0
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads artifact_admission --min-throughput-ratio 0.85
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads artifact_page_continuation,twitter_archive_selection --min-throughput-ratio 0.85 --max-growth-exponent 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-s-tier-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads native_cli_launch --max-spawn-count 0 --max-p99-regression 1.10
pnpm perf:runtime:check -- --baseline .scafld/perf/oss-runtime-production-path-baseline.json --candidate .scafld/perf/oss-runtime-s-tier-final.json --workloads cli_file_input --max-p99-regression 1.25 --max-spawn-count 0
```

## Evidence ownership

Generated performance reports are the sole owner of measured values, hardware,
toolchain, source commit, tree digest, sample count, each Criterion mean and
its interval, each request workload's percentiles and counts, throughput,
growth, process metrics and journey measurements. This document defines workloads and gates; it does
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
