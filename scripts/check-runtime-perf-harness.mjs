#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(fileURLToPath(new URL("..", import.meta.url)));
const tempRoot = mkdtempSync(path.join(tmpdir(), "runx-perf-harness-"));

try {
  const baselinePath = path.join(tempRoot, "baseline.json");
  const passingPath = path.join(tempRoot, "candidate-pass.json");
  const failingPath = path.join(tempRoot, "candidate-fail.json");
  const missingEvidencePath = path.join(tempRoot, "candidate-missing-evidence.json");
  const qualityPassingPath = path.join(tempRoot, "quality-pass.json");
  const qualityFailingPath = path.join(tempRoot, "quality-fail.json");

  writeFixture(baselinePath, {
    throughput: 100,
    mean_ns: 10_000_000,
    p95_ns: 11_000_000,
    p99_ns: 12_000_000,
    spawn_count: 1,
  });
  writeFixture(passingPath, {
    throughput: 210,
    mean_ns: 4_700_000,
    p95_ns: 5_000_000,
    p99_ns: 12_100_000,
    spawn_count: 1,
  });
  writeFixture(failingPath, {
    throughput: 90,
    mean_ns: 11_000_000,
    p95_ns: 15_000_000,
    p99_ns: 20_000_000,
    spawn_count: 3,
  });
  writeFixture(missingEvidencePath, {
    throughput: 210,
    mean_ns: 4_700_000,
    spawn_count: 1,
  });

  const pass = runCheck(baselinePath, passingPath);
  if (pass.status !== 0) {
    process.stderr.write(pass.stderr || pass.stdout);
    throw new Error("runtime perf harness rejected the passing candidate fixture");
  }

  const fail = runCheck(baselinePath, failingPath);
  if (fail.status === 0) {
    process.stderr.write(fail.stdout);
    throw new Error("runtime perf harness accepted the intentionally bad candidate fixture");
  }

  const missingEvidence = runCheck(baselinePath, missingEvidencePath);
  if (missingEvidence.status === 0) {
    throw new Error("runtime perf harness accepted a candidate missing measured tail evidence");
  }

  const missingGrowth = runGrowthCheck(baselinePath, passingPath);
  if (missingGrowth.status === 0) {
    throw new Error("runtime perf harness accepted a growth gate without measured scale evidence");
  }

  writeQualityFixture(qualityPassingPath, {
    fanoutSpawnCount: 3,
    fanoutPeakInFlight: 3,
  });
  const qualityPass = runQualityVerification(qualityPassingPath);
  if (qualityPass.status !== 0) {
    process.stderr.write(qualityPass.stderr || qualityPass.stdout);
    throw new Error("runtime perf harness rejected valid release-quality evidence");
  }

  writeQualityFixture(qualityFailingPath, {
    fanoutSpawnCount: 5,
    fanoutPeakInFlight: 5,
    worktreeDirty: true,
  });
  const qualityFail = runQualityVerification(qualityFailingPath);
  if (qualityFail.status === 0) {
    throw new Error("runtime perf harness accepted invalid release-quality evidence");
  }
  for (const expectedDiagnostic of ["worktree_clean", "fanout_spawn_count", "fanout_peak_in_flight"]) {
    if (!qualityFail.stdout.includes(expectedDiagnostic)) {
      throw new Error(`runtime quality failure did not name ${expectedDiagnostic}`);
    }
  }

  assertReleaseProbeInvariant();
  assertArchitectureWorkloads();

  process.stdout.write("Runtime perf harness check passed.\n");
} finally {
  rmSync(tempRoot, { recursive: true, force: true });
}

function runGrowthCheck(baselinePath, candidatePath) {
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
      "graph_planning",
      "--max-growth-exponent",
      "1.10",
    ],
    {
      cwd: workspaceRoot,
      encoding: "utf8",
    },
  );
}

