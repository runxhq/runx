#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const tempRoot = mkdtempSync(path.join(tmpdir(), "runx-perf-test-"));
let qualityFixtures = 0;

try {
  // `check`: a Criterion workload is judged by its mean, a request workload
  // by its p99 from individual timings, and a bound that doesn't apply to a
  // workload's kind fails it.
  const baselinePath = report("baseline", {
    workflow_planning: criterion({ throughput: 100, mean_ns: 10_000_000 }),
    native_cli_launch: requests({ p99_ns: 12_000_000 }),
  });
  const passingPath = report("candidate-pass", {
    workflow_planning: criterion({ throughput: 210, mean_ns: 4_700_000 }),
    native_cli_launch: requests({ p99_ns: 12_100_000 }),
  });
  const failingPath = report("candidate-fail", {
    workflow_planning: criterion({ throughput: 90, mean_ns: 11_000_000 }),
    native_cli_launch: requests({ p99_ns: 20_000_000, spawn_count: 3 }),
  });
  const missingTailPath = report("candidate-missing-tail", {
    native_cli_launch: requests({ p99_ns: undefined }),
  });
  const batchPercentilePath = report("candidate-batch-percentile", {
    workflow_planning: { ...criterion({ throughput: 210, mean_ns: 4_700_000 }), p99_ns: 5_000_000 },
  });
  const failedRequestsPath = report("candidate-failed-requests", {
    native_cli_launch: requests({ p99_ns: 12_000_000, errors: 1, timeouts: 1 }),
  });

  expectPass(runMeanCheck(baselinePath, passingPath), "a Criterion mean within its bound");
  expectFail(runMeanCheck(baselinePath, failingPath), "a Criterion mean past its bound");
  expectPass(runRequestCheck(baselinePath, passingPath), "a request p99 within its bound");
  expectFail(runRequestCheck(baselinePath, failingPath), "a request p99 past its bound");
  expectFail(runRequestCheck(baselinePath, missingTailPath), "a request workload with no measured p99");
  expectFail(runMeanCheck(baselinePath, batchPercentilePath), "a percentile of Criterion's batch averages", "batch averages");
  expectFail(runRequestCheck(baselinePath, failedRequestsPath), "a request workload whose requests failed", "1 failed and 1 timed-out");
  expectFail(
    runChecks(baselinePath, passingPath, "workflow_planning", ["--max-p99-regression", "1.10"]),
    "a p99 bound on a Criterion workload",
    "never request percentiles",
  );
  expectFail(
    runChecks(baselinePath, passingPath, "native_cli_launch", ["--min-throughput-ratio", "1.00"]),
    "a throughput bound on a request workload",
    "held arrival rate",
  );
  expectFail(
    runChecks(baselinePath, passingPath, "workflow_planning", ["--max-growth-exponent", "1.10"]),
    "a growth bound with no measured scale points",
  );

  // `verify-quality`: every journey's work budget, each from its own
  // measurement, and every timing honest about what it is.
  expectPass(runQualityVerification(quality({})), "release-quality evidence with every journey in budget");

  const withoutMemory = journeys();
  delete withoutMemory["5"];
  expectFail(
    runQualityVerification(quality({ journeys: withoutMemory })),
    "a journey with no measurement",
    "journey_5",
  );

  const growingWrites = journeys();
  growingWrites["2"].store_writes_per_call["1000"] = 3;
  expectFail(
    runQualityVerification(quality({ journeys: growingWrites })),
    "store writes that grow with the run",
    "journey_2.constant_store_writes_per_call",
  );

  expectFail(
    runQualityVerification(quality({ nativeExtra: { p99_ns: 1_500_000 } })),
    "a percentile of Criterion's batch averages",
    "measurement.native_capability_dispatch",
  );

  const qualityFail = runQualityVerification(quality({ sessionSpawnCount: 2, worktreeDirty: true }));
  expectFail(qualityFail, "invalid release-quality evidence", "worktree_clean");
  expectFail(qualityFail, "invalid release-quality evidence", "session_spawn_count");

  assertReleaseProbeInvariant();
  assertArchitectureWorkloads();

  process.stdout.write("Runtime perf test check passed.\n");
} finally {
  rmSync(tempRoot, { recursive: true, force: true });
}

function expectPass(result, what) {
  if (result.status !== 0) {
    process.stderr.write(result.stderr || result.stdout);
    throw new Error(`runtime perf test refused ${what}`);
  }
}

function expectFail(result, what, named) {
  if (result.status === 0) {
    process.stderr.write(result.stdout);
    throw new Error(`runtime perf test accepted ${what}`);
  }
  if (named && !`${result.stdout}${result.stderr}`.includes(named)) {
    process.stderr.write(result.stdout);
    throw new Error(`runtime perf test refused ${what} without naming ${named}`);
  }
}

