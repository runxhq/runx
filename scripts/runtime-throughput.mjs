#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { cpus, tmpdir, totalmem } from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";

const schema = "runx.oss_runtime_throughput.v1";
const repoRoot = process.cwd();
const cargoTargetDir = path.join(repoRoot, "crates", "target", "runx-perf");
const cargoPerfProfileDir = path.join(cargoTargetDir, "release");
const javascriptWorkerPath = path.join(
  cargoPerfProfileDir,
  process.platform === "win32" ? "runx-js-worker.exe" : "runx-js-worker",
);
const criterionRoot = path.join(cargoTargetDir, "criterion");
const runtimeResourceMetricsPath = path.join(cargoTargetDir, "runtime-resource-metrics.json");
const nativeCliWarmupCount = 3;
const nativeCliSampleCount = 20;
let cachedNativeCliProbe;
const runtimeBench = {
  package: "runx-runtime",
  bench: "graph_throughput",
  features: "agent",
  workloads: new Set([
    "graph_planning",
    "wide_fanout",
    "graph_receipt_sealing",
    "receipt_store_append",
    "receipt_store_index",
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
  ]),
  supportWorkloads: new Set([
    "receipt_store_append_scale_small",
    "receipt_store_append_scale_large",
    "receipt_store_index_scale_small",
    "receipt_store_index_scale_large",
    "artifact_page_continuation_scale_small",
    "artifact_page_continuation_scale_large",
    "event_page_continuation_scale_small",
    "event_page_continuation_scale_large",
    "twitter_archive_selection_scale_small",
    "twitter_archive_selection_scale_large",
  ]),
};
const scalingWorkloads = {
  receipt_store_append: {
    small: "receipt_store_append_scale_small",
    large: "receipt_store_append_scale_large",
    smallSize: 16,
    largeSize: 128,
  },
  receipt_store_index: {
    small: "receipt_store_index_scale_small",
    large: "receipt_store_index_scale_large",
    smallSize: 16,
    largeSize: 128,
  },
  artifact_page_continuation: {
    small: "artifact_page_continuation_scale_small",
    large: "artifact_page_continuation_scale_large",
    smallSize: 256 * 1024,
    largeSize: 8 * 1024 * 1024,
  },
  event_page_continuation: {
    small: "event_page_continuation_scale_small",
    large: "event_page_continuation_scale_large",
    smallSize: 100,
    largeSize: 1_000,
  },
  twitter_archive_selection: {
    small: "twitter_archive_selection_scale_small",
    large: "twitter_archive_selection_scale_large",
    smallSize: 1_500,
    largeSize: 12_000,
  },
};
const receiptBench = {
  package: "runx-receipts",
  bench: "receipt_canonicalization",
  workloads: new Set([
    "receipt_canonicalization",
    "receipt_body_json",
    "receipt_full_json",
  ]),
};
const defaultWorkloads = [
  "graph_planning",
  "wide_fanout",
  "native_cli_launch",
  "receipt_canonicalization",
  "graph_receipt_sealing",
  "receipt_store_append",
  "receipt_store_index",
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
  "cli_file_input",
];
const processWorkloads = new Set([
  "native_cli_launch",
  "cli_file_input",
]);
const knownWorkloads = new Set([
  ...runtimeBench.workloads,
  ...receiptBench.workloads,
  ...processWorkloads,
]);

const command = process.argv[2];
const options = parseArgs(process.argv.slice(3));

