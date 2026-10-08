#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const schema = "runx.inline_harness_sweep.v1";
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const defaultPackageTimeoutMs = 120_000;

try {
  const options = parseArgs(process.argv.slice(2));
  if (options.selfTest) {
    await runSelfTests();
    process.stdout.write("harness-sweep self-test passed\n");
    process.exit(0);
  }
  const report = await runSweep(options);
  const json = `${JSON.stringify(report, null, 2)}\n`;
  if (options.output) {
    const outputPath = path.resolve(repoRoot, options.output);
    mkdirSync(path.dirname(outputPath), { recursive: true });
    writeFileSync(outputPath, json);
  }
  process.stdout.write(json);
  process.stderr.write(`runx harness sweep: ${report.summary}\n`);
  if (report.status !== "passed") {
    process.exitCode = 1;
  }
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}

async function runSweep(options) {
  const started = performance.now();
  const runxBin = resolveRunxBinary(options);
  const skills = officialSkills();
  const allowed = new Set(options.allowed);
  const scratchRoot = path.join(repoRoot, ".runx", "harness-sweep");
  mkdirSync(scratchRoot, { recursive: true });
  const tempRoot = mkdtempSync(path.join(scratchRoot, "run-"));
  const workspaceDir = path.join(tempRoot, "workspace");
  mkdirSync(workspaceDir, { recursive: true });
  const results = [];

  try {
    for (const skill of skills) {
      const result = await runSkillHarness(
        skill,
        runxBin,
        tempRoot,
        workspaceDir,
        allowed,
        options.timeoutMs ?? defaultPackageTimeoutMs,
      );
      results.push(result);
      const label = result.status === "passed"
        ? "PASS"
        : result.status === "not_declared"
          ? "NONE"
          : result.status === "allowed_failure"
            ? "ALLOW"
            : "FAIL";
      process.stderr.write(`[harness-sweep] ${label} ${skill.name} ${result.elapsed_ms}ms\n`);
    }
  } finally {
    if (!options.keepTemp) {
      rmSync(tempRoot, { recursive: true, force: true });
    }
  }

  const passedSkillCount = results.filter((result) => result.status === "passed").length;
  const notDeclaredSkillCount = results.filter(
    (result) => result.status === "not_declared",
  ).length;
  const allowedFailureCount = results.filter((result) => result.status === "allowed_failure").length;
  const failed = results.filter((result) => result.status === "failed");
  const required = options.require ?? 0;
  const gating = options.require !== undefined;
  const expectedSkillCount = options.expectedCount;
  const failures = sweepFailures({
    discoveredSkillCount: skills.length,
    expectedSkillCount,
    failed,
    gating,
    passedSkillCount,
    required,
  });

  return {
    schema,
    status: failures.length === 0 ? "passed" : "failed",
    summary: `${passedSkillCount} passed, ${notDeclaredSkillCount} internal not declared, ${skills.length} official`,
    required,
    expected_skill_count: expectedSkillCount ?? null,
    discovered_skill_count: skills.length,
    passed_skill_count: passedSkillCount,
    not_declared_skill_count: notDeclaredSkillCount,
    failed_skill_count: failed.length,
    allowed_failure_count: allowedFailureCount,
    allowed_failures: [...allowed].sort(),
    elapsed_ms: Math.round(performance.now() - started),
    package_timeout_ms: options.timeoutMs ?? defaultPackageTimeoutMs,
    runx_bin: path.relative(repoRoot, runxBin),
    temp_root: options.keepTemp ? tempRoot : undefined,
    failures,
    skills: results,
  };
}

function sweepFailures({
  discoveredSkillCount,
  expectedSkillCount,
  failed,
  gating,
  passedSkillCount,
  required,
}) {
  const failures = [];
  if (expectedSkillCount !== undefined && discoveredSkillCount !== expectedSkillCount) {
    failures.push(
      `expected ${expectedSkillCount} official skills, discovered ${discoveredSkillCount}`,
    );
  }
  if (gating && passedSkillCount < required) {
    failures.push(`required ${required} passing skills, got ${passedSkillCount}`);
  }
  if (failed.length > 0) {
    failures.push(
      `unallowed harness failures: ${failed.map((result) => result.skill).join(", ")}`,
    );
  }
  return failures;
}

