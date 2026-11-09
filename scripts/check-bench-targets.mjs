#!/usr/bin/env node
// Every `[[bench]]` target in the workspace runs under Criterion.
//
// Criterion supplies its own `main`, so a bench target declares Cargo's
// `harness = false`. Without it Cargo builds the target under libtest's
// test runner, which runs no Criterion workload and refuses Criterion's own
// arguments, and nothing fails: `cargo test --benches` passes over a target
// with no tests, and a capture finds no estimates. A rename once turned the
// key into `test = false`, which Cargo also accepts, and the benches ran
// nothing unseen. This check refuses any bench target that doesn't declare
// it, naming the file and the target.
//
// Usage: node scripts/check-bench-targets.mjs [root]

import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(
  process.argv[2] ?? path.join(path.dirname(fileURLToPath(import.meta.url)), ".."),
);
const SKIPPED = new Set([".git", "node_modules", "target", "dist"]);

function manifests(directory) {
  const found = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    if (SKIPPED.has(entry.name)) continue;
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) found.push(...manifests(full));
    else if (entry.isFile() && entry.name === "Cargo.toml") found.push(full);
  }
  return found;
}

// What a bench target writes when it supplies its own main, Criterion's.
const CRITERION_MAIN = "harness = false";

// Each `[[bench]]` table in a manifest, with its name and whether it runs
// under Criterion. A table runs from its header to the next header.
function benchTargets(text) {
  const targets = [];
  let current = null;
  for (const line of text.split(/\r?\n/u)) {
    const trimmed = line.replace(/#.*$/u, "").trim();
    if (trimmed.startsWith("[")) {
      current = trimmed === "[[bench]]" ? { name: null, runsUnderCriterion: false } : null;
      if (current) targets.push(current);
      continue;
    }
    if (!current) continue;
    const pair = /^([A-Za-z0-9_-]+)\s*=\s*(.+)$/u.exec(trimmed);
    if (!pair) continue;
    const [, key, value] = pair;
    if (key === "name") current.name = value.replace(/^"|"$/gu, "");
    if (`${key} = ${value}` === CRITERION_MAIN) current.runsUnderCriterion = true;
  }
  return targets;
}

const refused = [];
for (const manifest of manifests(root).sort()) {
  for (const target of benchTargets(readFileSync(manifest, "utf8"))) {
    if (!target.runsUnderCriterion) {
      refused.push(
        `${path.relative(root, manifest)}: bench target ${target.name ?? "(unnamed)"} doesn't declare harness = false, so Cargo runs it under libtest's test runner and none of its Criterion workloads run`,
      );
    }
  }
}

if (refused.length > 0) {
  process.stderr.write(`${refused.join("\n")}\n`);
  process.exit(1);
}
process.stdout.write("Every bench target declares harness = false.\n");
