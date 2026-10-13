#!/usr/bin/env node
import { spawn } from "node:child_process";
import os from "node:os";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cargo = process.platform === "win32" ? "cargo.exe" : "cargo";
const cargoTargetDir = process.env.CARGO_TARGET_DIR
  ? path.resolve(workspaceRoot, process.env.CARGO_TARGET_DIR)
  : path.join(workspaceRoot, "crates", "target");
const rustKernelBin = path.join(
  cargoTargetDir,
  "debug",
  process.platform === "win32" ? "runx.exe" : "runx",
);
const rustHarnessFixtureOracleBin = path.join(
  cargoTargetDir,
  "debug",
  "examples",
  process.platform === "win32" ? "runx-harness-fixture-oracles.exe" : "runx-harness-fixture-oracles",
);
const rustCapabilitySnapshotBin = path.join(
  cargoTargetDir,
  "debug",
  "examples",
  process.platform === "win32"
    ? "runx-native-capability-snapshot.exe"
    : "runx-native-capability-snapshot",
);
const rustSchemaArtifactsBin = path.join(
  cargoTargetDir,
  "debug",
  "examples",
  process.platform === "win32" ? "runx-schema-artifacts.exe" : "runx-schema-artifacts",
);
const rustPaidInvocationFixturesBin = path.join(
  cargoTargetDir,
  "debug",
  "examples",
  process.platform === "win32"
    ? "runx-paid-invocation-fixtures.exe"
    : "runx-paid-invocation-fixtures",
);
const rustPrincipalIdFixturesBin = path.join(
  cargoTargetDir,
  "debug",
  "examples",
  process.platform === "win32"
    ? "runx-principal-id-fixtures.exe"
    : "runx-principal-id-fixtures",
);
const rustX402FixturesBin = path.join(
  cargoTargetDir,
  "debug",
  "examples",
  process.platform === "win32" ? "runx-x402-fixtures.exe" : "runx-x402-fixtures",
);

const evalBinEnv = {
  RUNX_RUST_CLI_BIN: rustKernelBin,
  RUNX_HARNESS_FIXTURE_ORACLE_BIN: rustHarnessFixtureOracleBin,
  RUNX_CAPABILITY_SNAPSHOT_BIN: rustCapabilitySnapshotBin,
  RUNX_SCHEMA_ARTIFACTS_BIN: rustSchemaArtifactsBin,
  RUNX_PAID_INVOCATION_FIXTURES_BIN: rustPaidInvocationFixturesBin,
  RUNX_PRINCIPAL_ID_FIXTURES_BIN: rustPrincipalIdFixturesBin,
  RUNX_X402_FIXTURES_BIN: rustX402FixturesBin,
};
const rustBuildEnv = {
  CARGO_BUILD_JOBS: process.env.CARGO_BUILD_JOBS ?? defaultCargoBuildJobs(),
};

const results = [];

await runParallelGroup("source checks", [
  step("readiness structural guard", "node", ["scripts/check-readiness-structural.mjs"]),
  step("demo inventory guard", "node", ["scripts/check-demo-inventory.mjs"]),
  step("boundary:check", "pnpm", ["boundary:check"]),
  step("license manifest", "node", ["scripts/check-license-edges.mjs", "--check", "manifest-complete"]),
  step("license identifiers", "node", ["scripts/check-license-edges.mjs", "--check", "identifiers"]),
  step("runtime architecture", "pnpm", ["runtime:architecture-check"]),
  step("test:boundary", "pnpm", ["test:boundary"]),
  step("test:skills", "pnpm", ["test:skills"]),
  step("typecheck", "pnpm", ["typecheck"]),
  step("bindings:check", "pnpm", ["bindings:check"]),
  step("public domain URLs", "pnpm", ["domains:check"]),
  step("release version sync", "pnpm", ["release:version:check"]),
  step("deterministic module engine decision", "node", [
    "scripts/check-deterministic-module-engine-decision.mjs",
    "docs/architecture/deterministic-module-engine.json",
  ]),
  step("integration module guard", "node", ["scripts/check-integration-test-modules.mjs"]),
]);

await runSerialGroup("rust structure checks", [
  step("rustfmt", cargo, ["fmt", "--manifest-path", "crates/Cargo.toml", "--all", "--check"]),
  step("rust:crate-graph", "pnpm", ["rust:crate-graph"]),
]);

// One invocation for the shipped binaries and the build-only examples: the
// resolver unifies runx-runtime's feature set across the packages, so its lib
// compiles once instead of twice with divergent feature fingerprints.
const rustBuild = await runStep(
  step("build rust binaries", cargo, [
    "build",
    "--quiet",
    "--manifest-path",
    "crates/Cargo.toml",
    "-p",
    "runx-cli",
    "-p",
    "runx-runtime",
    "-p",
    "runx-js-worker",
    "-p",
    "runx-contracts",
    "-p",
    "runx-receipts",
    "-p",
    "runx-core",
    "-p",
    "runx-parser",
    "--bin",
    "runx",
    "--example",
    "runx-harness-fixture-oracles",
    "--example",
    "runx-native-capability-snapshot",
    "--bin",
    "runx-js-worker",
    "--example",
    "runx-schema-artifacts",
    "--example",
    "runx-paid-invocation-fixtures",
    "--example",
    "runx-principal-id-fixtures",
    "--example",
    "kernel_eval_batch",
    "--example",
    "parser_eval",
    "--example",
    "runx-x402-fixtures",
  ]),
  rustBuildEnv,
);

