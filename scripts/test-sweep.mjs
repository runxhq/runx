#!/usr/bin/env node
import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

import { bundledCoreSkills } from "./classify-ci-changes.mjs";
import { repositorySkills } from "./lib/skills.mjs";

const schema = "runx.inline_test_sweep.v1";
const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

try {
  const options = parseArgs(process.argv.slice(2));
  if (options.selfTest) {
    await runSelfTests();
    process.stdout.write("test-sweep self-test passed\n");
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
  process.stderr.write(`runx test sweep: ${report.summary}\n`);
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
  const admitted = options.skills ? new Set([...bundledCoreSkills(), ...options.skills]) : null;
  const skills = selectSkills(
    repositorySkills(repoRoot, { ...process.env, RUNX_RUST_CLI_BIN: runxBin }, admitted),
    options.skills,
  );
  const allowed = new Set(options.allowed);
  const scratchRoot = path.join(repoRoot, ".runx", "test-sweep");
  mkdirSync(scratchRoot, { recursive: true });
  const tempRoot = mkdtempSync(path.join(scratchRoot, "run-"));
  const workspaceDir = path.join(tempRoot, "workspace");
  mkdirSync(workspaceDir, { recursive: true });
  const results = [];
  let exercised = new Set();

  try {
    for (const skill of skills) {
      const result = await runSkillTest(
        skill,
        runxBin,
        tempRoot,
        workspaceDir,
        allowed,
      );
      results.push(result);
      const label = result.status === "passed"
        ? "PASS"
        : result.status === "not_declared"
          ? "NONE"
          : result.status === "allowed_failure"
            ? "ALLOW"
            : "FAIL";
      process.stderr.write(`[test-sweep] ${label} ${skill.name} ${result.elapsed_ms}ms\n`);
    }
    exercised = exercisedPacketIds(path.join(tempRoot, "receipts"));
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
  // Every package packet is validated by at least one fixture's run. Only a
  // sweep of every skill can say so.
  const packets = workspacePacketIds();
  const unexercised = packets.filter((id) => !exercised.has(id));
  if (!options.skills && unexercised.length > 0) {
    failures.push(`no fixture validates ${unexercised.length} package packet(s): ${unexercised.join(", ")}`);
  }

  return {
    schema,
    status: failures.length === 0 ? "passed" : "failed",
    summary: `${passedSkillCount} passed, ${notDeclaredSkillCount} internal not declared, ${skills.length} runx-maintained`,
    required,
    expected_skill_count: expectedSkillCount ?? null,
    discovered_skill_count: skills.length,
    passed_skill_count: passedSkillCount,
    not_declared_skill_count: notDeclaredSkillCount,
    failed_skill_count: failed.length,
    allowed_failure_count: allowedFailureCount,
    allowed_failures: [...allowed].sort(),
    packet_count: packets.length,
    exercised_packet_count: packets.length - unexercised.length,
    unexercised_packets: unexercised,
    elapsed_ms: Math.round(performance.now() - started),
    runx_bin: path.relative(repoRoot, runxBin),
    temp_root: options.keepTemp ? tempRoot : undefined,
    failures,
    skills: results,
  };
}

/** The packet ids the receipts under `root` sealed a packet check for. */
function exercisedPacketIds(root) {
  const ids = new Set();
  const visit = (directory) => {
    for (const entry of existsSync(directory) ? readdirSync(directory, { withFileTypes: true }) : []) {
      const entryPath = path.join(directory, entry.name);
      if (entry.isDirectory()) visit(entryPath);
      else if (entry.name.endsWith(".json")) {
        for (const match of readFileSync(entryPath, "utf8").matchAll(/packet_schema_verified:([^:"]+):/gu)) {
          ids.add(match[1]);
        }
      }
    }
  };
  visit(root);
  return ids;
}

/** Every packet a workspace package owns, by id. */
function workspacePacketIds() {
  const skillsRoot = path.join(repoRoot, "skills");
  return readdirSync(skillsRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && existsSync(path.join(skillsRoot, entry.name, "packets")))
    .flatMap((entry) => readdirSync(path.join(skillsRoot, entry.name, "packets"), { withFileTypes: true })
      .filter((file) => file.isFile() && file.name.endsWith(".schema.json"))
      .map((file) => JSON.parse(readFileSync(path.join(skillsRoot, entry.name, "packets", file.name), "utf8"))["x-runx-packet-id"]))
    .sort();
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
      `expected ${expectedSkillCount} runx-maintained skills, discovered ${discoveredSkillCount}`,
    );
  }
  if (gating && passedSkillCount < required) {
    failures.push(`required ${required} passing skills, got ${passedSkillCount}`);
  }
  if (failed.length > 0) {
    failures.push(
      `unallowed test failures: ${failed.map((result) => result.skill).join(", ")}`,
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
    ["unallowed test failures: broken"],
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
    ["expected 3 runx-maintained skills, discovered 2"],
    "--expected-count must catch catalog drift",
  );
  // POSIX process groups are owned directly by this sweep. On Windows, the
  // spawned Runx binary owns the kernel Job Object and its focused Rust tests
  // exercise descendant containment on a real Windows method.
  if (process.platform !== "win32") {
    // A root that exits while a descendant still holds its pipes: the sweep
    // ends the root's process group and returns once the pipes close, which
    // happens only when the descendant has died.
    const tree = await spawnCaptured(
      process.execPath,
      [
        "-e",
        [
          "const { spawn } = require('node:child_process');",
          "spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'],",
          "  { stdio: ['ignore', 'inherit', 'inherit'] }).unref();",
          "process.stdout.write('started\\n');",
        ].join(" "),
      ],
      { encoding: "utf8" },
    );
    if (tree.error || tree.status !== 0 || tree.stdout !== "started\n") {
      throw new Error(
        `a package's descendant must be ended with its root: ${JSON.stringify(tree)}`,
      );
    }
  }
  const internalEmpty = classifyTestResult({
    exitStatus: 0,
    reportStatus: "not_declared",
    caseCount: 0,
    visibility: "internal",
    allowed: false,
  });
  if (internalEmpty !== "not_declared") {
    throw new Error(`internal no-case package must remain not_declared, got ${internalEmpty}`);
  }
  const publicEmpty = classifyTestResult({
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

async function runSkillTest(skill, runxBin, tempRoot, workspaceDir, allowed) {
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
  const result = await spawnCaptured(
    runxBin,
    ["test", skillDir, "--json", "--receipt-dir", receiptDir],
    {
      cwd: skillWorkspaceDir,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      env: testEnv(runxBin, tempRoot, skillWorkspaceDir),
    },
  );
  const elapsedMs = Math.round(performance.now() - started);
  const report = parseTestReport(result.stdout);
  const error = result.error
    ? result.error.message
    : nonEmpty(result.stderr)
      ?? report.parse_error
      ?? (result.status === 0 ? undefined : `runx exited ${result.status ?? "with signal"}`);
  const caseCount = report.case_count ?? 0;
  const status = classifyTestResult({
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
    workflow_case_count: report.workflow_case_count ?? 0,
    assertion_error_count: report.assertion_error_count ?? 0,
    assertion_errors: report.assertion_errors ?? [],
    case_names: report.case_names ?? [],
    receipt_count: Array.isArray(report.receipt_ids) ? report.receipt_ids.length : 0,
    error: ["passed", "not_declared"].includes(status) ? undefined : error,
  };
}

function classifyTestResult({
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

/**
 * Run a package's tests to their end, capturing at most `maxBuffer` bytes of
 * each stream. A run that writes more is ended with its process group. A run
 * whose root exits while a descendant still holds its pipes has that group
 * ended too, and returns once the pipes close.
 */
async function spawnCaptured(command, args, options) {
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
  const outcome = await Promise.race([completion, overflow]);

  let error = spawnError;
  if (outcome !== "completed" || !rootClosed) {
    try {
      terminateSpawnedProcess(child);
    } catch (candidate) {
      error = candidate;
    }
    await rootClose;
  }
  if (!error && outcome === "overflow") {
    error = overflowError;
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

function failedSkill(skill, started, error) {
  return {
    skill,
    status: "failed",
    elapsed_ms: Math.round(performance.now() - started),
    exit_status: null,
    case_count: 0,
    workflow_case_count: 0,
    assertion_error_count: 0,
    assertion_errors: [],
    case_names: [],
    receipt_count: 0,
    error,
  };
}

function resolveRunxBinary(options) {
  const explicit = options.runxBin
    ?? process.env.RUNX_TEST_SWEEP_RUNX_BIN
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

// The named skills and every core skill whose methods or context name one of
// them, by package path or registry ref: a change to a skill is proven by its
// own tests and its core dependants', not the whole catalog.
function selectSkills(skills, names) {
  if (!names) return skills;
  const selected = new Set(names);
  const core = new Set(bundledCoreSkills());
  for (let grew = true; grew;) {
    grew = false;
    for (const skill of skills) {
      if (selected.has(skill.name) || !core.has(skill.name)) continue;
      const profile = path.join(repoRoot, "skills", skill.name, "run.yaml");
      const text = existsSync(profile) ? readFileSync(profile, "utf8") : "";
      if ([...selected].some((name) => new RegExp(`(?:\\.\\./|registry:runx/)${escapeRegExp(name)}(?![\\w-])`, "u").test(text))) {
        selected.add(skill.name);
        grew = true;
      }
    }
  }
  return skills.filter((skill) => selected.has(skill.name));
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

function testEnv(runxBin, tempRoot, workspaceDir) {
  const runxHome = path.join(tempRoot, "runx-home");
  mkdirSync(runxHome, { recursive: true });
  const toolRoots = testToolRoots();
  return {
    ...process.env,
    NO_COLOR: "1",
    RUNX_HOME: runxHome,
    RUNX_CWD: workspaceDir,
    RUNX_RUST_CLI_BIN: runxBin,
    RUNX_TOOL_ROOTS: process.env.RUNX_TOOL_ROOTS
      ? `${process.env.RUNX_TOOL_ROOTS}${path.delimiter}${toolRoots}`
      : toolRoots,
  };
}

function testToolRoots() {
  const skillsRoot = path.join(repoRoot, "skills");
  return [
    path.join(repoRoot, "tools"),
    ...readdirSync(skillsRoot)
      .filter((name) => existsSync(path.join(skillsRoot, name, "SKILL.md")))
      .sort()
      .map((name) => path.join(skillsRoot, name, "tools"))
      .filter(existsSync),
  ].join(path.delimiter);
}

function parseTestReport(stdout) {
  const text = stdout.trim();
  if (!text) {
    return { parse_error: "runx produced no JSON on stdout" };
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    return {
      parse_error: `invalid test JSON: ${error instanceof Error ? error.message : String(error)}`,
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
    } else if (arg === "--skills") {
      options.skills = requiredValue(argv, ++index, arg).split(/[,\s]+/u).filter(Boolean);
    } else if (arg === "--no-build") {
      options.noBuild = true;
    } else if (arg === "--keep-temp") {
      options.keepTemp = true;
    } else if (arg === "--self-test") {
      options.selfTest = true;
    } else if (arg === "--help" || arg === "-h") {
      throw new Error("usage: node scripts/test-sweep.mjs [--skills skill[,skill]] [--require n] [--allow skill[,skill]] [--expected-count n] [--output path] [--runx-bin path] [--no-build] [--keep-temp] [--self-test]");
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