try {
  if (command === "list") {
    process.stdout.write(`${JSON.stringify({
      schema: "runx.oss_runtime_throughput.workloads.v1",
      default_workloads: defaultWorkloads,
      criterion_workloads: [...runtimeBench.workloads, ...receiptBench.workloads],
      process_workloads: [...processWorkloads],
    }, null, 2)}\n`);
  } else if (command === "capture") {
    const workloads = options.workloads ?? defaultWorkloads;
    const report = capture(workloads, options);
    if (!options.output) {
      process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
    } else {
      mkdirSync(path.dirname(path.resolve(repoRoot, options.output)), { recursive: true });
      writeFileSync(path.resolve(repoRoot, options.output), `${JSON.stringify(report, null, 2)}\n`);
      process.stdout.write(`${JSON.stringify({
        status: "captured",
        output: options.output,
        workloads: Object.keys(report.workloads),
      }, null, 2)}\n`);
    }
  } else if (command === "check") {
    if (!options.baseline) {
      throw new Error("perf:runtime:check requires --baseline <path>.");
    }
    const baseline = readJson(path.resolve(repoRoot, options.baseline));
    assertBaselineShape(baseline);
    const workloads = options.workloads ?? Object.keys(baseline.workloads);
    const current = options.candidate
      ? readJson(path.resolve(repoRoot, options.candidate))
      : capture(workloads, { ...options, captureMode: "check" });
    assertBaselineShape(current, "candidate");
    const findings = compareReports(baseline, current, workloads, options);
    const failed = findings.filter((finding) => finding.status === "failed");
    process.stdout.write(`${JSON.stringify({
      status: failed.length === 0 ? "passed" : "failed",
      workloads: findings,
    }, null, 2)}\n`);
    if (failed.length > 0) {
      process.exitCode = 1;
    }
  } else if (command === "verify-quality") {
    if (!options.candidate) {
      throw new Error("perf:runtime:verify-quality requires --candidate <path>.");
    }
    if (!options.expectedSourceCommit) {
      throw new Error(
        "perf:runtime:verify-quality requires --expected-source-commit <sha>.",
      );
    }
    const candidate = readJson(path.resolve(repoRoot, options.candidate));
    const checks = runtimeQualityChecks(candidate, options.expectedSourceCommit);
    const failed = checks.filter((check) => check.status === "failed");
    process.stdout.write(`${JSON.stringify({
      status: failed.length === 0 ? "passed" : "failed",
      checks,
    }, null, 2)}\n`);
    if (failed.length > 0) {
      process.exitCode = 1;
    }
  } else {
    throw new Error("Usage: runtime-throughput.mjs <list|capture|check|verify-quality> [--output path] [--baseline path] [--candidate path] [--expected-source-commit sha] [--workloads a,b] [--min-throughput-ratio n] [--max-growth-exponent n] [--max-spawn-count n] [--max-p99-regression n]");
  }
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}

function capture(workloads, options) {
  const requested = [...new Set(workloads)];
  assertKnownWorkloads(requested);
  clearCriterionMetrics(requested);
  runRequiredBenches(requested, options);
  const criterionMetrics = readCriterionMetricsWithRetry(requested);
  const metrics = {};
  for (const workload of requested) {
    if (workload === "native_cli_launch") {
      metrics[workload] = measureNativeCliLaunch();
      continue;
    }
    if (workload === "cli_file_input") {
      metrics[workload] = measureCliFileInput();
      continue;
    }
    const metric = criterionMetrics[workload];
    if (!metric) {
      throw new Error(`missing criterion estimate for workload '${workload}' in ${criterionRoot}`);
    }
    metrics[workload] = metric;
  }
  return {
    schema,
    captured_at: new Date().toISOString(),
    command: "perf:runtime:capture",
    source_commit: gitOutput(["rev-parse", "HEAD"]),
    source_tree_digest: workspaceDigest(),
    worktree_dirty: gitOutput(["status", "--porcelain"]).length > 0,
    build: {
      profile: "release",
      criterion_sample_size: Number(options.sampleSize ?? (options.captureMode === "check" ? 10 : 20)),
      rustc: commandOutput("rustc", ["--version"]),
      node: process.version,
    },
    hardware: hardwareIdentity(),
    workloads: metrics,
  };
}

