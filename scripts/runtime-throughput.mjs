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
// A request workload's arrivals are held at this rate, one request at a time,
// whatever each takes; a request that runs past its slot delays the next, and
// the next one's latency counts that wait.
const defaultArrivalPerSecond = 10;
const requestTimeoutMs = 10_000;
// Journey 1's sizes: one skill alone, then workspaces of 10, 100 and 1,000
// packages. Declared with the rest, above the command that may capture.
const coldStartSizes = [1, 10, 100, 1000];
let cachedNativeCliProbe;
const runtimeBench = {
  package: "runx-runtime",
  bench: "workflow_throughput",
  features: "agent,probe-fixtures",
  workloads: new Set([
    "workflow_planning",
    "workflow_receipt_sealing",
    "receipt_store_append",
    "receipt_store_index",
    "native_capability_dispatch",
    "workflow_context_to_module",
    "pure_module_cold_start",
    "pure_module_session_reuse",
    "pure_module_large_input",
    "provider_effect_finality",
    "artifact_admission",
    "artifact_page_continuation",
    "twitter_archive_selection",
  ]),
  supportWorkloads: new Set([
    "receipt_store_append_scale_small",
    "receipt_store_append_scale_large",
    "receipt_store_index_scale_small",
    "receipt_store_index_scale_large",
    "artifact_page_continuation_scale_small",
    "artifact_page_continuation_scale_large",
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
  "workflow_planning",
  "native_cli_launch",
  "receipt_canonicalization",
  "workflow_receipt_sealing",
  "receipt_store_append",
  "receipt_store_index",
  "native_capability_dispatch",
  "workflow_context_to_module",
  "pure_module_cold_start",
  "pure_module_session_reuse",
  "pure_module_large_input",
  "provider_effect_finality",
  "artifact_admission",
  "artifact_page_continuation",
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
    throw new Error("Usage: runtime-throughput.mjs <list|capture|check|verify-quality> [--output path] [--baseline path] [--candidate path] [--expected-source-commit sha] [--workloads a,b] [--min-throughput-ratio n] [--max-growth-exponent n] [--max-spawn-count n] [--max-p99-regression n] [--arrival-rate n]");
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
      metrics[workload] = measureNativeCliLaunch(options);
      continue;
    }
    if (workload === "cli_file_input") {
      metrics[workload] = measureCliFileInput(options);
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
    // Each journey's structural measurements, from the probes that count
    // them: journey 1 from launches of the CLI, journeys 2, 3, 4, 5 and 7 from
    // the runtime bench, which runs them on the runtimes its fixtures warmed. A
    // journey no probe measures is absent, and the quality gate fails it.
    journeys: {
      ...(existsSync(runtimeResourceMetricsPath)
        ? readJson(runtimeResourceMetricsPath).journeys ?? {}
        : {}),
      1: measureColdStart(),
    },
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
    const interval = estimates.mean.confidence_interval ?? {};
    rawMetrics[workload] = {
      source: "criterion",
      unit: "iterations_per_second",
      mean_ns: meanNs,
      mean_lower_ns: interval.lower_bound,
      mean_upper_ns: interval.upper_bound,
      confidence_level: interval.confidence_level,
      throughput: 1_000_000_000 / meanNs,
      sample_count: criterionSampleCount(estimatesPath),
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

// How many samples Criterion took. Each sample is a batch's average, so a
// sample is never read as one request's latency.
function criterionSampleCount(estimatesPath) {
  const samplePath = path.join(path.dirname(estimatesPath), "sample.json");
  const sample = readJson(samplePath);
  if (!Array.isArray(sample.iters) || !Array.isArray(sample.times) || sample.iters.length !== sample.times.length) {
    throw new Error(`criterion sample is invalid at ${samplePath}`);
  }
  return sample.times.length;
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

function measureNativeCliLaunch(options) {
  return measureRequests(nativeCliProbe(), options);
}

function measureCliFileInput(options) {
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
      path.join(skill, "run.yaml"),
      `version: "0.1.0"
minimum_runx: 0.12.0

catalog:
  visibility: public

methods:
  digest:
    inputs:
      note:
        type: string
        description: Exact UTF-8 note to digest.
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
    return measureRequests(probe, options);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

// performance.md's journey 1, cold start, counted: `runx skill inspect`
// resolving one skill in a workspace of each size, as a new process after one
// discarded launch leaves the runx home as after an earlier run. Every
// package is alike, so a package readiness reads past its own shows as
// growth across the sizes. A launch's own process is one, and each child it
// spawns is another.
function measureColdStart() {
  const probe = nativeCliProbe();
  const root = mkdtempSync(path.join(tmpdir(), "runx-cold-start-"));
  try {
    const launches = {};
    for (const size of coldStartSizes) {
      const workspace = path.join(root, String(size));
      for (let index = 0; index < size; index += 1) {
        writeColdStartPackage(workspace, `cold-${String(index).padStart(4, "0")}`);
      }
      const launch = {
        command: probe.command,
        args: ["skill", "inspect", "cold-0000", "--json"],
        cwd: workspace,
        env: { RUNX_CWD: workspace, RUNX_HOME: path.join(root, `home-${size}`) },
      };
      const countsPath = path.join(root, `counts-${size}.json`);
      for (const counts of [undefined, countsPath]) {
        const outcome = runNativeCliProbe(launch, counts);
        if (outcome !== "completed") {
          throw new Error(`journey 1's launch in a workspace of ${size} packages ended in ${outcome}`);
        }
      }
      launches[size] = readJson(countsPath);
    }
    const counted = Object.values(launches);
    const most = (count) => Math.max(...counted.map(count));
    return {
      processes_started: most((counts) => 1 + counts.process_spawns),
      network_requests: most((counts) => counts.network_requests),
      verifications_per_identity: most((counts) => counts.most_verifications_of_one_package),
      packages_read: Object.fromEntries(
        coldStartSizes.map((size) => [String(size), launches[size].packages_read]),
      ),
      launches,
    };
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

function writeColdStartPackage(workspace, name) {
  const skill = path.join(workspace, "skills", name);
  mkdirSync(skill, { recursive: true });
  writeFileSync(
    path.join(skill, "SKILL.md"),
    `---\nname: ${name}\ndescription: Digest one fixed subject.\n---\n\n# ${name}\n`,
  );
  writeFileSync(
    path.join(skill, "run.yaml"),
    `minimum_runx: 0.12.0\nmethods:\n  digest:\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs:\n          value: {subject: ${name}}\n`,
  );
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

// How one request ended: it completed, failed, or ran past its timeout.
// With `countsPath`, the process writes the work it counted there at exit.
function runNativeCliProbe(probe, countsPath) {
  const result = spawnSync(probe.command, probe.args, {
    cwd: probe.cwd ?? repoRoot,
    env: {
      ...process.env,
      ...(probe.env ?? {}),
      ...(countsPath ? { RUNX_PERF_COUNTS_PATH: countsPath } : {}),
    },
    stdio: "ignore",
    timeout: requestTimeoutMs,
  });
  if (result.error?.code === "ETIMEDOUT") {
    return "timeout";
  }
  return result.status === 0 ? "completed" : "error";
}

// Each request timed on its own, at a held arrival rate, one at a time: the
// i-th request is due at start + i / rate, and its latency runs from when it
// was due to when it ended, so a stall shows in every request it delays.
// Errors and timeouts are counted, and percentiles come from the completed
// requests' own timings. The untimed warm-up requests count the processes
// runx spawns for one, so the timed ones carry no counting.
function measureRequests(probe, options) {
  const countsRoot = mkdtempSync(path.join(tmpdir(), "runx-request-counts-"));
  let spawnCount = 0;
  try {
    for (let index = 0; index < nativeCliWarmupCount; index += 1) {
      const countsPath = path.join(countsRoot, `${index}.json`);
      runNativeCliProbe(probe, countsPath);
      spawnCount = Math.max(spawnCount, readJson(countsPath).process_spawns);
    }
  } finally {
    rmSync(countsRoot, { recursive: true, force: true });
  }
  const arrivalPerSecond = Number(options.arrivalRate ?? defaultArrivalPerSecond);
  const intervalMs = 1_000 / arrivalPerSecond;
  const latencies = [];
  let errors = 0;
  let timeouts = 0;
  const start = performance.now();
  for (let index = 0; index < nativeCliSampleCount; index += 1) {
    const due = start + index * intervalMs;
    const wait = due - performance.now();
    if (wait > 0) {
      sleepSync(wait);
    }
    const outcome = runNativeCliProbe(probe);
    const latencyNs = (performance.now() - due) * 1_000_000;
    if (outcome === "completed") {
      latencies.push(latencyNs);
    } else if (outcome === "timeout") {
      timeouts += 1;
    } else {
      errors += 1;
    }
  }
  return requestMetric(latencies, {
    arrival_per_second: arrivalPerSecond,
    requests: nativeCliSampleCount,
    errors,
    timeouts,
    // The child processes runx spawned for one request, as it counted them.
    spawn_count: spawnCount,
  });
}

function requestMetric(latencies, counts) {
  const sorted = [...latencies].sort((left, right) => left - right);
  return {
    source: "requests",
    unit: "nanoseconds",
    completed: latencies.length,
    mean_ns: latencies.reduce((sum, value) => sum + value, 0) / Math.max(1, latencies.length),
    p50_ns: percentile(sorted, 0.50),
    p95_ns: percentile(sorted, 0.95),
    p99_ns: percentile(sorted, 0.99),
    ...counts,
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

// Each workload is judged by what it measures. A Criterion workload is a CPU
// benchmark: its mean, over batches, against the baseline's, and its growth
// across scale points. A request workload is individual timings at a held
// arrival rate: its p99 against the baseline's, with every request completed.
// A bound that doesn't apply to a workload's kind fails it, so nothing passes
// by having no figure to compare.
function compareReports(baseline, current, workloads, options) {
  const minRatio = options.minThroughputRatio;
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
    const refused = measurementRefusals(workload, currentMetric);
    const requests = currentMetric.source === "requests";
    if (requests && minRatio !== undefined) {
      refused.push("a request workload runs at a held arrival rate, so it has no throughput to compare: bound its p99");
    }
    if (!requests && maxP99Regression !== undefined) {
      refused.push("a Criterion workload's figures are batch means, never request percentiles: bound its mean with --min-throughput-ratio");
    }
    const ratio = requests ? undefined : currentMetric.throughput / baseMetric.throughput;
    const effectiveMinRatio = requests ? undefined : Number(minRatio ?? 1);
    const exponent = currentMetric.growth_exponent;
    const hasGrowthMetric = typeof exponent === "number";
    const p99Ratio = requests ? metricRatio(currentMetric.p99_ns, baseMetric.p99_ns) : undefined;
    const ratioPassed = requests || (Number.isFinite(ratio) && ratio >= effectiveMinRatio);
    const exponentPassed =
      maxGrowthExponent === undefined
      || (hasGrowthMetric && exponent <= maxGrowthExponent);
    const spawnPassed =
      maxSpawnCount === undefined
      || (typeof currentMetric.spawn_count === "number" && currentMetric.spawn_count <= maxSpawnCount);
    const p99Passed =
      maxP99Regression === undefined
      || !requests
      || (Number.isFinite(p99Ratio) && p99Ratio <= maxP99Regression);
    const passed = refused.length === 0 && ratioPassed && exponentPassed && spawnPassed && p99Passed;
    return {
      workload,
      status: passed ? "passed" : "failed",
      ...(refused.length === 0 ? {} : { refused }),
      ...(requests ? {} : {
        throughput_ratio: ratio,
        min_throughput_ratio: effectiveMinRatio,
      }),
      ...(maxGrowthExponent === undefined || !hasGrowthMetric ? {} : {
        growth_exponent: exponent,
        max_growth_exponent: maxGrowthExponent,
      }),
      ...(maxSpawnCount === undefined ? {} : {
        spawn_count: currentMetric.spawn_count,
        max_spawn_count: maxSpawnCount,
      }),
      ...(maxP99Regression === undefined || !requests ? {} : {
        p99_regression: p99Ratio,
        max_p99_regression: maxP99Regression,
      }),
    };
  });
}

// Why a workload's figures can't be read as what they claim. A Criterion
// workload carries its mean and interval, and a percentile on one is a batch
// average posing as a request's latency. A request workload carries
// individual timings at a held arrival rate, every request completed.
function measurementRefusals(workload, metric) {
  const refused = [];
  if (metric?.source === "criterion") {
    for (const field of ["p50_ns", "p95_ns", "p99_ns"]) {
      if (field in metric) {
        refused.push(`${workload}'s ${field} is a percentile of Criterion's batch averages, not of individual requests`);
      }
    }
    for (const field of ["mean_ns", "mean_lower_ns", "mean_upper_ns"]) {
      if (!(typeof metric[field] === "number" && Number.isFinite(metric[field]))) {
        refused.push(`${workload} has no ${field}: a Criterion workload reports its mean and the mean's interval`);
      }
    }
  } else if (metric?.source === "requests") {
    for (const field of ["arrival_per_second", "requests", "completed", "errors", "timeouts", "p50_ns", "p99_ns"]) {
      if (!(typeof metric[field] === "number" && Number.isFinite(metric[field]))) {
        refused.push(`${workload} has no ${field}: a request workload is timed request by request at a held arrival rate`);
      }
    }
    if (metric.errors > 0 || metric.timeouts > 0) {
      refused.push(`${workload} had ${metric.errors} failed and ${metric.timeouts} timed-out requests of ${metric.requests}`);
    }
  } else {
    refused.push(`${workload} doesn't say how it was measured: its source is criterion or requests`);
  }
  return refused;
}

// performance.md's eight journeys, each with the work budget its row states,
// as the structural measurements a capture's `journeys` holds for it. These
// hold at any speed, so they're checked apart from timing. A journey with no
// measurement fails, and so does any budget whose measurement is missing.
function journeyBudgets() {
  // Journey 4's runs: a script's tool calls, its skill calls, and the calls
  // at a turn whose first call waits on a person; each at 10, 100 and 1,000.
  const LONG_RUNS = ["calls", "children", "waiting_turn"];
  const LONG_RUN_SIZES = ["10", "100", "1000"];
  return [
    {
      journey: 1,
      name: "Cold start",
      budgets: [
        { id: "one_process", says: "One process.", holds: (m) => exactly(m.processes_started, 1) },
        { id: "no_network", says: "No network.", holds: (m) => exactly(m.network_requests, 0) },
        { id: "verified_once_per_identity", says: "Package material resolved and verified once per identity.", holds: (m) => atMost(m.verifications_per_identity, 1) },
        { id: "readiness_ignores_untouched_packages", says: "Readiness doesn't grow with packages it doesn't touch, from one skill to 1,000 packages.", holds: (m) => flatAcross(m.packages_read, ["1", "10", "100", "1000"]) },
      ],
    },
    {
      journey: 2,
      name: "Warm execution",
      budgets: [
        { id: "constant_store_writes_per_call", says: "A constant number of store writes per call, at 10, 100 and 1,000 calls.", holds: (m) => flatAcross(m.store_writes_per_call, ["10", "100", "1000"]) },
        { id: "durable_writes_are_the_calls_that_change_the_world_and_the_hosts_answers", says: "A run's durable writes are its calls that change the world plus the points where it answers its host, at 1, 10, 100 and 1,000 calls.", holds: (m) => everyAt(m.runs, ["1", "10", "100", "1000"], (run) => isCount(run?.calls_changing_the_world) && isCount(run?.host_answers) && exactly(run?.durable_writes, run.calls_changing_the_world + run.host_answers)) },
        { id: "no_context_reserialization", says: "No whole-context clone or re-serialization per call.", holds: (m) => exactly(m.context_reserializations_per_call, 0) },
        { id: "no_transaction_spans_a_provider_call", says: "No transaction spans a provider call.", holds: (m) => exactly(m.transactions_spanning_provider_calls, 0) },
      ],
    },
    {
      journey: 3,
      name: "Composition",
      budgets: [
        { id: "settlement_flat_at_call_1000", says: "Settling a call costs the same at call 1,000 as at call 1: no replay in normal running. Its record reads, store writes and runs taken whole are the same per call at 10, 100 and 1,000 children.", holds: (m) => ["settle_reads_per_call", "settle_writes_per_call", "settle_replays_per_call"].every((count) => flatAcross(m[count], ["10", "100", "1000"])) },
        { id: "settlement_flat_at_a_waiting_turn", says: "Settling a call costs the same at call 1,000 as at call 1 at an agent turn waiting on a person: the rows its last call reads are the same at 10, 100 and 1,000 calls.", holds: (m) => flatAcross(m.turn_call_rows_read, ["10", "100", "1000"]) },
        { id: "settlement_flat_at_depth_5", says: "Settling a call at depth 5 costs what it does at depth 1: its record reads and store writes are the same per call at depth 1, 3 and 5.", holds: (m) => ["settle_reads_at_depth", "settle_writes_at_depth"].every((count) => flatAcross(m[count], ["1", "3", "5"])) },
        { id: "slow_read_delays_no_other_call", says: "A slow provider read never delays another call recording its result.", holds: (m) => exactly(m.results_delayed_by_slow_read, 0) },
        { id: "concurrency_within_the_runs", says: "Nested concurrency never exceeds the run's max_concurrency.", holds: (m) => atMost(m.peak_concurrency, m.max_concurrency) },
        { id: "nested_scripts_keep_their_concurrency", says: "Nested scripts keep their declared concurrency, drawing from the run's: the calls they have in flight at once reach its max_concurrency.", holds: (m) => exactly(m.peak_concurrency, m.max_concurrency) },
      ],
    },
    {
      journey: 4,
      name: "Long runs",
      budgets: [
        { id: "run_read_costs_what_moved", says: "A run read costs what moved since its cursor, not the tree's size: with nothing new, the same at 10, 100 and 1,000 calls, children, and calls at a held turn.", holds: (m) => LONG_RUNS.every((shape) => flatAcross(m.run_read_rows_with_nothing_new?.[shape], LONG_RUN_SIZES)) },
        { id: "resume_costs_the_same_at_1000", says: "A resume of a run whose settled steps made 1,000 calls or children costs what it does at 10: it reads the run's own events, and the calls of a step still open, never those of a step that settled.", holds: (m) => ["calls", "children"].every((shape) => flatAcross(m.resume_rows_read?.[shape], LONG_RUN_SIZES)) },
        { id: "cancel_costs_the_same_at_1000", says: "A cancel at 1,000 calls costs what it does at 10: the rows a read cancelled while it executes reads are the same at a turn of 10, 100 and 1,000 calls.", holds: (m) => flatAcross(m.cancel_rows_read, LONG_RUN_SIZES) },
        { id: "recovery_reads_each_stream_once", says: "Recovery reads each stream once: a resume reads no event of the run's streams twice.", holds: (m) => exactly(m.recovery_reads_per_stream, 1) },
        { id: "receipt_append_costs_the_same", says: "A receipt append does the same work at 16, 128 and 1,000 receipts.", holds: (m) => ["directory_entries_read", "index_entries_read", "index_entries_written"].every((field) => flatAcross(fieldBySize(m.receipt_append, field), ["16", "128", "1000"])) },
      ],
    },
    {
      journey: 5,
      name: "Memory",
      budgets: [
        { id: "index_reads_what_it_shows", says: "The index reads only the pages its budget reads, counted in rows visited from the query plan: the same at 10,000 entries as at 100,000, and no more than those pages hold with the disabled and expired rows within them. Its standing rules are read from an index of their own, so the rows they cost don't grow with the facts.", holds: (m) => flatAcross(m.index_rows_visited, ["10000", "100000"]) && ["100", "10000", "100000"].every((size) => atMost(m.index_rows_visited?.[size], m.index_rows_its_budget_reads?.[size])) },
        { id: "search_one_page_per_stream", says: "Search reads one page per stream.", holds: (m) => exactly(m.search_pages_per_stream, 1) },
        { id: "search_current_entries_only", says: "Search reads current entries only.", holds: (m) => exactly(m.search_history_rows_read, 0) },
        { id: "focus_one_search_page_per_stream", says: "With a focus on a word every entry holds, the index reads one search page per fact stream.", holds: (m) => exactly(m.index_focus_search_pages_per_stream, 1) },
        { id: "focus_current_entries_only", says: "With a focus, the index reads current entries only.", holds: (m) => exactly(m.index_focus_history_rows_read, 0) },
      ],
    },
    {
      journey: 6,
      name: "The registry",
      budgets: [
        { id: "one_query_per_page", says: "One query per page, never one per listing, at 10, 100 and 1,000 listings.", holds: (m) => everyAt(m.queries_per_page, ["10", "100", "1000"], (value) => exactly(value, 1)) },
        { id: "no_package_body_per_listing", says: "No package body read for a listing.", holds: (m) => exactly(m.package_bodies_read, 0) },
      ],
    },
    {
      journey: 7,
      name: "Artifacts",
      budgets: [
        { id: "bounded_byte_copies", says: "Each byte copied a bounded number of times, the same at 1 KiB, 1 MiB and 64 MiB.", holds: (m) => flatAcross(m.copies_per_byte, ["1KiB", "1MiB", "64MiB"]) },
        { id: "page_read_touches_its_range", says: "A page read touches only its range.", holds: (m) => exactly(m.page_bytes_read_beyond_range, 0) },
      ],
    },
    {
      journey: 8,
      name: "Agent work",
      budgets: [
        { id: "context_reads_what_the_turn_uses", says: "Context preparation reads what the turn can use.", holds: (m) => exactly(m.context_reads_unused, 0) },
        { id: "repeated_reads_no_more_than_before", says: "Repeated reads counted against the task's previous run.", holds: (m) => atMost(m.repeated_reads?.current, m.repeated_reads?.previous) },
        { id: "model_calls_no_more_than_before", says: "Model calls counted against the task's previous run.", holds: (m) => atMost(m.model_calls?.current, m.model_calls?.previous) },
      ],
    },
  ];
}

// One field of each size's counts, by size.
function fieldBySize(bySize, field) {
  return Object.fromEntries(Object.entries(bySize ?? {}).map(([size, counts]) => [size, counts?.[field]]));
}

function isCount(value) {
  return Number.isInteger(value) && value >= 0;
}

function exactly(value, expected) {
  return isCount(value) && value === expected;
}

function atMost(value, bound) {
  return isCount(value) && isCount(bound) && value <= bound;
}

// The same measured count at every size the row names.
function flatAcross(bySize, sizes) {
  return sizes.every((size) => isCount(bySize?.[size]))
    && sizes.every((size) => bySize[size] === bySize[sizes[0]]);
}

function everyAt(bySize, sizes, holds) {
  return sizes.every((size) => holds(bySize?.[size]));
}

function journeyChecks(report) {
  const measured = report?.journeys ?? {};
  return journeyBudgets().flatMap(({ journey, name, budgets }) => {
    const measurement = measured[String(journey)];
    if (!measurement || typeof measurement !== "object") {
      return [{
        id: `journey_${journey}`,
        status: "failed",
        journey,
        name,
        reason: "no measurement: a journey with none never passes",
      }];
    }
    return budgets.map(({ id, says, holds }) => ({
      id: `journey_${journey}.${id}`,
      status: holds(measurement) ? "passed" : "failed",
      journey,
      says,
    }));
  });
}

function runtimeQualityChecks(report, expectedSourceCommit) {
  const native = report?.workloads?.native_capability_dispatch;
  const session = report?.workloads?.pure_module_session_reuse;
  const measurements = Object.entries(report?.workloads ?? {}).map(([workload, metric]) => {
    const refused = measurementRefusals(workload, metric);
    return {
      id: `measurement.${workload}`,
      status: refused.length === 0 ? "passed" : "failed",
      ...(refused.length === 0 ? {} : { refused }),
    };
  });
  return [
    qualityCheck("schema", report?.schema, { expected: schema }),
    qualityCheck("source_commit", report?.source_commit, { expected: expectedSourceCommit }),
    qualityCheck("worktree_clean", report?.worktree_dirty, { expected: false }),
    qualityCheck("native_sample_count", native?.sample_count, { minimum: 10 }),
    qualityCheck("session_sample_count", session?.sample_count, { minimum: 10 }),
    qualityCheck("session_spawn_count", session?.spawn_count, { expected: 1 }),
    qualityCheck("session_peak_in_flight", session?.peak_in_flight, { expected: 1 }),
    ...measurements,
    ...journeyChecks(report),
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
    } else if (arg === "--arrival-rate") {
      parsed.arrivalRate = Number(requiredValue(argv, ++index, arg));
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
