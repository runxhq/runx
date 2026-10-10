import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import process from "node:process";

const repoRoot = path.resolve(import.meta.dirname, "..");
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const oracleBinName = process.platform === "win32"
  ? "runx-harness-fixture-oracles.exe"
  : "runx-harness-fixture-oracles";
const defaultOracleBin = path.join(repoRoot, "crates", "target", "debug", oracleBinName);

const forwardedArgs = [...process.argv.slice(2), "--repo-root", repoRoot];
const summaryJson = process.argv.includes("--summary-json");

if (process.env.RUNX_HARNESS_FIXTURE_ORACLE_BIN) {
  run(process.env.RUNX_HARNESS_FIXTURE_ORACLE_BIN, forwardedArgs);
} else if (existsSync(defaultOracleBin) && !summaryJson) {
  run(defaultOracleBin, forwardedArgs);
} else {
  run(cargo, [
    "run",
    "--quiet",
    "--manifest-path",
    path.join(repoRoot, "crates", "Cargo.toml"),
    "-p",
    "runx-runtime",
    "--bin",
    "runx-harness-fixture-oracles",
    "--",
    ...forwardedArgs,
  ]);
}

function run(command: string, args: readonly string[]): void {
  const result = spawnSync(command, args, {
    cwd: repoRoot,
    env: process.env,
    stdio: "inherit",
  });
  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