function assertKnownWorkloads(workloads) {
  const unknown = workloads.filter((workload) => !knownWorkloads.has(workload));
  if (unknown.length > 0) {
    throw new Error(`unknown runtime workload(s): ${unknown.join(", ")}`);
  }
}

function runRequiredBenches(workloads, options) {
  const sampleSize = String(options.sampleSize ?? (options.captureMode === "check" ? 10 : 20));
  const runtimeWorkloads = workloads.filter((workload) => runtimeBench.workloads.has(workload));
  if (runtimeWorkloads.length > 0) {
    buildJavaScriptWorker();
    runCargoBench(runtimeBench, sampleSize, runtimeWorkloads, options);
  }
  const receiptWorkloads = workloads.filter((workload) => receiptBench.workloads.has(workload));
  if (receiptWorkloads.length > 0) {
    runCargoBench(receiptBench, sampleSize, receiptWorkloads, options);
  }
}

function buildJavaScriptWorker() {
  const args = [
    "build",
    "--manifest-path",
    "crates/Cargo.toml",
    "-p",
    "runx-js-worker",
    "--bin",
    "runx-js-worker",
    "--release",
  ];
  const result = spawnSync("cargo", args, {
    cwd: repoRoot,
    stdio: "inherit",
    env: cargoBenchEnv(),
  });
  if (result.status !== 0) {
    throw new Error(`cargo ${args.join(" ")} failed with exit ${result.status ?? "signal"}`);
  }
  if (!existsSync(javascriptWorkerPath)) {
    throw new Error(`cargo build runx-js-worker did not produce ${javascriptWorkerPath}`);
  }
}

function runCargoBench(bench, sampleSize, workloads, options) {
  const executable = buildCargoBench(bench);
  for (const run of criterionRuns(bench, workloads)) {
    runCriterionBench(executable, sampleSize, run.filter, options);
    waitForCriterionEstimates(run.workloads);
  }
}

function buildCargoBench(bench) {
  const args = [
    "bench",
    "--manifest-path",
    "crates/Cargo.toml",
    "-p",
    bench.package,
  ];
  if (bench.features) {
    args.push("--features", bench.features);
  }
  args.push("--bench", bench.bench, "--no-run", "--message-format=json");
  const result = spawnSync("cargo", args, {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "inherit"],
    env: cargoBenchEnv(),
  });
  if (result.status !== 0) {
    throw new Error(`cargo ${args.join(" ")} failed with exit ${result.status ?? "signal"}`);
  }
  const executable = benchExecutableFromCargoOutput(result.stdout, bench.bench);
  if (!executable) {
    throw new Error(`cargo ${args.join(" ")} did not report an executable for ${bench.bench}`);
  }
  return executable;
}

function benchExecutableFromCargoOutput(stdout, benchName) {
  let executable;
  for (const line of stdout.split(/\r?\n/u)) {
    if (!line.trimStart().startsWith("{")) {
      continue;
    }
    let event;
    try {
      event = JSON.parse(line);
    } catch {
      continue;
    }
    if (
      event.reason === "compiler-artifact"
      && Array.isArray(event.target?.kind)
      && event.target.kind.includes("bench")
      && event.target.name === benchName
      && typeof event.executable === "string"
    ) {
      executable = event.executable;
    }
  }
  return executable;
}

function runCriterionBench(executable, sampleSize, filter, options) {
  const args = [];
  if (filter) {
    args.push(filter);
  }
  args.push("--sample-size", sampleSize);
  const warmUpTime = options.warmUpTime ?? (options.captureMode === "check" ? 1 : undefined);
  const measurementTime = options.measurementTime ?? (options.captureMode === "check" ? 2 : undefined);
  if (warmUpTime !== undefined) {
    args.push("--warm-up-time", String(warmUpTime));
  }
  if (measurementTime !== undefined) {
    args.push("--measurement-time", String(measurementTime));
  }
  args.push("--bench");
  const result = spawnSync(executable, args, {
    cwd: repoRoot,
    stdio: "inherit",
    env: cargoBenchEnv(),
  });
  if (result.status !== 0) {
    throw new Error(`${executable} ${args.join(" ")} failed with exit ${result.status ?? "signal"}`);
  }
}