if (rustBuild.status === 0) {
  await runSerialGroup(
    "generated artifacts and fixtures",
    [
      step("catalog version drift", "pnpm", ["catalog:check"]),
      step("runx-maintained skill lock", "pnpm", ["runx-maintained-lock:check"]),
      step("bundled core skills", "pnpm", ["bundled-skills:check"]),
      step("runx-maintained connectors", "pnpm", ["runx-maintained-connectors:check"]),
      step("registry taxonomy", "pnpm", ["registry-taxonomy:check"]),
      step("build workspace", "node", ["scripts/build-workspace.mjs"]),
      step("extension SDK package contract", "node", ["scripts/check-extension-sdk-package-contract.mjs"]),
      step("publishable manifests", "node", ["scripts/check-publishable-package-manifests.mjs"]),
      step("fixtures:kernel:validate", "pnpm", ["fixtures:kernel:validate"]),
      step("fixtures:kernel:check", "pnpm", ["fixtures:kernel:check"]),
      step("fixtures:kernel:keys", "pnpm", ["fixtures:kernel:keys"]),
      step("fixtures:parser:check", "pnpm", ["fixtures:parser:check"]),
      step("contracts:schemas:check", "pnpm", ["contracts:schemas:check"]),
      step("packet contracts", "pnpm", ["packet-schemas:check"]),
      step("x402 contract conformance", "pnpm", ["x402:contract-conformance"]),
      step("fixtures:contracts:check", "pnpm", ["fixtures:contracts:check"]),
      step("fixtures:contracts:keys", "pnpm", ["fixtures:contracts:keys"]),
      step("fixtures:harness:check", "pnpm", ["fixtures:harness:check"]),
      step("fixtures:harness:summary-check", "pnpm", ["fixtures:harness:summary-check"]),
      step("fixtures:skills:check", "pnpm", ["fixtures:skills:check"]),
      step("fixtures:doctor:check", "pnpm", ["fixtures:doctor:check"]),
      step("fixtures:tool-catalog:check", "pnpm", ["fixtures:tool-catalog:check"]),
      step("docs:api:check", "pnpm", ["docs:api:check"]),
      step("docs:exit-codes", "pnpm", ["docs:exit-codes"]),
      step("doctor json", rustKernelBin, ["doctor", "--json"]),
      step("release candidate smoke", "node", [
        "scripts/smoke-release-candidate.mjs", "--runx-bin", rustKernelBin,
      ]),
      step("test:fast", "pnpm", ["test:fast"]),
    ],
    evalBinEnv,
  );
} else {
  console.error("Skipping eval-binary-dependent checks because a required Rust binary failed to build.");
}

printSummaryAndExit();

function step(name, command, args) {
  return { name, command, args };
}

function defaultCargoBuildJobs() {
  const available = typeof os.availableParallelism === "function" ? os.availableParallelism() : os.cpus().length;
  return String(Math.max(1, Math.min(available, 4)));
}

async function runSerialGroup(name, steps, envExtra = {}) {
  console.log(`\n== ${name} ==`);
  for (const current of steps) {
    await runStep(current, envExtra);
  }
}

async function runParallelGroup(name, steps, envExtra = {}) {
  console.log(`\n== ${name} ==`);
  await Promise.all(steps.map((current) => runStep(current, envExtra)));
}

function runStep(current, envExtra = {}) {
  const started = performance.now();
  console.log(`\n[verify:fast] start ${current.name}`);
  return new Promise((resolve) => {
    const child = spawn(current.command, current.args, {
      cwd: workspaceRoot,
      env: { ...process.env, ...envExtra },
      stdio: "inherit",
    });
    child.on("close", (status, signal) => {
      const durationMs = Math.round(performance.now() - started);
      const result = {
        ...current,
        status: status ?? 1,
        signal,
        durationMs,
      };
      results.push(result);
      const label = result.status === 0 ? "pass" : "fail";
      const signalSuffix = signal ? ` signal=${signal}` : "";
      console.log(`[verify:fast] ${label} ${current.name} (${durationMs}ms)${signalSuffix}`);
      resolve(result);
    });
    child.on("error", (error) => {
      const durationMs = Math.round(performance.now() - started);
      const result = {
        ...current,
        status: 1,
        signal: undefined,
        durationMs,
        error,
      };
      results.push(result);
      console.log(`[verify:fast] fail ${current.name} (${durationMs}ms): ${error.message}`);
      resolve(result);
    });
  });
}

function printSummaryAndExit() {
  const failed = results.filter((result) => result.status !== 0);
  console.log("\n== verify:fast summary ==");
  for (const result of results) {
    const label = result.status === 0 ? "PASS" : "FAIL";
    console.log(`${label} ${result.name} ${result.durationMs}ms`);
  }
  if (failed.length > 0) {
    console.error(`\nverify:fast failed ${failed.length} required step(s):`);
    for (const result of failed) {
      console.error(`- ${result.name}`);
    }
    process.exit(1);
  }
}
