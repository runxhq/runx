#!/usr/bin/env node
// What the Rust workspace reaches from its entry points, from rust-analyzer's
// SCIP index, which is the removal list's Rust half (plans/0.12, Stage 2 step 5).
//
//   rust-analyzer scip crates --output /tmp/oss.scip
//   node tools/reach/reach.mjs /tmp/oss.scip report.json [--cut <symbol text>]... [--why <symbol text>]...
//
// Entry points come in four tiers, and each item is filed under the first
// that reaches it: the product (the runx and runx-js-worker binaries), what
// publishing runs (the schema artifacts and the capability snapshot), the
// benches the performance contract measures with (plans/0.12/contracts/
// performance.md), which prove kept behaviour, and test tooling (every other
// example program). A --cut stops the walk at any symbol holding that text,
// the way a removed command's handler is cut.
//
// A reference belongs to the innermost definition whose extent holds it. A
// trait method reaches each implementation whose type is reachable, and an
// implementation of a trait from outside the workspace is reachable with its
// type. Fields and variants count with their type both ways, since serde reads
// them without a reference. What the derive macro generates calls to counts
// with it, and so does the `extern crate self as <crate>` alias its generated
// paths name. Tests and build scripts are never callers.

import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { readIndex } from "./scip.mjs";

const PREFIX = "rust-analyzer cargo ";
const PUBLISHING = ["runx-schema-artifacts", "runx-native-capability-snapshot"];
const DERIVE_SOURCE = new URL("../../crates/runx-contracts-derive/src/", import.meta.url);

const [indexFile, outFile, ...rest] = process.argv.slice(2);
if (!indexFile || !outFile) {
  process.stderr.write("usage: reach.mjs <index.scip> <report.json> [--cut <symbol text>]...\n");
  process.exit(2);
}
const cuts = [];
const whys = [];
for (let i = 0; i < rest.length; i += 1) {
  if (rest[i] === "--cut") cuts.push(rest[++i]);
  else if (rest[i] === "--why") whys.push(rest[++i]);
}

const crateOf = (symbol) => symbol.slice(PREFIX.length).split(" ")[0];
const descriptor = (symbol) => symbol.split(" ").slice(4).join(" ");
const isOurs = (symbol) => symbol.startsWith(PREFIX) && crateOf(symbol).startsWith("runx-");
const isTestDoc = (file) => /(^|\/)tests\//.test(file) || /(^|\/)build\.rs$/.test(file) || /_tests?\.rs$/.test(file);
const isBenchDoc = (file) => /(^|\/)benches\//.test(file);
const isTestSymbol = (symbol) => descriptor(symbol).split("/").some((segment) => segment === "tests" || segment === "test" || segment.endsWith("_tests"));
const exampleOf = (file) => /(^|\/)examples\/([^/]+)\.rs$/.exec(file)?.[2];

const as4 = (r) => (r.length === 3 ? [r[0], r[1], r[0], r[2]] : r);
const within = (outer, inner) => {
  const [a0, a1, a2, a3] = as4(outer);
  const [b0, b1, b2, b3] = as4(inner);
  return (a0 < b0 || (a0 === b0 && a1 <= b1)) && (a2 > b2 || (a2 === b2 && a3 >= b3));
};
const smaller = (a, b) => {
  const [a0, a1, a2, a3] = as4(a);
  const [b0, b1, b2, b3] = as4(b);
  return a2 - a0 < b2 - b0 || (a2 - a0 === b2 - b0 && a3 - a1 < b3 - b1);
};