function cargoBenchEnv() {
  return {
    ...process.env,
    CARGO_TARGET_DIR: cargoTargetDir,
    CARGO_TERM_COLOR: process.env.CARGO_TERM_COLOR ?? "never",
    RUNX_JS_WORKER_PATH: javascriptWorkerPath,
    RUNX_PERF_RESOURCE_METRICS_PATH: runtimeResourceMetricsPath,
  };
}

function criterionRuns(bench, workloads) {
  const selected = expandedCriterionWorkloads(workloads)
    .filter((workload) => bench.workloads.has(workload) || bench.supportWorkloads?.has(workload));
  if (selected.length === 0) {
    return [];
  }
  return [{
    filter: `^(${selected.map(escapeRegularExpression).join("|")})$`,
    workloads: selected,
  }];
}

function escapeRegularExpression(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function clearCriterionMetrics(workloads) {
  rmSync(runtimeResourceMetricsPath, { force: true });
  for (const workload of expandedCriterionWorkloads(workloads)) {
    const workloadPath = path.join(criterionRoot, workload);
    if (existsSync(workloadPath)) {
      rmSync(workloadPath, { recursive: true, force: true });
    }
  }
}

function readCriterionMetricsWithRetry(requested) {
  const expectedCriterionWorkloads = requested.filter((workload) =>
    runtimeBench.workloads.has(workload) || receiptBench.workloads.has(workload)
  );
  const deadline = performance.now() + 2_000;
  let metrics = {};
  do {
    metrics = readCriterionMetrics(requested);
    if (expectedCriterionWorkloads.every((workload) => metrics[workload])) {
      return metrics;
    }
    sleepSync(50);
  } while (performance.now() < deadline);
  return metrics;
}

function waitForCriterionEstimates(workloads) {
  const deadline = performance.now() + 120_000;
  do {
    const metrics = readCriterionMetrics(workloads);
    if (workloads.every((workload) => metrics[workload])) {
      return;
    }
    sleepSync(100);
  } while (performance.now() < deadline);
}

function readCriterionMetrics(requested) {
  const rawMetrics = {};
  if (!existsSync(criterionRoot)) {
    return rawMetrics;
  }
  const requestedSet = new Set(expandedCriterionWorkloads(requested));
  const resourceMetrics = existsSync(runtimeResourceMetricsPath)
    ? readJson(runtimeResourceMetricsPath)
    : {};
  for (const estimatesPath of findEstimateFiles(criterionRoot)) {
    const workload = workloadNameFromEstimatePath(estimatesPath);
    if (!requestedSet.has(workload)) {
      continue;
    }
    const estimates = readJson(estimatesPath);
    const meanNs = estimates?.mean?.point_estimate;
    if (typeof meanNs !== "number" || !Number.isFinite(meanNs) || meanNs <= 0) {
      continue;
    }
    const sample = criterionSampleMetrics(estimatesPath);
    rawMetrics[workload] = {
      source: "criterion",
      unit: "iterations_per_second",
      mean_ns: meanNs,
      p50_ns: sample.p50_ns,
      p95_ns: sample.p95_ns,
      p99_ns: sample.p99_ns,
      throughput: 1_000_000_000 / meanNs,
      sample_count: sample.sample_count,
      ...(resourceMetrics[workload] ?? {}),
    };
  }
  const metrics = {};
  for (const workload of requested) {
    const metric = rawMetrics[workload];
    if (!metric) {
      continue;
    }
    const scaling = scalingWorkloads[workload];
    metrics[workload] = scaling
      ? {
          ...metric,
          growth_exponent: measuredGrowthExponent(rawMetrics, scaling),
        }
      : metric;
  }
  return metrics;
}

function expandedCriterionWorkloads(workloads) {
  return [...new Set(workloads.flatMap((workload) => {
    const scaling = scalingWorkloads[workload];
    return scaling ? [workload, scaling.small, scaling.large] : [workload];
  }))];
}

function measuredGrowthExponent(metrics, scaling) {
  const small = metrics[scaling.small]?.mean_ns;
  const large = metrics[scaling.large]?.mean_ns;
  if (
    typeof small !== "number"
    || typeof large !== "number"
    || small <= 0
    || large <= 0
  ) {
    throw new Error(
      `missing measured scale points '${scaling.small}' and '${scaling.large}'`,
    );
  }
  return Math.log(large / small) / Math.log(scaling.largeSize / scaling.smallSize);
}

function criterionSampleMetrics(estimatesPath) {
  const samplePath = path.join(path.dirname(estimatesPath), "sample.json");
  const sample = readJson(samplePath);
  if (!Array.isArray(sample.iters) || !Array.isArray(sample.times) || sample.iters.length !== sample.times.length) {
    throw new Error(`criterion sample is invalid at ${samplePath}`);
  }
  const perIteration = sample.times.map((time, index) => time / sample.iters[index]);
  if (perIteration.length === 0 || perIteration.some((value) => !Number.isFinite(value) || value <= 0)) {
    throw new Error(`criterion sample has invalid iteration timings at ${samplePath}`);
  }
  const sorted = [...perIteration].sort((left, right) => left - right);
  return {
    p50_ns: percentile(sorted, 0.50),
    p95_ns: percentile(sorted, 0.95),
    p99_ns: percentile(sorted, 0.99),
    sample_count: perIteration.length,
  };
}

function sleepSync(milliseconds) {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, milliseconds);
}

