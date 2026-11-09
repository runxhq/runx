#!/usr/bin/env node
// What OSS's files outside the Rust crates reach, file by file: the removal
// list's other half (plans/0.12, Stage 2 step 5).
//
//   node tools/reach/files.mjs <report.json>
//
// A file reaches another by importing it, by naming its path in a command, or,
// in a package's run.yaml or connector.yaml, by naming one of its own files.
// Entry points come in tiers, and each file is filed under the first that
// reaches it: ci (what the workflows run), product (each published package's
// exports and bin, every skill's run.yaml, every connector's connector.yaml),
// and dev (package.json's scripts, which only a person runs). Tests and docs
// never make a file live; tests go with what they test.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const ROOT = path.resolve(new URL("../..", import.meta.url).pathname);
const outFile = process.argv[2];
if (!outFile) {
  process.stderr.write("usage: files.mjs <report.json>\n");
  process.exit(2);
}

const tracked = execFileSync("git", ["-C", ROOT, "ls-files"], { encoding: "utf8", maxBuffer: 1 << 28 }).split("\n").filter(Boolean);
const trackedSet = new Set(tracked);
const CODE = /\.(ts|mts|cts|tsx|js|mjs|cjs|sh|py)$/;
const TEXT = /\.(ts|mts|cts|tsx|js|mjs|cjs|sh|py|json|ya?ml|toml)$/;
const isTest = (file) => /(\.|_)(test|spec)\.[a-z]+$/.test(file) || /(^|\/)(tests?|__tests__|fixtures)\//.test(file);
const isRust = (file) => file.startsWith("crates/");
const subjects = tracked.filter((file) => CODE.test(file) && !isRust(file) && !file.startsWith("tools/reach/") && !file.startsWith("tools/convert/"));

// Every published package, by name, to its entry files.
const packages = new Map();
for (const file of tracked.filter((f) => /^packages\/[^/]+\/package\.json$/.test(f))) {
  const manifest = JSON.parse(readFileSync(path.join(ROOT, file), "utf8"));
  const dir = path.dirname(file);
  const entries = new Set();
  const collect = (value) => {
    if (typeof value === "string") entries.add(path.normalize(path.join(dir, value)));
    else if (value && typeof value === "object") Object.values(value).forEach(collect);
  };
  collect(manifest.exports);
  collect(manifest.main);
  collect(manifest.bin);
  collect(manifest.types);
  packages.set(manifest.name, { dir, entries: [...entries], published: !manifest.private });
}

