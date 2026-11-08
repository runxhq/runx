#!/usr/bin/env node
// The stale-name guard for 0.12's two retired senses (plan §4.2, §6.1, §6.5):
// `runner` meaning a skill's method, which is `method` now, and `harness`
// meaning a scripted test or a run, which are `test` and `run`. It fails on
// any use of either word outside what may keep it:
//
// - a name identifiers.tsv decides stays, such as `$RUNNER_OS`;
// - `runner` naming the machinery that executes runs: the
//   `execution/runner/` module, its paths and its types;
// - what can't truthfully change (§4.6): signed data and history documents.
//
// A bare word is checked like any other name, so a use the rename left
// behind fails here until a row in the table says what it is.
//
//   node tools/convert/stale-words.mjs <root> [--list]

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";

const SCOPE = /^(crates|packages|schemas|skills|connectors|docs|scripts)\//;
const TEXT = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|json|ya?ml|toml|sql|rb|py|sh|md)$/;
const TOKEN = /[A-Za-z_$][A-Za-z0-9_$-]*[A-Za-z0-9]|[A-Za-z]/g;
const RETIRED = { runner: /(^|[^A-Za-z])(runners?|Runners?|RUNNERS?)($|[^a-z])/, harness: /(^|[^A-Za-z])(harness(es)?|Harness(es)?|HARNESS(ES)?)($|[^a-z])/ };
// The machinery keeps `runner` (§4.2): the module, its paths and its types;
// the operational policy's runners, each with a kind, a state and target
// repositories (rename-table.tsv, the policy row); and the kernel fixture
// runner, whose ingestion errors take the `runner-` prefix.
const ENGINE_FILE = /(^|\/)execution\/runner(\/|\.rs$)|^crates\/runx-contracts\/src\/operational_policy(\.rs$|\/)|^crates\/runx-cli\/src\/policy\.rs$|^schemas\/(operational-policy|policy-lint)\.schema\.json$|^packages\/contracts\/src\/schemas\/operational-policy\.|^scripts\/generate-kernel-parity-fixtures\.ts$/;
const ENGINE_LINE = /execution(::|\/)runner|\brunner::|\bmod runner\b/;
// What can't truthfully change (§4.6), and the vendored copies a sync owns.
const HISTORY = /(^|\/)(release\/notes|plans\/(archive|evidence)|\.scafld\/specs\/archive|\.ai|mockups|proof|vendor)\//;
const SIGNED = /"signature"\s*:/;

const table = new URL("./identifiers.tsv", import.meta.url);

function decided(repo) {
  const rows = readFileSync(table, "utf8").split("\n").filter(Boolean).slice(1).map((line) => line.split("\t"));
  return new Map(rows.filter((row) => row[0] === repo).map((row) => [row[1], row[4]]));
}

function files(root) {
  const listed = execFileSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard"], { encoding: "utf8", maxBuffer: 1 << 28 });
  return listed.split("\n").filter((file) => file && SCOPE.test(file) && TEXT.test(file) && !file.includes("node_modules/") && !HISTORY.test(file) && existsSync(path.join(root, file)));
}

function main([root, ...rest]) {
  if (!root) {
    process.stderr.write("usage: stale-words.mjs <root> [--list]\n");
    return 2;
  }
  const list = rest.includes("--list");
  const names = decided("oss");
  const found = { runner: 0, harness: 0 };
  const byFile = new Map();
  for (const file of files(root)) {
    const text = readFileSync(path.join(root, file), "utf8");
    if (file.endsWith(".json") && SIGNED.test(text)) continue;
    text.split("\n").forEach((line, index) => {
      for (const token of line.match(TOKEN) ?? []) {
        for (const [word, pattern] of Object.entries(RETIRED)) {
          if (!pattern.test(token)) continue;
          const to = names.get(token);
          if (to !== undefined && to === token) continue;
          if (word === "runner" && (ENGINE_FILE.test(file) || ENGINE_LINE.test(line))) continue;
          found[word] += 1;
          byFile.set(file, (byFile.get(file) ?? 0) + 1);
          if (list) process.stdout.write(`${file}:${index + 1}\t${word}\t${token}\t${line.trim().slice(0, 160)}\n`);
        }
      }
    });
  }
  const total = found.runner + found.harness;
  process.stdout.write(`${total === 0 ? "clean" : "stale"}: ${found.runner} uses of runner as a method and ${found.harness} of harness as a test or run, in ${byFile.size} files\n`);
  return total === 0 ? 0 : 1;
}

process.exitCode = main(process.argv.slice(2));
