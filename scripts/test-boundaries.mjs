#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const boundaryScript = path.join(workspaceRoot, "scripts", "check-boundaries.mjs");
const fixtureRoot = mkdtempSync(path.join(tmpdir(), "runx-boundary-"));
const forbiddenBrokerageTerm = "authorize" + "_url";

try {
  writeMinimalWorkspace(fixtureRoot);

  const ignoredBuildDir = path.join(
    fixtureRoot,
    "packages",
    "source-test",
    ".build",
    "runtime",
  );
  mkdirSync(ignoredBuildDir, { recursive: true });
  writeFileSync(
    path.join(ignoredBuildDir, "cached.ts"),
    `export const stale = "${forbiddenBrokerageTerm}";\n`,
  );
  runBoundary(fixtureRoot, true, "boundary check should ignore stale build output");

  mkdirSync(path.join(fixtureRoot, "packages", "source-test", "src"), { recursive: true });
  writeFileSync(
    path.join(fixtureRoot, "packages", "source-test", "src", "index.ts"),
    `export const live = "${forbiddenBrokerageTerm}";\n`,
  );
  const failed = runBoundary(
    fixtureRoot,
    false,
    "boundary check should reject forbidden source terms",
  );
  const combinedOutput = `${failed.stdout}\n${failed.stderr}`;
  if (!combinedOutput.includes("packages/source-test/src/index.ts")) {
    throw new Error(`boundary finding did not cite source file:\n${combinedOutput}`);
  }
  if (combinedOutput.includes(".build/runtime/cached.ts")) {
    throw new Error(`boundary finding cited ignored build output:\n${combinedOutput}`);
  }

  writeFileSync(path.join(fixtureRoot, "packages/source-test/src/index.ts"), "export {};\n");
  const clientRoot = path.join(fixtureRoot, "packages/client");
  const adaptersRoot = path.join(clientRoot, "src/adapters");
  mkdirSync(adaptersRoot, { recursive: true });
  writeJson(path.join(clientRoot, "package.json"), { name: "@runxhq/client", dependencies: {} });
  const adapterFile = path.join(adaptersRoot, "index.ts");
  for (const specifier of ["node:fs", "@runxhq/client", "../process.js", "../../../../cloud/private.js", "@langchain/core"]) {
    writeFileSync(adapterFile, `export * from ${JSON.stringify(specifier)};\n`);
    const rejected = runBoundary(fixtureRoot, false, `portable adapters must reject ${specifier}`);
    if (!rejected.stderr.includes("packages/client/src/adapters/index.ts")) {
      throw new Error(`portable boundary did not identify the importing file:\n${rejected.stderr}`);
    }
  }
  writeFileSync(adapterFile, "export const portable = true;\n");
  runBoundary(fixtureRoot, true, "portable adapters should pass without forbidden imports");

  console.log("Boundary regression checks passed.");
} finally {
  rmSync(fixtureRoot, { recursive: true, force: true });
}

function writeMinimalWorkspace(root) {
  mkdirSync(path.join(root, "plugins"), { recursive: true });
  mkdirSync(path.join(root, "scripts"), { recursive: true });
  mkdirSync(path.join(root, "tests"), { recursive: true });
  mkdirSync(path.join(root, "fixtures", "contracts"), { recursive: true });
  mkdirSync(path.join(root, "schemas"), { recursive: true });
  mkdirSync(path.join(root, "crates", "runx-contracts", "src"), { recursive: true });
  mkdirSync(path.join(root, "crates", "runx-contracts", "tests"), { recursive: true });
  mkdirSync(path.join(root, "crates", "runx-runtime", "src"), { recursive: true });
  mkdirSync(path.join(root, "crates", "runx-core", "src"), { recursive: true });

  writeJson(path.join(root, "package.json"), {
    private: true,
    devDependencies: {},
  });
  writeJson(path.join(root, "tsconfig.base.json"), {
    compilerOptions: {
      paths: {},
    },
  });
  writeFileSync(path.join(root, "vitest.workspace-aliases.ts"), "export const aliases = {};\n");
}

function writeJson(filePath, value) {
  writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function runBoundary(root, shouldPass, label) {
  const result = spawnSync(process.execPath, [boundaryScript], {
    cwd: workspaceRoot,
    env: {
      ...process.env,
      RUNX_BOUNDARY_WORKSPACE_ROOT: root,
    },
    encoding: "utf8",
  });
  if (shouldPass && result.status !== 0) {
    throw new Error(`${label} failed:\n${result.stdout}\n${result.stderr}`);
  }
  if (!shouldPass && result.status === 0) {
    throw new Error(`${label} unexpectedly passed:\n${result.stdout}\n${result.stderr}`);
  }
  return result;
}