async function runSelfTests() {
  const defaults = {
    discoveredSkillCount: 2,
    expectedSkillCount: undefined,
    gating: false,
    passedSkillCount: 1,
    required: 0,
  };
  assertFailures(
    sweepFailures({ ...defaults, failed: [{ skill: "broken" }] }),
    ["unallowed harness failures: broken"],
    "an unallowed failure must fail an ungated sweep",
  );
  assertFailures(
    sweepFailures({ ...defaults, failed: [] }),
    [],
    "a clean ungated sweep must pass",
  );
  assertFailures(
    sweepFailures({ ...defaults, failed: [], gating: true, required: 2 }),
    ["required 2 passing skills, got 1"],
    "--require must enforce the minimum passing count",
  );
  assertFailures(
    sweepFailures({ ...defaults, failed: [], expectedSkillCount: 3 }),
    ["expected 3 official skills, discovered 2"],
    "--expected-count must catch catalog drift",
  );
  const started = performance.now();
  const timedOut = await spawnBounded(
    process.execPath,
    ["-e", "setInterval(() => {}, 1000)"],
    { encoding: "utf8" },
    50,
  );
  const elapsedMs = performance.now() - started;
  if (timedOut.error?.code !== "ETIMEDOUT") {
    throw new Error(
      `bounded package execution must time out: ${timedOut.error?.code ?? "no error"}`,
    );
  }
  if (elapsedMs > 2_000) {
    throw new Error(`bounded package execution exceeded its watchdog: ${elapsedMs}ms`);
  }
  // POSIX process groups are owned directly by this Node watchdog. On Windows,
  // the spawned Runx binary owns the kernel Job Object and its focused Rust
  // tests exercise descendant containment on a real Windows runner.
  if (process.platform !== "win32") {
    const treeStarted = performance.now();
    const tree = await spawnBounded(
      process.execPath,
      [
        "-e",
        [
          "const { spawn } = require('node:child_process');",
          "const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'],",
          "  { stdio: ['ignore', 'inherit', 'inherit'] });",
          "process.stdout.write(`${child.pid}\\n`);",
          "setInterval(() => {}, 1000);",
        ].join(" "),
      ],
      { encoding: "utf8" },
      250,
    );
    const treeElapsedMs = performance.now() - treeStarted;
    if (tree.error?.code !== "ETIMEDOUT") {
      throw new Error(
        `bounded process-tree execution must time out: ${tree.error?.code ?? "no error"}`,
      );
    }
    if (treeElapsedMs > 5_000) {
      throw new Error(`process-tree cleanup exceeded its watchdog: ${treeElapsedMs}ms`);
    }
    const descendantPid = Number.parseInt(tree.stdout, 10);
    if (!Number.isInteger(descendantPid)) {
      throw new Error("bounded package execution did not report its descendant");
    }
    if (!(await processExited(descendantPid, 2_000))) {
      throw new Error(`timed-out package left descendant ${descendantPid} alive`);
    }
  }
  const internalEmpty = classifyHarnessResult({
    exitStatus: 0,
    reportStatus: "not_declared",
    caseCount: 0,
    visibility: "internal",
    allowed: false,
  });
  if (internalEmpty !== "not_declared") {
    throw new Error(`internal no-case package must remain not_declared, got ${internalEmpty}`);
  }
  const publicEmpty = classifyHarnessResult({
    exitStatus: 0,
    reportStatus: "not_declared",
    caseCount: 0,
    visibility: "public",
    allowed: false,
  });
  if (publicEmpty !== "failed") {
    throw new Error(`public no-case package must fail admission, got ${publicEmpty}`);
  }
}

function assertFailures(actual, expected, message) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`${message}: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`);
  }
}

