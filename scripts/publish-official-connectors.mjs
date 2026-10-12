#!/usr/bin/env node
// Publishes every official connector at the version
// registry/official-connectors.lock.json names, through `runx registry
// connector publish`, as the principal the runx CLI is authenticated as (the
// runx publisher's service principal).
//
//   node scripts/publish-official-connectors.mjs --registry https://api.runx.ai
//
// Every package is checked against the lock before anything is published. A
// package or a registry record whose ref, version or content digest differs
// from the lock fails. A rerun reports every entry unchanged. RUNX_BIN names
// the runx to run; it defaults to the `runx` on PATH, the adopted release.
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const USAGE = "Usage: publish-official-connectors --registry <url>";
const workspaceRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const connectorsRoot = path.join(workspaceRoot, "connectors");
const runx = process.env.RUNX_BIN || "runx";

function fail(message) {
  console.error(`publish-official-connectors: ${message}`);
  process.exit(1);
}

function registryArgument() {
  const args = process.argv.slice(2).filter((arg) => arg !== "--");
  const index = args.indexOf("--registry");
  const registry = index >= 0 ? args[index + 1] : undefined;
  if (!registry || args.length !== 2) fail(USAGE);
  return registry;
}

// `runx registry connector <args> --json`, parsed; a failed command stops the run.
function connectorCommand(args) {
  const result = spawnSync(runx, ["registry", "connector", ...args, "--json"], {
    cwd: workspaceRoot,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    stdio: ["ignore", "pipe", "inherit"],
  });
  if (result.error) fail(`${runx} could not run: ${result.error.message}`);
  if (result.status !== 0) fail(`runx registry connector ${args[0]} ${args[1]} exited with ${result.status}`);
  return JSON.parse(result.stdout).registry;
}

function drift(entry, actual) {
  return ["ref", "version", "content_digest"]
    .filter((field) => actual[field] !== entry[field])
    .map((field) => `${field} ${actual[field]} (lock: ${entry[field]})`);
}

const registry = registryArgument();
const lockPath = path.join(workspaceRoot, "registry", "official-connectors.lock.json");
if (!existsSync(lockPath)) fail("registry/official-connectors.lock.json is missing; run pnpm official-connectors:generate");
const { connectors } = JSON.parse(readFileSync(lockPath, "utf8"));
const entries = connectors.map((entry) => ({ ...entry, directory: path.join("connectors", entry.ref.slice(entry.ref.indexOf("/") + 1)) }));

// Nothing is published unless every package is the version the lock names.
const stale = entries.flatMap((entry) => {
  if (!existsSync(path.join(workspaceRoot, entry.directory))) return [`${entry.ref}: ${entry.directory} is missing`];
  const differences = drift(entry, connectorCommand(["package", entry.directory]).connector);
  return differences.length > 0 ? [`${entry.ref}: ${differences.join(", ")}`] : [];
});
if (stale.length > 0) fail(`packages differ from the lock; run pnpm official-connectors:generate\n  ${stale.join("\n  ")}`);

const counts = { published: 0, unchanged: 0 };
for (const entry of entries) {
  const published = connectorCommand(["publish", entry.directory, "--registry", registry]).publish;
  const differences = drift(entry, published);
  if (differences.length > 0) fail(`${entry.ref}: the registry recorded ${differences.join(", ")}`);
  if (published.status !== "published" && published.status !== "unchanged") {
    fail(`${entry.ref}: unexpected publish status ${published.status}`);
  }
  counts[published.status] += 1;
  console.log(`${entry.ref}@${entry.version} ${published.status}`);
}
console.log(`${entries.length} official connectors: ${counts.published} published, ${counts.unchanged} unchanged`);