function runCheck(baselinePath, candidatePath) {
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
      "graph_planning",
      "--min-throughput-ratio",
      "2.00",
      "--max-spawn-count",
      "1",
      "--max-p99-regression",
      "1.10",
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

function writeFixture(filePath, metric) {
  writeFileSync(
    filePath,
    `${JSON.stringify({
      schema: "runx.oss_runtime_throughput.v1",
      captured_at: "2026-05-27T00:00:00.000Z",
      command: "perf:harness-check",
      workloads: {
        graph_planning: {
          source: "fixture",
          unit: "iterations_per_second",
          ...metric,
        },
      },
    }, null, 2)}\n`,
  );
}

function writeQualityFixture(
  filePath,
  {
    fanoutSpawnCount,
    fanoutPeakInFlight,
    worktreeDirty = false,
  },
) {
  writeFileSync(
    filePath,
    `${JSON.stringify({
      schema: "runx.oss_runtime_throughput.v1",
      source_commit: "quality-fixture-commit",
      worktree_dirty: worktreeDirty,
      workloads: {
        native_capability_dispatch: {
          sample_count: 10,
          spawn_count: 0,
          peak_in_flight: 0,
        },
        pure_module_session_reuse: {
          sample_count: 10,
          spawn_count: 1,
          peak_in_flight: 1,
        },
        bounded_parallel_fanout: {
          sample_count: 10,
          spawn_count: fanoutSpawnCount,
          peak_in_flight: fanoutPeakInFlight,
        },
      },
    }, null, 2)}\n`,
  );
}

function assertReleaseProbeInvariant() {
  const source = readFileSync(path.join(workspaceRoot, "scripts/runtime-throughput.mjs"), "utf8");
  if (!/cargoPerfProfileDir\s*=\s*path\.join\(cargoTargetDir,\s*"release"\)/u.test(source)) {
    throw new Error("runtime perf harness must use the release profile directory for process probes");
  }
  const workerBuildSource = functionSource(source, "buildJavaScriptWorker", "runCargoBench");
  if (!workerBuildSource.includes('"--release"') || !workerBuildSource.includes('"runx-js-worker"')) {
    throw new Error("runtime perf harness must build the deterministic worker in release mode");
  }
  if (!/RUNX_JS_WORKER_PATH:\s*javascriptWorkerPath/u.test(source)) {
    throw new Error("runtime perf harness must bind benchmarks to the release worker it built");
  }
  const nativeProbeSource = functionSource(source, "nativeCliProbe", "runNativeCliProbe");
  if (!/"--release"[\s\S]*"--bin"[\s\S]*"runx"/u.test(nativeProbeSource)) {
    throw new Error("runtime perf harness must build the native runx launch probe with --release");
  }
  if (!/cargo build runx-cli did not produce/u.test(nativeProbeSource)) {
    throw new Error("runtime perf harness must verify the native runx release probe exists after build");
  }
  if (!/nativeCliWarmupCount\s*=\s*3/u.test(source) || !/nativeCliSampleCount\s*=\s*20/u.test(source)) {
    throw new Error("native CLI probe must use three warmups and twenty measured launches");
  }
  if (/crates",\s*"target",\s*"debug"/u.test(source)) {
    throw new Error("runtime perf harness must not fall back to stale crates/target/debug probe binaries");
  }
}

function assertArchitectureWorkloads() {
  const criterionExpected = [
    "native_capability_dispatch",
    "graph_context_to_module",
    "pure_module_cold_start",
    "pure_module_session_reuse",
    "pure_module_large_input",
    "bounded_parallel_fanout",
    "provider_effect_finality",
    "artifact_admission",
    "artifact_page_continuation",
    "event_page_continuation",
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
    "crates/runx-runtime/benches/graph_throughput/runtime_paths.rs",
    "crates/runx-runtime/benches/graph_throughput/volume_paths/artifact_io.rs",
    "crates/runx-runtime/benches/graph_throughput/volume_paths/event_paging.rs",
    "crates/runx-runtime/benches/graph_throughput/volume_paths/twitter_selection.rs",
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

  const graphBenchmarkSource = readFileSync(
    path.join(workspaceRoot, "crates/runx-runtime/benches/graph_throughput.rs"),
    "utf8",
  );
  for (const retiredReimplementation of [
    "context_projection",
    "output_projection",
    "fn project_context(",
    "fn project_output(",
  ]) {
    if (graphBenchmarkSource.includes(retiredReimplementation)) {
      throw new Error(
        `runtime benchmark must not restore benchmark-only projection path ${retiredReimplementation}`,
      );
    }
  }

  const harnessSource = readFileSync(
    path.join(workspaceRoot, "scripts/runtime-throughput.mjs"),
    "utf8",
  );
  for (const invariant of ["source_tree_digest", "hardwareIdentity", "sample_count", "sample.json"]) {
    if (!harnessSource.includes(invariant)) {
      throw new Error(`runtime perf evidence is missing ${invariant}`);
    }
  }
  if (/growth_exponent:\s*1(?:\.0+)?(?:,|\s*\})/u.test(harnessSource)) {
    throw new Error("runtime perf harness must not report a hard-coded linear growth exponent");
  }
  for (const invariant of ["measuredGrowthExponent", "runtimeResourceMetricsPath", "p50_ns"]) {
    if (!harnessSource.includes(invariant)) {
      throw new Error(`runtime perf evidence is missing measured invariant ${invariant}`);
    }
  }
}

function functionSource(source, functionName, nextFunctionName) {
  const start = source.indexOf(`function ${functionName}(`);
  const end = source.indexOf(`function ${nextFunctionName}(`);
  if (start < 0 || end < 0 || end <= start) {
    throw new Error(`runtime perf harness is missing expected ${functionName} function boundary`);
  }
  return source.slice(start, end);
}