function runMeanCheck(baselinePath, candidatePath) {
  return runChecks(baselinePath, candidatePath, "workflow_planning", ["--min-throughput-ratio", "2.00"]);
}

function runRequestCheck(baselinePath, candidatePath) {
  return runChecks(baselinePath, candidatePath, "native_cli_launch", [
    "--max-spawn-count",
    "0",
    "--max-p99-regression",
    "1.10",
  ]);
}

function runChecks(baselinePath, candidatePath, workload, bounds) {
  return spawnSync(
    "node",
    [
      "scripts/runtime-throughput.mjs",
      "check",
      "--baseline",
      baselinePath,
      "--candidate",
      candidatePath,
      "--workloads",
      workload,
      ...bounds,
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
    },
  );
}

function runQualityVerification(candidatePath) {
  return spawnSync(
    "node",
    [
      "scripts/runtime-throughput.mjs",
      "verify-quality",
      "--candidate",
      candidatePath,
      "--expected-source-commit",
      "quality-fixture-commit",
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
    },
  );
}

/** A Criterion workload as a capture reports it: its mean and the mean's interval. */
function criterion({ throughput, mean_ns }) {
  return {
    source: "criterion",
    unit: "iterations_per_second",
    mean_ns,
    mean_lower_ns: mean_ns * 0.98,
    mean_upper_ns: mean_ns * 1.02,
    confidence_level: 0.95,
    throughput,
    sample_count: 20,
  };
}

/** A request workload as a capture reports it: each request timed on its own. */
function requests({ p99_ns, spawn_count = 0, errors = 0, timeouts = 0 }) {
  const metric = {
    source: "requests",
    unit: "nanoseconds",
    arrival_per_second: 10,
    requests: 20,
    completed: 20 - errors - timeouts,
    errors,
    timeouts,
    mean_ns: 3_000_000,
    p50_ns: 3_000_000,
    p95_ns: 3_100_000,
    p99_ns,
    spawn_count,
  };
  if (p99_ns === undefined) {
    delete metric.p99_ns;
  }
  return metric;
}

function report(name, workloads) {
  const filePath = path.join(tempRoot, `${name}.json`);
  writeFileSync(
    filePath,
    `${JSON.stringify({
      schema: "runx.oss_runtime_throughput.v1",
      captured_at: "2026-05-27T00:00:00.000Z",
      command: "perf:test-check",
      workloads,
    }, null, 2)}\n`,
  );
  return filePath;
}

/** Every journey's structural measurements, each within its work budget. */
function journeys() {
  return {
    1: {
      processes_started: 1,
      network_requests: 0,
      verifications_per_identity: 1,
      packages_read: { 1: 1, 10: 1, 100: 1, 1000: 1 },
    },
    2: {
      store_writes_per_call: { 10: 2, 100: 2, 1000: 2 },
      context_reserializations_per_call: 0,
      transactions_spanning_provider_calls: 0,
    },
    3: {
      settle_reads_per_call: { 1: 4, 1000: 4 },
      results_delayed_by_slow_read: 0,
      peak_concurrency: 8,
      max_concurrency: 8,
    },
    4: {
      run_read_rows_with_nothing_new: { 10: 1, 100: 1, 1000: 1 },
      recovery_reads_per_stream: 1,
    },
    5: {
      index_rows_visited: { 100: 40, 10000: 40, 100000: 40 },
      index_rows_its_budget_can_show: 40,
      search_pages_per_stream: 1,
      search_history_rows_read: 0,
    },
    6: {
      queries_per_page: { 10: 1, 100: 1, 1000: 1 },
      package_bodies_read: 0,
    },
    7: {
      copies_per_byte: { "1KiB": 2, "1MiB": 2, "64MiB": 2 },
      page_bytes_read_beyond_range: 0,
    },
    8: {
      context_reads_unused: 0,
      repeated_reads: { previous: 3, current: 3 },
      model_calls: { previous: 5, current: 4 },
    },
  };
}

function quality({
  sessionSpawnCount = 1,
  worktreeDirty = false,
  journeys: measured = journeys(),
  nativeExtra = {},
}) {
  qualityFixtures += 1;
  const filePath = path.join(tempRoot, `quality-${qualityFixtures}.json`);
  writeFileSync(
    filePath,
    `${JSON.stringify({
      schema: "runx.oss_runtime_throughput.v1",
      source_commit: "quality-fixture-commit",
      worktree_dirty: worktreeDirty,
      workloads: {
        native_capability_dispatch: { ...criterion({ throughput: 900, mean_ns: 1_100_000 }), ...nativeExtra },
        pure_module_session_reuse: {
          ...criterion({ throughput: 3_400, mean_ns: 290_000 }),
          spawn_count: sessionSpawnCount,
          peak_in_flight: 1,
        },
      },
      journeys: measured,
    }, null, 2)}\n`,
  );
  return filePath;
}

