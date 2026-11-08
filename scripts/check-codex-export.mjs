#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const runx = path.resolve(process.env.RUNX_RUST_CLI_BIN ?? path.join(root, "crates", "target", "debug", "runx"));
const maximumShimBytes = 64 * 1024;
const failures = [];

if (!existsSync(runx)) fail(`runx binary is missing: ${runx}`);

// Every public skill is exported into a home of the check's own, so the check
// reads what this source exports and never a person's installed skills.
const home = mkdtempSync(path.join(os.tmpdir(), "runx-codex-export-"));
process.on("exit", () => rmSync(home, { recursive: true, force: true }));
const installRoot = path.join(home, ".agents", "skills");
exportInto(home);

// Export admission belongs to native Runx, including standalone readiness and
// closure binding. A project export exercises that owner without duplicating
// its selection rules here.
const publicNames = exportableNames();
const expectedManaged = [...publicNames, "runx"].sort();
const installedManaged = readdirSync(installRoot, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .filter((entry) => {
    const file = path.join(installRoot, entry.name, "SKILL.md");
    if (!existsSync(file)) return false;
    const marker = managedMarker(readFileSync(file, "utf8"));
    if (!marker) return false;
    const source = path.resolve(marker.source);
    const relative = path.relative(path.join(root, "skills"), source);
    return source === root || (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
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
  skills_dir: installRoot,
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

function exportInto(target) {
  const result = spawnSync(runx, ["export", "codex", "--json"], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, HOME: target, RUNX_HOME: path.join(target, ".runx"), RUNX_CWD: root, RUNX_EXPORT_BIN: runx },
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0) {
    fail(`native export failed: ${result.stderr.trim() || result.stdout.trim() || result.error?.message || result.status}`);
  }
}

function exportableNames() {
  if (!existsSync(runx)) return [];
  const scratch = path.join(root, ".runx");
  mkdirSync(scratch, { recursive: true });
  const project = mkdtempSync(path.join(scratch, "export-check-"));
  try {
    symlinkSync(path.join(root, "skills"), path.join(project, "skills"), process.platform === "win32" ? "junction" : "dir");
    const result = spawnSync(runx, ["export", "codex", "--project", "--json"], {
      cwd: project,
      encoding: "utf8",
      env: { ...process.env, RUNX_CWD: project, RUNX_EXPORT_BIN: runx },
      maxBuffer: 16 * 1024 * 1024,
    });
    if (result.status !== 0) {
      fail(`native export selection failed: ${result.stderr.trim() || result.stdout.trim() || result.error?.message || result.status}`);
      return [];
    }
    return JSON.parse(result.stdout).exported.map((entry) => entry.skill).sort();
  } catch (error) {
    fail(`native export selection failed: ${error instanceof Error ? error.message : String(error)}`);
    return [];
  } finally {
    rmSync(project, { recursive: true, force: true });
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
  // A skill whose agent makes calls runs on the local MCP server, which binds
  // the package and closure when it starts. Its shim carries no shell command.
  if (contents.includes("MCP server rather than the shell")) return;
  const defaultMethod = inspection.semantic_report?.defaultMethod;
  const runner = inspection.method_inspections?.find((entry) => entry.runner?.name === defaultMethod);
  const closureDigest = runner?.execution_closure?.closure_digest;
  if (!defaultMethod || !isDigest(closureDigest)) {
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
