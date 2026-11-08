#!/usr/bin/env node
// The stale-name guard for 0.12's two retired senses (plan §4.2, §6.1, §6.5):
// `runner` meaning a skill's method, which is `method` now, and `harness`
// meaning a scripted test or a run, which are `test` and `run`. It fails on
// any use of either word outside what may keep it, which the rename reads
// too: a name identifiers.tsv decides stays, such as `$RUNNER_OS`, and a bare
// word one of words.mjs's stays keeps. History documents and signed data
// can't truthfully change (§4.6), so they aren't read, and generated files
// are as clean as the sources their generators read.
//
//   node tools/convert/stale-words.mjs <root> [--list]

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { GENERATED, HISTORY, bareWord, stayFor } from "./words.mjs";

const SCOPE = /^(crates|packages|schemas|skills|connectors|docs|scripts)\//;
const TEXT = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|json|ya?ml|toml|sql|rb|py|sh|md)$/;
const TOKEN = /[A-Za-z_$][A-Za-z0-9_$-]*[A-Za-z0-9]|[A-Za-z]/g;
const RETIRED = { runner: /(^|[^A-Za-z])(runners?|Runners?|RUNNERS?)($|[^a-z])/, harness: /(^|[^A-Za-z])(harness(es)?|Harness(es)?|HARNESS(ES)?)($|[^a-z])/ };
const SIGNED = /"signature"\s*:/;

const table = new URL("./identifiers.tsv", import.meta.url);

function decided(repo) {
  const rows = readFileSync(table, "utf8").split("\n").filter(Boolean).slice(1).map((line) => line.split("\t"));
  return new Map(rows.filter((row) => row[0] === repo).map((row) => [row[1], row[4]]));
}

function files(root) {
  const listed = execFileSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard"], { encoding: "utf8", maxBuffer: 1 << 28 });
  return listed.split("\n").filter((file) => file && SCOPE.test(file) && TEXT.test(file) && !file.includes("node_modules/") && !HISTORY.test(file) && !/(^|\/)vendor\//.test(file) && !GENERATED.test(file) && existsSync(path.join(root, file)));
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
      for (const match of line.matchAll(TOKEN)) {
        const token = match[0];
        for (const [word, pattern] of Object.entries(RETIRED)) {
          if (!pattern.test(token)) continue;
          if (names.get(token) === token) continue;
          if (bareWord(token) === word && stayFor(file, line, match.index, token)) continue;
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
