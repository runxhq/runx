#!/usr/bin/env node
// The stale-name guard for 0.12's retired senses (plan §4.2, §6.1, §6.5):
// `runner` meaning a skill's method, which is `method` now, `harness` meaning
// a scripted test or a run, which are `test` and `run`, and, in prose, `graph`
// meaning a method's steps, which is `workflow`, "execution closure", which is
// a package's closure, its one meaning, "agent act", which is a turn, and
// every key the format contract removes, as removed-keys.mjs reads them. It fails on any use of
// a retired word outside what may keep it, which the rename reads
// too: a name identifiers.tsv decides stays, such as `$RUNNER_OS`, and a bare
// word one of words.mjs's stays keeps. History documents and signed data
// can't truthfully change (§4.6), so they aren't read, and generated files
// are as clean as the sources their generators read. A path is read too: each
// file and folder whose own name holds a retired word fails, but where a path
// stay in words.mjs keeps it, as history documents keep theirs.
//
//   node tools/convert/stale-words.mjs <root> [--list]

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { removedKeys } from "./removed-keys.mjs";
import { GENERATED, HISTORY, LOCKFILE, PROSE_ONLY, SOURCE, bareWord, fencedBlocks, pathStayFor, proseSites, retiredInPath, stayFor, stringLiterals } from "./words.mjs";

// What the rename reads: everything but the converter, scafld's records, vendored copies and lockfiles.
const NOT_OURS = /^tools\/convert\/|(^|\/)\.scafld\/|(^|\/)vendor\//;
const TOKEN = /[A-Za-z_$][A-Za-z0-9_$-]*[A-Za-z0-9]|[A-Za-z]/g;
const RETIRED = {
  runner: /(^|[^A-Za-z])(runners?|Runners?|RUNNERS?)($|[^a-z])/,
  harness: /(^|[^A-Za-z])(harness(es)?|Harness(es)?|HARNESS(ES)?)($|[^a-z])/,
  graph: /(^|[^A-Za-z])(graphs?|Graphs?|GRAPHS?)($|[^a-z])/,
};
// "Execution closure" read as the old shape: a package's closure is its closure.
const EXECUTION_CLOSURE = /(?<![A-Za-z])execution[ _-]?[Cc]losures?(?![a-z])|Execution[ _-]?[Cc]losures?(?![a-z])|(?<![A-Z])EXECUTION_CLOSURES?(?![A-Z])/g;
// An agent's unit of work is its turn; an act is what a call is told to do.
// A count of acts beside an agent, as a summary prints it, is a count of turns.
const AGENT_ACT = /(?<![A-Za-z])agent[ _-]acts?(?![a-z])|(?<![a-z])Agent[ _-]?[Aa]cts?(?![a-z])|(?<![A-Za-z])[Aa]gent: (?:\{\}|\d+) act(?:s|\(s\))?(?![a-z])/g;
const SIGNED = /"signature"\s*:/;

const table = new URL("./identifiers.tsv", import.meta.url);

function decided(repo) {
  const rows = readFileSync(table, "utf8").split("\n").filter(Boolean).slice(1).map((line) => line.split("\t"));
  return new Map(rows.filter((row) => row[0] === repo).map((row) => [row[1], row[4]]));
}

function tracked(root) {
  const listed = execFileSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard"], { encoding: "utf8", maxBuffer: 1 << 28 });
  return listed.split("\n").filter((file) => file && !file.includes("node_modules/") && existsSync(path.join(root, file)));
}

function files(root) {
  return tracked(root).filter((file) => !NOT_OURS.test(file) && SOURCE.test(file) && !LOCKFILE.test(path.basename(file)) && !HISTORY.test(file) && !GENERATED.test(file));
}

/** Each file and folder whose own name holds a retired word, in order, with no path stay. */
function stalePaths(root) {
  const paths = new Set();
  for (const file of tracked(root)) {
    const parts = file.split("/");
    parts.forEach((_, index) => paths.add(parts.slice(0, index + 1).join("/") + (index < parts.length - 1 ? "/" : "")));
  }
  return [...paths]
    .sort()
    .filter((one) => !/(^|\/)vendor\/./.test(one))
    .flatMap((one) => {
      const retired = retiredInPath(one);
      return retired && !pathStayFor(one) ? [{ path: one, ...retired }] : [];
    });
}

function main([root, ...rest]) {
  if (!root) {
    process.stderr.write("usage: stale-words.mjs <root> [--list]\n");
    return 2;
  }
  const list = rest.includes("--list");
  const names = decided("oss");
  const found = { runner: 0, harness: 0, graph: 0, closure: 0, turn: 0, key: 0, path: 0 };
  const byFile = new Map();
  for (const file of files(root)) {
    const text = readFileSync(path.join(root, file), "utf8");
    if (file.endsWith(".json") && SIGNED.test(text)) continue;
    // A prose-only word is stale where it's read as code or prose.
    const inProse = proseSites(file, text);
    const lines = text.split("\n");
    const lineOf = (at) => text.slice(0, at).split("\n").length;
    const literals = (code) => stringLiterals(code, file).map(({ pieces }) => [pieces[0][0], pieces.at(-1)[1]]);
    for (const removed of removedKeys(file, text, { fences: fencedBlocks, strings: literals })) {
      const line = lineOf(removed.at);
      found.key += 1;
      byFile.set(file, (byFile.get(file) ?? 0) + 1);
      if (list) process.stdout.write(`${file}:${line}\tkey\t${removed.key}\t${removed.row}\t${(lines[line - 1] ?? "").trim().slice(0, 120)}\n`);
    }
    let lineStart = 0;
    lines.forEach((line, index) => {
      const start = lineStart;
      lineStart += line.length + 1;
      for (const match of line.matchAll(TOKEN)) {
        const token = match[0];
        for (const [word, pattern] of Object.entries(RETIRED)) {
          if (!pattern.test(token)) continue;
          if (PROSE_ONLY.has(word) && !inProse(start + match.index, token)) continue;
          if (names.get(token) === token) continue;
          if (bareWord(token) === word && stayFor(file, line, match.index, token)) continue;
          found[word] += 1;
          byFile.set(file, (byFile.get(file) ?? 0) + 1);
          if (list) process.stdout.write(`${file}:${index + 1}\t${word}\t${token}\t${line.trim().slice(0, 160)}\n`);
        }
      }
      for (const [word, pattern] of [["closure", EXECUTION_CLOSURE], ["turn", AGENT_ACT]]) {
        for (const match of line.matchAll(pattern)) {
          found[word] += 1;
          byFile.set(file, (byFile.get(file) ?? 0) + 1);
          if (list) process.stdout.write(`${file}:${index + 1}\t${word}\t${match[0]}\t${line.trim().slice(0, 160)}\n`);
        }
      }
    });
  }
  for (const stale of stalePaths(root)) {
    found.path += 1;
    if (list) process.stdout.write(`${stale.path}\tpath\t${stale.word}\t${stale.name}\n`);
  }
  const total = found.runner + found.harness + found.graph + found.closure + found.turn + found.key + found.path;
  process.stdout.write(`${total === 0 ? "clean" : "stale"}: ${found.runner} uses of runner as a method, ${found.harness} of harness as a test or run, ${found.graph} of graph as a workflow in prose, ${found.closure} of execution closure, which is closure, ${found.turn} of agent act, which is a turn, ${found.key} of a key the format removes and ${found.path} paths holding a retired word, in ${byFile.size} files\n`);
  return total === 0 ? 0 : 1;
}

process.exitCode = main(process.argv.slice(2));