async function runSkillHarness(skill, runxBin, tempRoot, workspaceDir, allowed, timeoutMs) {
  const started = performance.now();
  const skillDir = path.join(repoRoot, "skills", skill.name);
  const receiptDir = path.join(tempRoot, "receipts", skill.name);
  const skillWorkspaceDir = path.join(workspaceDir, skill.name);
  mkdirSync(receiptDir, { recursive: true });
  mkdirSync(skillWorkspaceDir, { recursive: true });

  if (!existsSync(path.join(skillDir, "SKILL.md"))) {
    return failedSkill(skill.name, started, "missing SKILL.md");
  }
  if (!existsSync(path.join(skillDir, "run.yaml"))) {
    return failedSkill(skill.name, started, "missing run.yaml");
  }
  const result = await spawnBounded(
    runxBin,
    ["harness", skillDir, "--json", "--receipt-dir", receiptDir],
    {
      cwd: skillWorkspaceDir,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      env: harnessEnv(runxBin, tempRoot, skillWorkspaceDir),
    },
    timeoutMs,
  );
  const elapsedMs = Math.round(performance.now() - started);
  const report = parseHarnessReport(result.stdout);
  const error = result.error
    ? result.error.message
    : nonEmpty(result.stderr)
      ?? report.parse_error
      ?? (result.status === 0 ? undefined : `runx exited ${result.status ?? "with signal"}`);
  const caseCount = report.case_count ?? 0;
  const status = classifyHarnessResult({
    exitStatus: result.status,
    reportStatus: report.status,
    caseCount,
    visibility: skill.visibility,
    allowed: allowed.has(skill.name),
  });
  return {
    skill: skill.name,
    visibility: skill.visibility,
    status,
    elapsed_ms: elapsedMs,
    exit_status: result.status,
    case_count: caseCount,
    graph_case_count: report.graph_case_count ?? 0,
    assertion_error_count: report.assertion_error_count ?? 0,
    assertion_errors: report.assertion_errors ?? [],
    case_names: report.case_names ?? [],
    receipt_count: Array.isArray(report.receipt_ids) ? report.receipt_ids.length : 0,
    error: ["passed", "not_declared"].includes(status) ? undefined : error,
  };
}

function classifyHarnessResult({
  exitStatus,
  reportStatus,
  caseCount,
  visibility,
  allowed,
}) {
  if (exitStatus === 0 && reportStatus === "passed" && caseCount > 0) {
    return "passed";
  }
  if (
    exitStatus === 0
    && reportStatus === "not_declared"
    && caseCount === 0
    && visibility === "internal"
  ) {
    return "not_declared";
  }
  return allowed ? "allowed_failure" : "failed";
}

async function spawnBounded(command, args, options, timeoutMs) {
  const {
    encoding,
    maxBuffer = 1024 * 1024,
    ...spawnOptions
  } = options;
  const child = spawn(command, args, {
    ...spawnOptions,
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
  });
  const stdout = [];
  const stderr = [];
  let stdoutBytes = 0;
  let stderrBytes = 0;
  let overflowError;
  let signalOverflow;
  const overflow = new Promise((resolve) => {
    signalOverflow = resolve;
  });

  const capture = (stream, chunks, streamName) => {
    stream.on("data", (value) => {
      const chunk = Buffer.isBuffer(value) ? value : Buffer.from(value);
      const nextBytes = streamName === "stdout"
        ? stdoutBytes + chunk.length
        : stderrBytes + chunk.length;
      if (streamName === "stdout") {
        stdoutBytes = nextBytes;
      } else {
        stderrBytes = nextBytes;
      }
      if (nextBytes <= maxBuffer) {
        chunks.push(chunk);
        return;
      }
      if (!overflowError) {
        overflowError = processError(
          "ENOBUFS",
          `${command} ${streamName} exceeded maxBuffer (${maxBuffer} bytes)`,
        );
        signalOverflow("overflow");
      }
    });
  };
  capture(child.stdout, stdout, "stdout");
  capture(child.stderr, stderr, "stderr");

  let status = null;
  let signal = null;
  let spawnError;
  let rootClosed = false;
  let resolveRootClosed;
  const rootClose = new Promise((resolve) => {
    resolveRootClosed = resolve;
    child.once("close", (code, closeSignal) => {
      rootClosed = true;
      if (status === null) {
        status = code;
        signal = closeSignal;
      }
      resolve();
    });
  });
  let completionSettled = false;
  let resolveCompletion;
  const completion = new Promise((resolve) => {
    resolveCompletion = resolve;
  });
  const finish = (outcome) => {
    if (completionSettled) return;
    completionSettled = true;
    resolveCompletion(outcome);
  };
  child.once("error", (error) => {
    spawnError = error;
    resolveRootClosed();
    finish("spawn_error");
  });
  child.once("exit", (code, exitSignal) => {
    status = code;
    signal = exitSignal;
    finish("completed");
  });
  let timeoutHandle;
  const timedOut = new Promise((resolve) => {
    timeoutHandle = setTimeout(() => resolve("timeout"), timeoutMs);
  });
  const outcome = await Promise.race([completion, overflow, timedOut]);
  clearTimeout(timeoutHandle);

  let error = spawnError;
  let cleanupError;
  if (outcome !== "completed" || !rootClosed) {
    try {
      terminateSpawnedProcess(child);
    } catch (candidate) {
      cleanupError = candidate;
    }
    await Promise.race([rootClose, delay(1_000)]);
    if (!rootClosed) {
      try {
        child.kill("SIGKILL");
      } catch (candidate) {
        if (!isMissingProcess(candidate) && !cleanupError) {
          cleanupError = candidate;
        }
      }
      await Promise.race([rootClose, delay(1_000)]);
    }
    child.stdout.destroy();
    child.stderr.destroy();
    child.unref();
  }
  if (cleanupError) {
    error = cleanupError;
  } else if (!error && outcome !== "completed") {
    error = outcome === "timeout"
      ? processError("ETIMEDOUT", `${command} timed out after ${timeoutMs}ms`)
      : overflowError;
  }

  return {
    pid: child.pid,
    status,
    signal,
    stdout: decodeOutput(stdout, encoding),
    stderr: decodeOutput(stderr, encoding),
    error,
  };
}

