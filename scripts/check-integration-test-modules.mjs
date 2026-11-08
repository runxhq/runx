#!/usr/bin/env node
// Guard for the consolidated integration-test layout.
//
// Each crate that sets `autotests = false` compiles its integration tests as a
// single binary (tests/integration.rs) whose body is a list of `mod <name>;`
// declarations. That is the layout Cargo recommends when many integration test
// files make compile/run time inefficient (see the Cargo Book, "Integration
// tests"): https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests
//
// The risk of `autotests = false` is silent loss of coverage: someone adds
// tests/new_thing.rs and forgets `mod new_thing;`, so Cargo never builds it and
// nobody notices. This guard fails when a top-level tests/*.rs file is not
// referenced by integration.rs, when Cargo.toml is missing the explicit
// integration target required by `autotests = false`, when a directory-style
// tests/<name>/main.rs target would be dropped by `autotests = false`, when
// integration.rs references a module with no matching file or mod.rs, and when a
// test mutates process-global state (which is unsafe across tests sharing one
// binary under `cargo test`).
//
// Usage: node scripts/check-integration-test-modules.mjs

import { readdirSync, readFileSync, existsSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ossRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cratesDir = path.join(ossRoot, "crates");

// Process-global mutations break test isolation when many test files share one
// binary: under `cargo test` they run as threads in one process, so one test's
// mutation leaks into others. nextest isolates per process, but the suite must
// also stay correct under plain `cargo test`. Ban these in test code; if a test
// genuinely needs them, isolate it (e.g. serial_test) and add an explicit
// `// allow-process-global:` justification comment on the same line.
const BANNED_GLOBAL_MUTATIONS = [
  /\benv::set_var\s*\(/,
  /\benv::remove_var\s*\(/,
  /\bstd::env::set_var\s*\(/,
  /\bstd::env::remove_var\s*\(/,
  /\bset_current_dir\s*\(/,
];

const errors = [];

function listRustFiles(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) out.push(...listRustFiles(full));
    else if (entry.isFile() && entry.name.endsWith(".rs")) out.push(full);
  }
  return out;
}

function topLevelModNames(integrationSource) {
  // Only column-0 module declarations are crate-root test modules. Each maps
  // to the file a `#[path = "..."]` attribute directly above it names, if
  // one does.
  const names = new Map();
  let attributedPath = null;
  for (const line of integrationSource.split("\n")) {
    const attribute = /^#\[path\s*=\s*"([^"]+)"\]\s*$/.exec(line);
    if (attribute) {
      attributedPath = attribute[1];
      continue;
    }
    const match = /^(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;/.exec(
      line,
    );
    if (match) names.set(match[1], attributedPath);
    if (!/^#\[/.test(line)) attributedPath = null;
  }
  return names;
}

function moduleHasSource(testsDir, name, attributedPath) {
  // A `mod name;` in tests/integration.rs is satisfied by the file its
  // `#[path]` names, else by tests/name.rs or tests/name/mod.rs. A bare
  // tests/name/ directory is not enough.
  if (attributedPath !== null) {
    return existsSync(path.resolve(testsDir, attributedPath));
  }
  return (
    existsSync(path.join(testsDir, `${name}.rs`)) ||
    existsSync(path.join(testsDir, name, "mod.rs"))
  );
}

function hasIntegrationTestTarget(cargoToml) {
  let inTestTarget = false;
  let targetName = "";
  let targetPath = "";

  function matchesIntegrationTarget() {
    return targetName === "integration" && targetPath === "tests/integration.rs";
  }

  for (const rawLine of cargoToml.split("\n")) {
    const line = rawLine.replace(/\s+#.*$/, "").trim();
    const table = /^\[\[?([^\]]+)\]\]?/.exec(line);
    if (table) {
      if (inTestTarget && matchesIntegrationTarget()) return true;
      inTestTarget = line === "[[test]]";
      targetName = "";
      targetPath = "";
      continue;
    }
    if (!inTestTarget) continue;

    const assignment = /^([A-Za-z_][A-Za-z0-9_-]*)\s*=\s*"([^"]*)"\s*$/.exec(line);
    if (!assignment) continue;
    if (assignment[1] === "name") targetName = assignment[2];
    if (assignment[1] === "path") targetPath = assignment[2];
  }

  return inTestTarget && matchesIntegrationTarget();
}

const crates = readdirSync(cratesDir, { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => path.join(cratesDir, d.name))
  .filter((c) => existsSync(path.join(c, "Cargo.toml")));

let checkedCrates = 0;

for (const crate of crates) {
  const cargo = readFileSync(path.join(crate, "Cargo.toml"), "utf8");
  if (!/^\s*autotests\s*=\s*false\b/m.test(cargo)) continue;

  const testsDir = path.join(crate, "tests");
  const rel = path.relative(ossRoot, crate);
  const integrationPath = path.join(testsDir, "integration.rs");

  if (!hasIntegrationTestTarget(cargo)) {
    errors.push(
      `${rel}: autotests = false but Cargo.toml is missing an explicit ` +
        `[[test]] target with name = "integration" and ` +
        `path = "tests/integration.rs".`,
    );
    continue;
  }

  if (!existsSync(integrationPath)) {
    errors.push(`${rel}: autotests = false but tests/integration.rs is missing.`);
    continue;
  }
  checkedCrates += 1;

  const declared = topLevelModNames(readFileSync(integrationPath, "utf8"));

  // Every top-level tests/*.rs (except integration.rs) must be referenced.
  const fileStems = readdirSync(testsDir)
    .filter((f) => f.endsWith(".rs") && f !== "integration.rs")
    .map((f) => f.slice(0, -3));
  for (const stem of fileStems) {
    if (!declared.has(stem)) {
      errors.push(
        `${rel}/tests/${stem}.rs exists but is not declared in integration.rs ` +
          `(add \`mod ${stem};\`), so it is never compiled or run.`,
      );
    }
  }

  for (const entry of readdirSync(testsDir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const mainPath = path.join(testsDir, entry.name, "main.rs");
    if (existsSync(mainPath)) {
      errors.push(
        `${rel}/tests/${entry.name}/main.rs is a directory-style integration ` +
          `test target, but autotests = false disables Cargo's automatic target ` +
          `discovery. Move it to tests/${entry.name}.rs or ` +
          `tests/${entry.name}/mod.rs and declare \`mod ${entry.name};\` in ` +
          `integration.rs.`,
      );
    }
  }

  // Every declared module must resolve to a real file or directory.
  for (const [name, attributedPath] of declared) {
    if (!moduleHasSource(testsDir, name, attributedPath)) {
      errors.push(
        `${rel}/tests/integration.rs declares \`mod ${name};\` but no matching ` +
          `file or directory exists.`,
      );
    }
  }
}

// Ban process-global mutations anywhere under crates/*/tests.
for (const crate of crates) {
  const testsDir = path.join(crate, "tests");
  if (!existsSync(testsDir) || !statSync(testsDir).isDirectory()) continue;
  for (const file of listRustFiles(testsDir)) {
    const lines = readFileSync(file, "utf8").split("\n");
    lines.forEach((line, index) => {
      if (line.includes("allow-process-global")) return;
      for (const pattern of BANNED_GLOBAL_MUTATIONS) {
        if (pattern.test(line)) {
          errors.push(
            `${path.relative(ossRoot, file)}:${index + 1} mutates process-global ` +
              `state, which is unsafe across tests sharing one integration binary. ` +
              `Isolate it (e.g. serial_test) and annotate with ` +
              `\`// allow-process-global: <reason>\`, or scope the state per test.`,
          );
          break;
        }
      }
    });
  }
}

if (errors.length > 0) {
  console.error("Integration-test module guard failed:\n");
  for (const error of errors) console.error(`  - ${error}`);
  console.error(
    `\n${errors.length} issue(s). See ` +
      `.scafld/specs/active/test-surface-build-consolidation.md.`,
  );
  process.exit(1);
}

console.log(`Integration-test module guard passed (${checkedCrates} consolidated crate(s)).`);
