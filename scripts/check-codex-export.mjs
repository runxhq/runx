#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { repositorySkills } from "./lib/skills.mjs";

const root =path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const runx = path.resolve(process.env.RUNX_BIN ?? path.join(root, "crates", "target", "debug", "runx"));
const codexHome = path.resolve(process.env.CODEX_HOME ?? path.join(os.homedir(), ".codex"));
const installRoot = path.join(codexHome, "skills");
const maximumShimBytes = 64 * 1024;
const failures = [];

if (!existsSync(runx)) fail(`runx binary is missing: ${runx}`);
if (!existsSync(installRoot)) fail(`Codex skill directory is missing: ${installRoot}`);

const publicNames = repositorySkills(root, { ...process.env, RUNX_RUST_CLI_BIN: runx })
  .filter((skill) => skill.visibility === "public")
  .map((skill) => skill.name);
const expectedManaged = [...publicNames, "runx"].sort();
const installedManaged = readdirSync(installRoot, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .filter((entry) => {
    const file = path.join(installRoot, entry.name, "SKILL.md");
    return existsSync(file) && readFileSync(file, "utf8").includes("<!-- runx-export:codex ");
  })
  .map((entry) => entry.name)
  .sort();

for (const name of expectedManaged.filter((entry) => !installedManaged.includes(entry))) {
  fail(`${name}: current managed export is missing`);
}
for (const name of installedManaged.filter((entry) => !expectedManaged.includes(entry))) {
  fail(`${name}: stale or internal managed export is installed`);
}

let largestShim = { name: "", bytes: 0 };
for (const name of publicNames) {
  const inspection = inspect(name);
  if (!inspection) continue;
  validateShim(name, inspection, path.join(root, "skills", name));
}

const runtimeFile = path.join(installRoot, "runx", "SKILL.md");
if (existsSync(runtimeFile)) {
  const runtime = readFileSync(runtimeFile, "utf8");
  validateSize("runx", runtime);
  validateEmbeddedManual("runx", runtime);
  const marker = managedMarker(runtime);
  if (!marker || marker.source !== root || !isDigest(marker.packageDigest)) {
    fail("runx: runtime skill marker is missing or not bound to this workspace");
  }
}

const report = {
  schema: "runx.codex_export_check.v1",
  status: failures.length === 0 ? "passed" : "failed",
  codex_home: codexHome,
  runx_bin: runx,
  public_skills: publicNames.length,
  runtime_skills: 1,
  expected_managed: expectedManaged.length,
  installed_managed: installedManaged.length,
  maximum_shim_bytes: maximumShimBytes,
  largest_shim: largestShim,
  failures,
};
process.stdout.write(`${JSON.stringify(report, null, 2)}\n`);
if (failures.length > 0) process.exitCode = 1;

function inspect(name) {
  const result = spawnSync(runx, ["skill", "inspect", path.join(root, "skills", name), "--json"], {
    cwd: root,
    encoding: "utf8",
    env: process.env,
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0) {
    fail(`${name}: native inspection failed with exit ${result.status ?? "unknown"}`);
    return undefined;
  }
  try {
    const inspection = JSON.parse(result.stdout);
    if (inspection.status !== "ok") {
      fail(`${name}: native inspection returned ${inspection.status ?? "no status"}`);
      return undefined;
    }
    return inspection;
  } catch (error) {
    fail(`${name}: native inspection returned invalid JSON (${error instanceof Error ? error.message : String(error)})`);
    return undefined;
  }
}

function validateShim(name, inspection, source) {
  const file = path.join(installRoot, name, "SKILL.md");
  if (!existsSync(file)) {
    fail(`${name}: SKILL.md is missing`);
    return;
  }
  const contents = readFileSync(file, "utf8");
  validateSize(name, contents);
  const manual = validateEmbeddedManual(name, contents);
  const marker = managedMarker(contents);
  if (!marker || marker.source !== source || marker.packageDigest !== inspection.package_digest) {
    fail(`${name}: managed source or package digest drifted`);
  }
  if (
    manual &&
    (manual.digest !== inspection.manual_digest || manual.packageDigest !== inspection.package_digest)
  ) {
    fail(`${name}: embedded manual is not bound to the inspected package`);
  }
  const defaultRunner = inspection.semantic_report?.defaultRunner;
  const runner = inspection.runner_inspections?.find((entry) => entry.runner?.name === defaultRunner);
  const closureDigest = runner?.execution_closure?.closure_digest;
  if (!defaultRunner || !isDigest(closureDigest)) {
    fail(`${name}: inspected default runner has no exact execution closure`);
  } else if (!contents.includes(`--execution-closure-digest ${closureDigest}`)) {
    fail(`${name}: default invocation is not bound to ${closureDigest}`);
  }
  if (!contents.includes(`--package-digest ${inspection.package_digest}`)) {
    fail(`${name}: default invocation is not bound to ${inspection.package_digest}`);
  }
  if (/--(?:package|execution-closure)-digest sha256:</.test(contents)) {
    fail(`${name}: invocation contains a digest placeholder`);
  }
}

function validateSize(name, contents) {
  const bytes = Buffer.byteLength(contents);
  if (bytes > largestShim.bytes) largestShim = { name, bytes };
  if (bytes > maximumShimBytes) fail(`${name}: shim is ${bytes} bytes; maximum is ${maximumShimBytes}`);
}

function validateEmbeddedManual(name, contents) {
  const match = contents.match(
    /<!-- runx-source-manual-begin digest=(sha256:[0-9a-f]{64}) package-digest=(sha256:[0-9a-f]{64}) bytes=([0-9]+) -->\n([\s\S]*?)<!-- runx-source-manual-end -->/,
  );
  if (!match) {
    fail(`${name}: embedded source manual binding is missing`);
    return undefined;
  }
  const [, digest, packageDigest, declaredBytes, manual] = match;
  const bytes = Buffer.byteLength(manual);
  const observedDigest = `sha256:${createHash("sha256").update(manual).digest("hex")}`;
  if (Number(declaredBytes) !== bytes) fail(`${name}: embedded manual byte count drifted`);
  if (digest !== observedDigest) fail(`${name}: embedded manual digest drifted`);
  return { digest, packageDigest };
}

function managedMarker(contents) {
  const match = contents.match(
    /<!-- runx-export:codex source=(.+) package-digest=(sha256:[0-9a-f]{64}) - generated, do not edit -->/,
  );
  return match ? { source: match[1], packageDigest: match[2] } : undefined;
}

function isDigest(value) {
  return typeof value === "string" && /^sha256:[0-9a-f]{64}$/.test(value);
}

function fail(message) {
  failures.push(message);
}