function assertReleaseProbeInvariant() {
  const source = readFileSync(path.join(workspaceRoot, "scripts/runtime-throughput.mjs"), "utf8");
  if (!/cargoPerfProfileDir\s*=\s*path\.join\(cargoTargetDir,\s*"release"\)/u.test(source)) {
    throw new Error("runtime perf test must use the release profile directory for process probes");
  }
  const workerBuildSource = functionSource(source, "buildJavaScriptWorker", "runCargoBench");
  if (!workerBuildSource.includes('"--release"') || !workerBuildSource.includes('"runx-js-worker"')) {
    throw new Error("runtime perf test must build the deterministic worker in release mode");
  }
  if (!/RUNX_JS_WORKER_PATH:\s*javascriptWorkerPath/u.test(source)) {
    throw new Error("runtime perf test must bind benchmarks to the release worker it built");
  }
  const nativeProbeSource = functionSource(source, "nativeCliProbe", "runNativeCliProbe");
  if (!/"--release"[\s\S]*"--bin"[\s\S]*"runx"/u.test(nativeProbeSource)) {
    throw new Error("runtime perf test must build the native runx launch probe with --release");
  }
  if (!/cargo build runx-cli did not produce/u.test(nativeProbeSource)) {
    throw new Error("runtime perf test must verify the native runx release probe exists after build");
  }
  if (!/nativeCliWarmupCount\s*=\s*3/u.test(source) || !/nativeCliSampleCount\s*=\s*20/u.test(source)) {
    throw new Error("native CLI probe must use three warmups and twenty measured launches");
  }
  if (/crates",\s*"target",\s*"debug"/u.test(source)) {
    throw new Error("runtime perf test must not fall back to stale crates/target/debug probe binaries");
  }
}

function assertArchitectureWorkloads() {
  const criterionExpected = [
    "native_capability_dispatch",
    "workflow_context_to_module",
    "pure_module_cold_start",
    "pure_module_session_reuse",
    "pure_module_large_input",
    "provider_effect_finality",
    "artifact_admission",
    "artifact_page_continuation",
    "twitter_archive_selection",
  ];
  const processExpected = ["cli_file_input"];
  const listed = spawnSync("node", ["scripts/runtime-throughput.mjs", "list"], {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (listed.status !== 0) {
    throw new Error(`runtime workload catalog failed: ${listed.stderr.trim()}`);
  }
  const catalog = JSON.parse(listed.stdout);
  for (const workload of [...criterionExpected, ...processExpected]) {
    if (!catalog.default_workloads?.includes(workload)) {
      throw new Error(`runtime perf defaults are missing ${workload}`);
    }
  }
  for (const workload of criterionExpected) {
    if (!catalog.criterion_workloads?.includes(workload)) {
      throw new Error(`runtime Criterion catalog is missing ${workload}`);
    }
  }
  for (const workload of processExpected) {
    if (!catalog.process_workloads?.includes(workload)) {
      throw new Error(`runtime process-probe catalog is missing ${workload}`);
    }
  }

  const benchmarkSource = [
    "crates/runx-runtime/benches/workflow_throughput/runtime_paths.rs",
    "crates/runx-runtime/benches/workflow_throughput/volume_paths/artifact_io.rs",
    "crates/runx-runtime/benches/workflow_throughput/volume_paths/twitter_selection.rs",
  ]
    .map((relative) => readFileSync(path.join(workspaceRoot, relative), "utf8"))
    .join("\n");
  for (const workload of criterionExpected) {
    const direct = benchmarkSource.includes(`bench_function("${workload}"`);
    const registered = benchmarkSource.includes(`c, "${workload}"`);
    if (!direct && !registered) {
      throw new Error(`runtime benchmark implementation is missing ${workload}`);
    }
  }

  const testSource = readFileSync(
    path.join(workspaceRoot, "scripts/runtime-throughput.mjs"),
    "utf8",
  );
  for (const invariant of ["source_tree_digest", "hardwareIdentity", "sample_count", "sample.json"]) {
    if (!testSource.includes(invariant)) {
      throw new Error(`runtime perf evidence is missing ${invariant}`);
    }
  }
  if (/growth_exponent:\s*1(?:\.0+)?(?:,|\s*\})/u.test(testSource)) {
    throw new Error("runtime perf test must not report a hard-coded linear growth exponent");
  }
  for (const invariant of ["measuredGrowthExponent", "runtimeResourceMetricsPath", "p50_ns"]) {
    if (!testSource.includes(invariant)) {
      throw new Error(`runtime perf evidence is missing measured invariant ${invariant}`);
    }
  }
}

function functionSource(source, functionName, nextFunctionName) {
  const start = source.indexOf(`function ${functionName}(`);
  const end = source.indexOf(`function ${nextFunctionName}(`);
  if (start < 0 || end < 0 || end <= start) {
    throw new Error(`runtime perf test is missing expected ${functionName} function boundary`);
  }
  return source.slice(start, end);
}
