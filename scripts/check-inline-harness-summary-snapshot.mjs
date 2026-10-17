#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const defaultSkill = "code-intake";
const snapshotPath = path.join(
  repoRoot,
  "fixtures",
  "harness",
  "oracle",
  "inline-summary.issue-intake.json",
);

try {
  const options = parseArgs(process.argv.slice(2));
  const snapshot = captureSnapshot(options.skill ?? defaultSkill, options);
  const json = `${JSON.stringify(snapshot, null, 2)}\n`;
  if (options.write) {
    mkdirSync(path.dirname(snapshotPath), { recursive: true });
    writeFileSync(snapshotPath, json);
    process.stdout.write(`wrote ${path.relative(repoRoot, snapshotPath)}\n`);
  } else {
    const expected = readFileSync(snapshotPath, "utf8");
    if (expected !== json) {
      process.stderr.write(
        `inline harness summary snapshot is stale: ${path.relative(repoRoot, snapshotPath)}\n`
          + "run `node scripts/check-inline-harness-summary-snapshot.mjs --write` to regenerate\n",
      );
      process.exitCode = 1;
    }
  }
} catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
}

function captureSnapshot(skill, options) {
  const runxBin = resolveRunxBinary(options);
  const tempRoot = mkdtempSync(path.join(os.tmpdir(), "runx-inline-harness-snapshot-"));
  const workspaceDir = path.join(tempRoot, "workspace");
  mkdirSync(workspaceDir, { recursive: true });
  try {
    const receiptDir = path.join(tempRoot, "receipts");
    mkdirSync(receiptDir, { recursive: true });
    const result = spawnSync(
      runxBin,
      ["harness", path.join(repoRoot, "skills", skill), "--json", "--receipt-dir", receiptDir],
      {
        cwd: workspaceDir,
        encoding: "utf8",
        maxBuffer: 32 * 1024 * 1024,
        env: harnessEnv(runxBin, tempRoot, workspaceDir),
      },
    );
    if (result.status !== 0) {
      throw new Error(
        `runx harness ${skill} failed with exit ${result.status ?? "signal"}: ${result.stderr.trim()}`,
      );
    }
    return {
      schema: "runx.inline_harness_report_snapshot.v1",
      skill,
      report: normalizeReport(JSON.parse(result.stdout)),
    };
  } finally {
    rmSync(tempRoot, { recursive: true, force: true });
  }
}

function normalizeReport(report) {
  return {
    status: report.status,
    case_count: report.case_count,
    assertion_error_count: report.assertion_error_count,
    assertion_errors: report.assertion_errors,
    case_names: report.case_names,
    receipt_ids: Array.isArray(report.receipt_ids)
      ? report.receipt_ids.map((_, index) => `<receipt:${index + 1}>`)
      : [],
    graph_case_count: report.graph_case_count,
  };
}

function resolveRunxBinary(options) {
  const explicit = options.runxBin ?? process.env.RUNX_RUST_CLI_BIN;
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

function harnessEnv(runxBin, tempRoot, workspaceDir) {
  return {
    ...process.env,
    NO_COLOR: "1",
    RUNX_HOME: path.join(tempRoot, "runx-home"),
    RUNX_CWD: workspaceDir,
    RUNX_RUST_CLI_BIN: runxBin,
  };
}

function parseArgs(argv) {
  const options = {};
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (arg === "--write") {
      options.write = true;
    } else if (arg === "--skill") {
      options.skill = requiredValue(argv, ++index, arg);
    } else if (arg === "--runx-bin") {
      options.runxBin = requiredValue(argv, ++index, arg);
    } else if (arg === "--no-build") {
      options.noBuild = true;
    } else if (arg === "--help" || arg === "-h") {
      throw new Error("usage: node scripts/check-inline-harness-summary-snapshot.mjs [--write] [--skill name] [--runx-bin path] [--no-build]");
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