function findEstimateFiles(directory) {
  const entries = readdirSync(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const entryPath = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...findEstimateFiles(entryPath));
    } else if (entry.name === "estimates.json" && entryPath.endsWith(`${path.sep}new${path.sep}estimates.json`)) {
      files.push(entryPath);
    }
  }
  return files;
}

function workloadNameFromEstimatePath(estimatesPath) {
  const relative = path.relative(criterionRoot, estimatesPath);
  const segments = relative.split(path.sep);
  return segments[0] ?? "";
}

function measureNativeCliLaunch() {
  const probe = nativeCliProbe();
  for (let index = 0; index < nativeCliWarmupCount; index += 1) {
    runNativeCliProbe(probe);
  }
  const samples = [];
  for (let index = 0; index < nativeCliSampleCount; index += 1) {
    const started = performance.now();
    runNativeCliProbe(probe);
    samples.push((performance.now() - started) * 1_000_000);
  }
  return metricFromSamples("native_cli", samples, {
    spawn_count: 1,
  });
}

function measureCliFileInput() {
  const root = mkdtempSync(path.join(tmpdir(), "runx-cli-file-input-"));
  try {
    const skill = path.join(root, "skills", "cli-file-input-performance");
    mkdirSync(skill, { recursive: true });
    writeFileSync(
      path.join(skill, "SKILL.md"),
      `---
name: cli-file-input-performance
description: Exercise the canonical skill CLI with one file-backed input document.
---

# CLI file input performance

Digest one bounded note through the normal skill execution and receipt path.
`,
    );
    writeFileSync(
      path.join(skill, "X.yaml"),
      `skill: cli-file-input-performance
version: "0.1.0"

catalog:
  kind: graph
  audience: public
  visibility: public
  role: context
  execution: read
  completion: runtime_receipt
  requires_adapter: false
  approval: none

runners:
  digest:
    default: true
    type: graph
    inputs:
      note:
        type: string
        required: true
        description: Exact UTF-8 note to digest.
    graph:
      name: cli-file-input-performance
      steps:
        - id: digest
          tool: data.digest
          inputs:
            value: $input.note
            encoding: utf8_text
`,
    );
    writeFileSync(
      path.join(root, "input.json"),
      `${JSON.stringify({ note: "volume-independent-input" })}\n`,
    );
    const probe = {
      ...nativeCliProbe(),
      args: [
        "skill",
        "skills/cli-file-input-performance",
        "digest",
        "--inputs",
        "input.json",
        "--json",
      ],
      cwd: root,
      env: { RUNX_CWD: root },
    };
    for (let index = 0; index < nativeCliWarmupCount; index += 1) {
      runNativeCliProbe(probe);
    }
    const samples = [];
    for (let index = 0; index < nativeCliSampleCount; index += 1) {
      const started = performance.now();
      runNativeCliProbe(probe);
      samples.push((performance.now() - started) * 1_000_000);
    }
    return metricFromSamples("native_cli_file_input", samples, {
      spawn_count: 1,
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function nativeCliProbe() {
  if (cachedNativeCliProbe) {
    return cachedNativeCliProbe;
  }
  const binaryName = process.platform === "win32" ? "runx.exe" : "runx";
  const perfBinary = path.join(cargoPerfProfileDir, binaryName);
  const result = spawnSync(
    "cargo",
    [
      "build",
      "--manifest-path",
      "crates/Cargo.toml",
      "-p",
      "runx-cli",
      "--release",
      "--bin",
      "runx",
    ],
    {
      cwd: repoRoot,
      stdio: "inherit",
      env: cargoBenchEnv(),
    },
  );
  if (result.status !== 0) {
    throw new Error(`cargo build runx-cli failed with exit ${result.status ?? "signal"}`);
  }
  if (!existsSync(perfBinary)) {
    throw new Error(`cargo build runx-cli did not produce ${perfBinary}`);
  }
  cachedNativeCliProbe = { command: perfBinary, args: ["--version"] };
  return cachedNativeCliProbe;
}

function runNativeCliProbe(probe) {
  const result = spawnSync(probe.command, probe.args, {
    cwd: probe.cwd ?? repoRoot,
    env: probe.env ? { ...process.env, ...probe.env } : process.env,
    stdio: "ignore",
  });
  if (result.status !== 0) {
    throw new Error(`native CLI launch probe failed with exit ${result.status ?? "signal"}`);
  }
}

function metricFromSamples(source, samples, counters) {
  const sorted = [...samples].sort((left, right) => left - right);
  const meanNs = samples.reduce((sum, value) => sum + value, 0) / samples.length;
  const p95Ns = percentile(sorted, 0.95);
  const p99Ns = percentile(sorted, 0.99);
  return {
    source,
    unit: "iterations_per_second",
    mean_ns: meanNs,
    p50_ns: percentile(sorted, 0.50),
    p95_ns: p95Ns,
    p99_ns: p99Ns,
    throughput: 1_000_000_000 / meanNs,
    sample_count: samples.length,
    ...counters,
  };
}

function percentile(sorted, percentileValue) {
  if (sorted.length === 0) {
    return Number.NaN;
  }
  const index = Math.min(
    sorted.length - 1,
    Math.max(0, Math.ceil(sorted.length * percentileValue) - 1),
  );
  return sorted[index];
}

function compareReports(baseline, current, workloads, options) {
  const minRatio = Number(options.minThroughputRatio ?? 1);
  const maxGrowthExponent =
    options.maxGrowthExponent === undefined ? undefined : Number(options.maxGrowthExponent);
  const maxSpawnCount =
    options.maxSpawnCount === undefined ? undefined : Number(options.maxSpawnCount);
  const maxP99Regression =
    options.maxP99Regression === undefined ? undefined : Number(options.maxP99Regression);
  return workloads.map((workload) => {
    const baseMetric = baseline.workloads[workload];
    const currentMetric = current.workloads[workload];
    if (!baseMetric || !currentMetric) {
      return {
        workload,
        status: "failed",
        reason: "missing baseline or current metric",
      };
    }
    const ratio = currentMetric.throughput / baseMetric.throughput;
    const exponent = currentMetric.growth_exponent;
    const hasGrowthMetric = typeof exponent === "number";
    const p99Ratio = metricRatio(currentMetric.p99_ns, baseMetric.p99_ns);
    const ratioPassed = Number.isFinite(ratio) && ratio >= minRatio;
    const exponentPassed =
      maxGrowthExponent === undefined
      || (hasGrowthMetric && exponent <= maxGrowthExponent);
    const spawnPassed =
      maxSpawnCount === undefined
      || (typeof currentMetric.spawn_count === "number" && currentMetric.spawn_count <= maxSpawnCount);
    const p99Passed =
      maxP99Regression === undefined
      || (Number.isFinite(p99Ratio) && p99Ratio <= maxP99Regression);
    return {
      workload,
      status: ratioPassed && exponentPassed && spawnPassed && p99Passed ? "passed" : "failed",
      throughput_ratio: ratio,
      min_throughput_ratio: minRatio,
      ...(maxGrowthExponent === undefined || !hasGrowthMetric ? {} : {
        growth_exponent: exponent,
        max_growth_exponent: maxGrowthExponent,
      }),
      ...(maxSpawnCount === undefined ? {} : {
        spawn_count: currentMetric.spawn_count,
        max_spawn_count: maxSpawnCount,
      }),
      ...(maxP99Regression === undefined ? {} : {
        p99_regression: p99Ratio,
        max_p99_regression: maxP99Regression,
      }),
    };
  });
}

function runtimeQualityChecks(report, expectedSourceCommit) {
  const native = report?.workloads?.native_capability_dispatch;
  const session = report?.workloads?.pure_module_session_reuse;
  const fanout = report?.workloads?.bounded_parallel_fanout;
  return [
    qualityCheck("schema", report?.schema, { expected: schema }),
    qualityCheck("source_commit", report?.source_commit, { expected: expectedSourceCommit }),
    qualityCheck("worktree_clean", report?.worktree_dirty, { expected: false }),
    qualityCheck("native_sample_count", native?.sample_count, { minimum: 10 }),
    qualityCheck("native_spawn_count", native?.spawn_count, { expected: 0 }),
    qualityCheck("native_peak_in_flight", native?.peak_in_flight, { expected: 0 }),
    qualityCheck("session_sample_count", session?.sample_count, { minimum: 10 }),
    qualityCheck("session_spawn_count", session?.spawn_count, { expected: 1 }),
    qualityCheck("session_peak_in_flight", session?.peak_in_flight, { expected: 1 }),
    qualityCheck("fanout_sample_count", fanout?.sample_count, { minimum: 10 }),
    qualityCheck("fanout_spawn_count", fanout?.spawn_count, { minimum: 2, maximum: 4 }),
    qualityCheck("fanout_peak_in_flight", fanout?.peak_in_flight, { minimum: 2, maximum: 4 }),
    qualityCheck(
      "fanout_peak_within_spawn_count",
      fanout?.peak_in_flight,
      { maximum: fanout?.spawn_count },
    ),
  ];
}

function qualityCheck(id, actual, requirement) {
  const hasRange = "minimum" in requirement || "maximum" in requirement;
  const rangeIsValid =
    !hasRange
    || (
      Number.isInteger(actual)
      && (!("minimum" in requirement) || actual >= requirement.minimum)
      && (
        !("maximum" in requirement)
        || (Number.isInteger(requirement.maximum) && actual <= requirement.maximum)
      )
    );
  const equalityIsValid =
    !("expected" in requirement) || Object.is(actual, requirement.expected);
  return {
    id,
    status: rangeIsValid && equalityIsValid ? "passed" : "failed",
    actual: actual ?? null,
    ...requirement,
  };
}

function metricRatio(currentValue, baselineValue) {
  if (typeof currentValue !== "number" || typeof baselineValue !== "number") {
    return Number.NaN;
  }
  if (!Number.isFinite(currentValue) || !Number.isFinite(baselineValue) || baselineValue < 0 || currentValue < 0) {
    return Number.NaN;
  }
  if (baselineValue === 0) {
    return currentValue === 0 ? 1 : Number.POSITIVE_INFINITY;
  }
  return currentValue / baselineValue;
}

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--") {
      continue;
    }
    if (arg === "--output") {
      parsed.output = requiredValue(argv, ++index, arg);
    } else if (arg === "--baseline") {
      parsed.baseline = requiredValue(argv, ++index, arg);
    } else if (arg === "--candidate") {
      parsed.candidate = requiredValue(argv, ++index, arg);
    } else if (arg === "--expected-source-commit") {
      parsed.expectedSourceCommit = requiredValue(argv, ++index, arg);
    } else if (arg === "--workloads") {
      parsed.workloads = requiredValue(argv, ++index, arg).split(",").filter(Boolean);
    } else if (arg === "--min-throughput-ratio") {
      parsed.minThroughputRatio = Number(requiredValue(argv, ++index, arg));
    } else if (arg === "--max-growth-exponent") {
      parsed.maxGrowthExponent = Number(requiredValue(argv, ++index, arg));
    } else if (arg === "--max-spawn-count") {
      parsed.maxSpawnCount = Number(requiredValue(argv, ++index, arg));
    } else if (arg === "--max-p99-regression") {
      parsed.maxP99Regression = Number(requiredValue(argv, ++index, arg));
    } else if (arg === "--sample-size") {
      parsed.sampleSize = Number(requiredValue(argv, ++index, arg));
    } else if (arg === "--warm-up-time") {
      parsed.warmUpTime = Number(requiredValue(argv, ++index, arg));
    } else if (arg === "--measurement-time") {
      parsed.measurementTime = Number(requiredValue(argv, ++index, arg));
    } else {
      throw new Error(`unknown argument '${arg}'`);
    }
  }
  return parsed;
}

function requiredValue(argv, index, flag) {
  const value = argv[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function readJson(filePath) {
  return JSON.parse(readFileSync(filePath, "utf8"));
}

function assertBaselineShape(report, label = "baseline") {
  if (!report || report.schema !== schema || typeof report.workloads !== "object") {
    throw new Error(`${label} must use ${schema}`);
  }
}

function hardwareIdentity() {
  const processors = cpus();
  return {
    platform: process.platform,
    architecture: process.arch,
    cpu_model: processors[0]?.model ?? "unknown",
    logical_cpu_count: processors.length,
    total_memory_bytes: totalmem(),
  };
}

function gitOutput(args) {
  return commandOutput("git", args);
}

function commandOutput(commandName, args) {
  const result = spawnSync(commandName, args, {
    cwd: repoRoot,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (result.status !== 0) {
    throw new Error(`${commandName} ${args.join(" ")} failed: ${result.stderr.trim()}`);
  }
  return result.stdout.trim();
}

function workspaceDigest() {
  const listed = spawnSync("git", ["ls-files", "-co", "--exclude-standard", "-z"], {
    cwd: repoRoot,
    stdio: ["ignore", "pipe", "pipe"],
  });
  if (listed.status !== 0) {
    throw new Error(`git ls-files failed: ${listed.stderr.toString("utf8").trim()}`);
  }
  const files = listed.stdout
    .toString("utf8")
    .split("\0")
    .filter(Boolean)
    .sort();
  const digest = createHash("sha256");
  for (const relativePath of files) {
    digest.update(relativePath);
    digest.update("\0");
    const absolutePath = path.join(repoRoot, relativePath);
    if (!existsSync(absolutePath)) {
      digest.update("deleted\0");
      continue;
    }
    const contents = readFileSync(absolutePath);
    digest.update(String(contents.length));
    digest.update("\0");
    digest.update(contents);
    digest.update("\0");
  }
  return `sha256:${digest.digest("hex")}`;
}