/** The tracked file a written path names, trying the forms TypeScript and Node accept. */
function resolve(from, written) {
  const base = written.startsWith(".") ? path.normalize(path.join(path.dirname(from), written)) : path.normalize(written);
  const stem = base.replace(/\.(js|mjs|cjs)$/, "");
  const candidates = [base, `${stem}.ts`, `${stem}.mts`, `${stem}.tsx`, `${base}.ts`, `${base}.mjs`, `${base}.js`, path.join(base, "index.ts"), path.join(base, "index.mjs"), path.join(base, "index.js")];
  // A package's built output stands for its source.
  candidates.push(...candidates.map((c) => c.replace(/\/dist\//, "/src/")));
  return candidates.find((c) => trackedSet.has(c));
}

const edges = new Map();
const add = (from, to) => {
  if (!to || from === to) return;
  if (!edges.has(from)) edges.set(from, new Set());
  edges.get(from).add(to);
};
const PATHLIKE = /["'`(\s=]((?:\.{1,2}\/|(?:scripts|packages|tools|skills|connectors|fixtures|tests|release|packaging|bindings|examples)\/)[A-Za-z0-9_./@-]+)/g;
const FILENAME = /(?<![A-Za-z0-9_@/.-])([A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:ts|mts|mjs|js|cjs|sh|py|json|ya?ml))(?![A-Za-z0-9_-])/g;
const PACKAGE_IMPORT = /["'](@runxhq\/[a-z-]+)(\/[A-Za-z0-9_./-]+)?["']/g;
const BARE_MODULE = /(?:module|path|source|script):\s*["']?([A-Za-z0-9_./-]+\.(?:mjs|js|ts|sh|py))/g;
for (const file of tracked) {
  if (!TEXT.test(file) || isRust(file)) continue;
  let text;
  try {
    text = readFileSync(path.join(ROOT, file), "utf8");
  } catch {
    continue;
  }
  if (text.length > 3_000_000) continue;
  for (const m of text.matchAll(PATHLIKE)) {
    const written = m[1].replace(/[.,;:)]+$/, "");
    const found = resolve(file, written);
    if (found) add(file, found);
    else {
      // A path naming a runx package's folder, as `skill: ../extract` does, reaches its
      // manifest, and the manifest's own references decide which of its code is reached.
      // A path naming any other package's folder reaches the code in it, as a build or
      // publish step does.
      const dir = `${path.normalize(written.startsWith(".") ? path.join(path.dirname(file), written) : written).replace(/\/$/, "")}/`;
      const runx = ["run.yaml", "connector.yaml"].map((m) => dir + m).filter((m) => trackedSet.has(m));
      const isPackage = ["package.json", "pyproject.toml", "setup.py", "Cargo.toml"].some((m) => trackedSet.has(dir + m));
      if (runx.length) for (const manifest of runx) add(file, manifest);
      else if (isPackage) for (const inside of subjects) if (inside.startsWith(dir) && !isTest(inside)) add(file, inside);
    }
  }
  // Any other token naming a file, from the file's own folder and then from the root.
  for (const m of text.matchAll(FILENAME)) add(file, resolve(file, `./${m[1]}`) ?? resolve(file, m[1]));
  for (const m of text.matchAll(PACKAGE_IMPORT)) {
    const pkg = packages.get(m[1]);
    if (!pkg) continue;
    if (m[2]) add(file, resolve(path.join(pkg.dir, "x"), `.${m[2]}`) ?? resolve(path.join(pkg.dir, "src", "x"), `.${m[2]}`));
    for (const entry of pkg.entries) add(file, resolve(entry, `./${path.basename(entry)}`) ?? resolve(path.join(pkg.dir, "x"), "./src/index"));
  }
  // A package's own files, named from its run.yaml, connector.yaml or a tool manifest,
  // and its own tools by name: `tool: reddit.brave` is tools/reddit/brave/manifest.json.
  if (/(^|\/)(run|connector)\.yaml$|manifest\.json$/.test(file)) {
    for (const m of text.matchAll(BARE_MODULE)) add(file, resolve(file, `./${m[1]}`));
    for (const m of text.matchAll(/tool:\s*["']?([a-z0-9_-]+(?:\.[a-z0-9_-]+)+)/g)) {
      add(file, resolve(file, `./tools/${m[1].split(".").join("/")}/manifest.json`));
    }
  }
  // Python's own imports, from the file's folder.
  if (file.endsWith(".py")) {
    for (const m of text.matchAll(/^\s*(?:from|import)\s+([A-Za-z_][A-Za-z0-9_.]*)/gm)) {
      add(file, resolve(file, `./${m[1].replace(/\./g, "/")}.py`) ?? resolve(file, `./${m[1].replace(/\./g, "/")}/__init__.py`));
    }
  }
  // A command that runs a package.json script by name.
  for (const m of text.matchAll(/\b(?:pnpm|npm run|pnpm run|yarn)\s+(?:-[a-zA-Z-]+\s+)*([a-z][a-z0-9:_-]*)/g)) add(file, `package.json#${m[1]}`);
}
// Each package.json script is its own node, reached only when something runs it by name:
// in a command, or as a quoted name a method script holds in a list.
const scriptNodes = [];
{
  const manifest = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8"));
  const names = new Set(Object.keys(manifest.scripts ?? {}));
  for (const file of tracked) {
    if (!/\.(ts|mts|mjs|js|cjs|sh|ya?ml)$/.test(file) || isRust(file)) continue;
    let text;
    try {
      text = readFileSync(path.join(ROOT, file), "utf8");
    } catch {
      continue;
    }
    for (const m of text.matchAll(/["'`]([a-z][a-z0-9:_-]*)["'`]/g)) if (names.has(m[1]) && m[1].includes(":")) add(file, `package.json#${m[1]}`);
  }
  edges.delete("package.json");
  for (const [name, command] of Object.entries(manifest.scripts ?? {})) {
    const node = `package.json#${name}`;
    scriptNodes.push(node);
    for (const m of ` ${command}`.matchAll(PATHLIKE)) add(node, resolve("package.json", m[1].replace(/[.,;:)]+$/, "")));
    for (const m of command.matchAll(FILENAME)) add(node, resolve("package.json", m[1]));
    for (const m of command.matchAll(/\b(?:pnpm|npm run|pnpm run)\s+(?:-[a-zA-Z-]+\s+)*([a-z][a-z0-9:_-]*)/g)) add(node, `package.json#${m[1]}`);
  }
}

const tiers = {
  ci: tracked.filter((f) => f.startsWith(".github/")),
  product: [
    ...[...packages.values()].filter((p) => p.published).flatMap((p) => p.entries.map((e) => resolve(e, `./${path.basename(e)}`) ?? e)),
    ...tracked.filter((f) => /^(skills|connectors|bindings)\/.*(run|connector)\.yaml$/.test(f) && !isTest(f)),
  ],
  dev: [...scriptNodes, ...tracked.filter((f) => /^packages\/[^/]+\/package\.json$/.test(f))],
};
const reachedBy = new Map();
for (const [tier, roots] of Object.entries(tiers)) {
  const stack = [...roots];
  while (stack.length) {
    const file = stack.pop();
    if (!file || reachedBy.has(file)) continue;
    reachedBy.set(file, tier);
    if (isTest(file) && !roots.includes(file)) continue;
    for (const next of edges.get(file) ?? []) stack.push(next);
  }
}

const report = { ci: [], product: [], dev: [], unreached: [], tests: [] };
for (const file of subjects) {
  if (isTest(file)) report.tests.push(file);
  else report[reachedBy.get(file) ?? "unreached"].push(file);
}
writeFileSync(outFile, `${JSON.stringify({ packages: Object.fromEntries([...packages].map(([n, p]) => [n, { dir: p.dir, published: p.published }])), report }, null, 1)}\n`);
process.stdout.write(`${JSON.stringify(Object.fromEntries(Object.entries(report).map(([k, v]) => [k, v.length])))}\n`);