function terminateSpawnedProcess(child) {
  if (!Number.isInteger(child.pid)) {
    return;
  }
  if (process.platform === "win32") {
    // Runx owns its descendants with a kernel Job Object. Killing the Runx
    // root closes the outer job handle and atomically terminates that tree.
    child.kill("SIGKILL");
    return;
  }
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch (error) {
    if (!isMissingProcess(error)) throw error;
  }
}

async function processExited(pid, timeoutMs) {
  const deadline = performance.now() + timeoutMs;
  while (performance.now() < deadline) {
    try {
      process.kill(pid, 0);
    } catch (error) {
      if (isMissingProcess(error)) return true;
      throw error;
    }
    await delay(25);
  }
  return false;
}

function isMissingProcess(error) {
  return error instanceof Error && "code" in error && error.code === "ESRCH";
}

function processError(code, message) {
  const error = new Error(message);
  error.code = code;
  return error;
}

function decodeOutput(chunks, encoding) {
  const output = Buffer.concat(chunks);
  return encoding ? output.toString(encoding) : output;
}

function delay(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function failedSkill(skill, started, error) {
  return {
    skill,
    status: "failed",
    elapsed_ms: Math.round(performance.now() - started),
    exit_status: null,
    case_count: 0,
    graph_case_count: 0,
    assertion_error_count: 0,
    assertion_errors: [],
    case_names: [],
    receipt_count: 0,
    error,
  };
}

function resolveRunxBinary(options) {
  const explicit = options.runxBin
    ?? process.env.RUNX_HARNESS_SWEEP_RUNX_BIN
    ?? process.env.RUNX_RUST_CLI_BIN;
  if (explicit) {
    const resolved = path.resolve(repoRoot, explicit);
    if (!existsSync(resolved)) {
      throw new Error(`runx binary does not exist: ${resolved}`);
    }
    return resolved;
  }
  if (!options.noBuild) {
    const result = spawnSync(
      process.platform === "win32" ? "cargo.exe" : "cargo",
      [
        "build",
        "--quiet",
        "--manifest-path",
        "crates/Cargo.toml",
        "-p",
        "runx-cli",
        "-p",
        "runx-js-worker",
        "--bins",
      ],
      {
        cwd: repoRoot,
        stdio: "inherit",
        env: { ...process.env, CARGO_TERM_COLOR: process.env.CARGO_TERM_COLOR ?? "never" },
      },
    );
    if (result.status !== 0) {
      throw new Error(`cargo build runx and runx-js-worker failed with exit ${result.status ?? "signal"}`);
    }
  }
  const targetRoot = process.env.CARGO_TARGET_DIR
    ? path.resolve(repoRoot, process.env.CARGO_TARGET_DIR)
    : path.join(repoRoot, "crates", "target");
  const binary = path.join(targetRoot, "debug", process.platform === "win32" ? "runx.exe" : "runx");
  if (!existsSync(binary)) {
    throw new Error(`runx binary does not exist after build: ${binary}`);
  }
  return binary;
}

function officialSkills() {
  const lockPath = path.join(repoRoot, "skills", "official.lock.json");
  const lock = JSON.parse(readFileSync(lockPath, "utf8"));
  if (!Array.isArray(lock)) {
    throw new Error("official skills lock is not an array");
  }
  return lock
    .map((entry) => {
      const parts = typeof entry?.skill_id === "string" ? entry.skill_id.split("/") : [];
      if (parts.length !== 2 || !parts[0] || !parts[1]) {
        throw new Error(`invalid official skill entry: ${JSON.stringify(entry)}`);
      }
      if (!["internal", "public"].includes(entry.catalog_visibility)) {
        throw new Error(`invalid official skill visibility: ${JSON.stringify(entry)}`);
      }
      return {
        name: parts[1],
        visibility: entry.catalog_visibility,
      };
    })
    .sort((left, right) => left.name.localeCompare(right.name));
}

function harnessEnv(runxBin, tempRoot, workspaceDir) {
  const runxHome = path.join(tempRoot, "runx-home");
  mkdirSync(runxHome, { recursive: true });
  const toolRoots = harnessToolRoots();
  return {
    ...process.env,
    NO_COLOR: "1",
    RUNX_HOME: runxHome,
    RUNX_CWD: workspaceDir,
    RUNX_RUST_CLI_BIN: runxBin,
    RUNX_TOOL_ROOTS: process.env.RUNX_TOOL_ROOTS
      ? `${process.env.RUNX_TOOL_ROOTS}${path.delimiter}${toolRoots}`
      : toolRoots,
    RUNX_RECEIPT_SIGN_KID: process.env.RUNX_RECEIPT_SIGN_KID ?? "harness-sweep-test-key",
    RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64:
      process.env.RUNX_RECEIPT_SIGN_ED25519_SEED_BASE64
        ?? "QkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkI=",
    RUNX_RECEIPT_SIGN_ISSUER_TYPE: process.env.RUNX_RECEIPT_SIGN_ISSUER_TYPE ?? "hosted",
  };
}

function harnessToolRoots() {
  return [
    path.join(repoRoot, "tools"),
    ...officialSkills()
      .map((skill) => path.join(repoRoot, "skills", skill.name, "tools"))
      .filter(existsSync),
  ].join(path.delimiter);
}

function parseHarnessReport(stdout) {
  const text = stdout.trim();
  if (!text) {
    return { parse_error: "runx produced no JSON on stdout" };
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    return {
      parse_error: `invalid harness JSON: ${error instanceof Error ? error.message : String(error)}`,
    };
  }
}

function parseArgs(argv) {
  const options = {
    allowed: [],
  };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--require") {
      options.require = positiveInteger(requiredValue(argv, ++index, arg), arg);
    } else if (arg === "--allow") {
      options.allowed.push(...requiredValue(argv, ++index, arg).split(",").filter(Boolean));
    } else if (arg === "--expected-count") {
      options.expectedCount = positiveInteger(requiredValue(argv, ++index, arg), arg);
    } else if (arg === "--output") {
      options.output = requiredValue(argv, ++index, arg);
    } else if (arg === "--runx-bin") {
      options.runxBin = requiredValue(argv, ++index, arg);
    } else if (arg === "--timeout-ms") {
      options.timeoutMs = positiveInteger(requiredValue(argv, ++index, arg), arg);
      if (options.timeoutMs === 0) {
        throw new Error("--timeout-ms requires a positive integer");
      }
    } else if (arg === "--no-build") {
      options.noBuild = true;
    } else if (arg === "--keep-temp") {
      options.keepTemp = true;
    } else if (arg === "--self-test") {
      options.selfTest = true;
    } else if (arg === "--help" || arg === "-h") {
      throw new Error("usage: node scripts/harness-sweep.mjs [--require n] [--allow skill[,skill]] [--expected-count n] [--output path] [--runx-bin path] [--timeout-ms n] [--no-build] [--keep-temp] [--self-test]");
    } else {
      throw new Error(`unknown argument '${arg}'`);
    }
  }
  return options;
}

function requiredValue(argv, index, flag) {
  const value = argv[index];
  if (!value || value.startsWith("--")) {
    throw new Error(`${flag} requires a value`);
  }
  return value;
}

function positiveInteger(value, flag) {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 0) {
    throw new Error(`${flag} requires a non-negative integer`);
  }
  return parsed;
}

function nonEmpty(value) {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : undefined;
}