// The lines each `use` item spans, in a crate file.
function useItemLines(file) {
  const lines = new Set();
  let text;
  try {
    text = readFileSync(new URL(`../../crates/${file}`, import.meta.url), "utf8");
  } catch {
    return lines;
  }
  for (const m of text.matchAll(/^[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?use[ \t][^;]*;/gm)) {
    const first = text.slice(0, m.index).split("\n").length - 1;
    const last = first + m[0].split("\n").length - 1;
    for (let line = first; line <= last; line += 1) lines.add(line);
  }
  return lines;
}

const docs = readIndex(indexFile);
const kindOf = new Map();
for (const doc of docs) for (const s of doc.symbols) kindOf.set(s.symbol, s.kind);

const defined = new Map();
const edges = new Map();
const addEdge = (from, to) => {
  if (from === to) return;
  let set = edges.get(from);
  if (!set) edges.set(from, (set = new Set()));
  set.add(to);
};
// A bench or an example is its own crate, but the index gives its root module
// and its `main` the same symbols as its package's library and its other
// examples. So a library definition wins over theirs, and what a bench's or an
// example's file references through a symbol defined elsewhere belongs to a
// node of that file's own, reached only from that file's tier.
const isSrcDoc = (file) => !isTestDoc(file) && !isBenchDoc(file) && !exampleOf(file);
const fileRoots = new Map(); // a bench's or example's file -> its roots
for (const doc of docs) {
  const test = isTestDoc(doc.path);
  const defs = doc.occurrences.filter((o) => o.roles & 1 && !o.symbol.startsWith("local") && o.enclosing);
  for (const d of defs) {
    if (defined.has(d.symbol) && (isSrcDoc(defined.get(d.symbol).path) || !isSrcDoc(doc.path))) continue;
    defined.set(d.symbol, { path: doc.path, test: test || isTestSymbol(d.symbol), bench: isBenchDoc(doc.path), line: d.range[0] + 1, example: exampleOf(doc.path) });
  }
  if (test) continue;
  const own = isSrcDoc(doc.path) ? undefined : `file ${doc.path}`;
  if (own) fileRoots.set(doc.path, new Set([own]));
  const useLines = useItemLines(doc.path);
  for (const o of doc.occurrences) {
    if (o.roles & 1 || o.symbol.startsWith("local") || !isOurs(o.symbol)) continue;
    let owner;
    for (const d of defs) if (within(d.enclosing, o.range) && (!owner || smaller(d.enclosing, owner.enclosing))) owner = d;
    if (!owner || isTestSymbol(owner.symbol)) continue;
    // A module's `use` items name what they import or re-export. That isn't
    // a use: a caller's reference already resolves to the definition itself.
    if (descriptor(owner.symbol).endsWith("/") && useLines.has(o.range[0])) continue;
    const shared = defined.get(owner.symbol).path !== doc.path || /^(crate\/|main\(\)\.)$/.test(descriptor(owner.symbol));
    addEdge(own && shared ? own : owner.symbol, o.symbol);
  }
}
for (const [symbol, info] of defined) {
  if (fileRoots.has(info.path) && !info.test && !/^(crate\/|main\(\)\.)$/.test(descriptor(symbol))) fileRoots.get(info.path).add(symbol);
}
const rootsOf = (test) => [...fileRoots].filter(([file]) => test(file)).flatMap(([, roots]) => [...roots]);

// What the derive macro generates calls to: `schema::<name>` in its source.
const deriveCalls = new Set();
for (const file of readdirSync(DERIVE_SOURCE)) {
  if (!file.endsWith(".rs")) continue;
  for (const m of readFileSync(new URL(file, DERIVE_SOURCE), "utf8").matchAll(/schema::([a-z_]+)/g)) deriveCalls.add(m[1]);
}
const deriveTargets = [...defined.keys()].filter(
  (s) =>
    crateOf(s) === "runx-contracts" &&
    (deriveCalls.has(/^schema\/([a-z_]+)\(\)\.$/.exec(descriptor(s))?.[1]) ||
      /^schema\/RunxSchema#[a-z_]+\(\)\.$/.test(descriptor(s)) ||
      descriptor(s) === "runx_contracts/"),
);

// Functions serde names in a string: `default = "..."`, `skip_serializing_if`, `with` and the like.
const serdeNamed = new Set();
for (const doc of docs) {
  if (isTestDoc(doc.path)) continue;
  let text;
  try {
    text = readFileSync(new URL(`../../crates/${doc.path}`, import.meta.url), "utf8");
  } catch {
    continue;
  }
  for (const attr of text.matchAll(/#\[serde\(([^\]]*)\)\]/g)) {
    for (const m of attr[1].matchAll(/(?:default|skip_serializing_if|serialize_with|deserialize_with|with)\s*=\s*"([^"]+)"/g)) serdeNamed.add(m[1].split("::").pop());
  }
}
const serdeTargets = [...defined.keys()].filter((s) => isOurs(s) && serdeNamed.has(/(?:^|\/)([a-z_0-9]+)\(\)\.$/.exec(descriptor(s))?.[1]));

const parentOf = (symbol) => {
  const d = descriptor(symbol);
  const m = /^(.*#)[^#/]+[.#]$/.exec(d);
  return m && m[1] !== d ? symbol.slice(0, symbol.length - d.length) + m[1] : undefined;
};
const membersOf = new Map();
for (const symbol of defined.keys()) {
  const parent = parentOf(symbol);
  if (parent && !/\(\)\.$/.test(descriptor(symbol))) membersOf.set(parent, [...(membersOf.get(parent) ?? []), symbol]);
}
const IMPL = /impl#\[`?([A-Za-z0-9_]+)[^\]]*\]\[`?([A-Za-z0-9_]+)[^\]]*\]([A-Za-z0-9_]+)\(\)\.$/;
const typeSymbols = new Map();
const traitMethods = new Map();
for (const symbol of defined.keys()) {
  const d = descriptor(symbol);
  const t = /(?:^|\/)([A-Za-z0-9_]+)#$/.exec(d);
  if (t) typeSymbols.set(`${crateOf(symbol)} ${t[1]}`, [...(typeSymbols.get(`${crateOf(symbol)} ${t[1]}`) ?? []), symbol]);
  const m = /(?:^|\/)([A-Za-z0-9_]+)#([A-Za-z0-9_]+)\(\)\.$/.exec(d);
  if (m) traitMethods.set(`${m[1]}#${m[2]}`, [...(traitMethods.get(`${m[1]}#${m[2]}`) ?? []), symbol]);
}
const implRules = [];
for (const symbol of defined.keys()) {
  const m = IMPL.exec(descriptor(symbol));
  if (m) implRules.push({ symbol, types: typeSymbols.get(`${crateOf(symbol)} ${m[1]}`) ?? [], traits: traitMethods.get(`${m[2]}#${m[3]}`) ?? [] });
}

const cut = (symbol) => cuts.some((text) => symbol.includes(text));
const reachedBy = new Map(); // symbol -> tier
const via = new Map(); // symbol -> [the symbol that reached it, how]
function walk(starts, tier) {
  const stack = starts.map((s) => [s, undefined, "root"]);
  const reach = () => {
    while (stack.length) {
      const [s, from, how] = stack.pop();
      if (reachedBy.has(s) || cut(s)) continue;
      reachedBy.set(s, tier);
      via.set(s, [from, how]);
      const parent = parentOf(s);
      if (parent && defined.has(parent)) stack.push([parent, s, "member of"]);
      for (const member of membersOf.get(s) ?? []) stack.push([member, s, "member"]);
      for (const next of edges.get(s) ?? []) stack.push([next, s, "references"]);
    }
  };
  reach();
  for (let changed = true; changed; ) {
    changed = false;
    for (const rule of implRules) {
      if (reachedBy.has(rule.symbol)) continue;
      // A bench's own implementations belong to the bench, never the product.
      if (defined.get(rule.symbol)?.bench && tier !== "proof") continue;
      // A type from outside the workspace is always there; one of ours must be reached.
      const typeLive = rule.types.length === 0 || rule.types.some((t) => reachedBy.has(t));
      const traitLive = rule.traits.length === 0 || rule.traits.some((t) => reachedBy.has(t));
      if (typeLive && traitLive) {
        stack.push([rule.symbol, undefined, "implementation of a reached trait for a reached type"]);
        reach();
        changed = true;
      }
    }
  }
}
const mains = (test) => [...defined.entries()].filter(([s, info]) => /(^|\/)main\(\)\.$/.test(descriptor(s)) && test(s, info)).map(([s]) => s);
const productRoots = mains((s, info) => isSrcDoc(info.path) && /^runx-(cli|js-worker)$/.test(crateOf(s)));
walk([...productRoots, ...deriveTargets, ...serdeTargets], "product");
walk(rootsOf((file) => PUBLISHING.includes(exampleOf(file))), "publishing");
walk(rootsOf(isBenchDoc), "proof");
walk(rootsOf((file) => exampleOf(file) && !PUBLISHING.includes(exampleOf(file))), "tooling");

// A module, its crate's root included, is reached in the first tier that
// reaches anything defined in it.
const TIERS = ["product", "publishing", "proof", "tooling"];
for (const [symbol, tier] of [...reachedBy]) {
  const d = descriptor(symbol);
  const prefix = symbol.slice(0, symbol.length - d.length);
  const modules = [prefix + "crate/"];
  for (let i = d.indexOf("/"); i !== -1; i = d.indexOf("/", i + 1)) modules.push(prefix + d.slice(0, i + 1));
  for (const module of modules) {
    if (!defined.has(module)) continue;
    const held = reachedBy.get(module);
    if (!held || TIERS.indexOf(tier) < TIERS.indexOf(held)) reachedBy.set(module, tier);
  }
}

// Items, not members: functions, methods, types, traits, consts, statics, macros, modules.
const ITEM = /(\(\)\.|#|!)$|\/$/;
const report = {};
for (const [symbol, info] of defined) {
  if (!isOurs(symbol) || info.test || info.bench || info.example) continue;
  const d = descriptor(symbol);
  if (!ITEM.test(d) || (parentOf(symbol) && !/\(\)\.$/.test(d))) continue;
  const file = path.join(crateOf(symbol), info.path.replace(/^[^/]+\//, ""));
  const entry = (report[file] ??= { items: 0, product: 0, publishing: [], proof: [], tooling: [], unreached: [] });
  entry.items += 1;
  const tier = reachedBy.get(symbol);
  if (tier === "product") entry.product += 1;
  else (tier ? entry[tier] : entry.unreached).push(`${info.line} ${d}`);
}
// --why <text>: the chain that reached the first symbol holding the text.
for (const text of whys) {
  const target = [...reachedBy.keys()].find((s) => s.includes(text));
  if (!target) {
    process.stdout.write(`why ${text}: nothing reaches it\n`);
    continue;
  }
  process.stdout.write(`why ${text} (${reachedBy.get(target)}):\n`);
  for (let s = target, guard = 0; s && guard < 200; guard += 1) {
    const [from, how] = via.get(s);
    process.stdout.write(`  ${descriptor(s) || s}  <- ${how}\n`);
    s = from;
  }
}
writeFileSync(outFile, `${JSON.stringify({ roots: productRoots, cuts, report }, null, 1)}\n`);
const totals = { items: 0, product: 0, publishing: 0, proof: 0, tooling: 0, unreached: 0 };
for (const e of Object.values(report)) {
  totals.items += e.items;
  totals.product += e.product;
  totals.publishing += e.publishing.length;
  totals.proof += e.proof.length;
  totals.tooling += e.tooling.length;
  totals.unreached += e.unreached.length;
}
process.stdout.write(`${JSON.stringify(totals)}\n`);
