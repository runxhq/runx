#!/usr/bin/env node
// What the Rust workspace reaches from its entry points, from rust-analyzer's
// SCIP index, which is the removal list's Rust half (plans/0.12, Stage 2 step 5).
//
//   rust-analyzer scip crates --output /tmp/oss.scip
//   node tools/reach/reach.mjs /tmp/oss.scip report.json [--cut <symbol text>]...
//
// Entry points come in three tiers, and each item is filed under the first
// that reaches it: the product (the runx and runx-js-worker binaries), what
// publishing runs (the schema artifacts and the capability snapshot), and test
// tooling (every other example program). A --cut stops the walk at any symbol
// holding that text, the way a removed command's handler is cut.
//
// A reference belongs to the innermost definition whose extent holds it. A
// trait method reaches each implementation whose type is reachable, and an
// implementation of a trait from outside the workspace is reachable with its
// type. Fields and variants count with their type both ways, since serde reads
// them without a reference. What the derive macro generates calls to counts with it.
// Tests, benches and build scripts are never callers.

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
for (let i = 0; i < rest.length; i += 1) if (rest[i] === "--cut") cuts.push(rest[++i]);

const crateOf = (symbol) => symbol.slice(PREFIX.length).split(" ")[0];
const descriptor = (symbol) => symbol.split(" ").slice(4).join(" ");
const isOurs = (symbol) => symbol.startsWith(PREFIX) && crateOf(symbol).startsWith("runx-");
const isTestDoc = (file) => /(^|\/)(tests|benches)\//.test(file) || /(^|\/)build\.rs$/.test(file) || /_tests?\.rs$/.test(file);
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
for (const doc of docs) {
  const test = isTestDoc(doc.path);
  const defs = doc.occurrences.filter((o) => o.roles & 1 && !o.symbol.startsWith("local") && o.enclosing);
  for (const d of defs) {
    if (defined.has(d.symbol)) continue;
    defined.set(d.symbol, { path: doc.path, test: test || isTestSymbol(d.symbol), line: d.range[0] + 1, example: exampleOf(doc.path) });
  }
  if (test) continue;
  for (const o of doc.occurrences) {
    if (o.roles & 1 || o.symbol.startsWith("local") || !isOurs(o.symbol)) continue;
    let owner;
    for (const d of defs) if (within(d.enclosing, o.range) && (!owner || smaller(d.enclosing, owner.enclosing))) owner = d;
    if (owner && !isTestSymbol(owner.symbol)) addEdge(owner.symbol, o.symbol);
  }
}

// What the derive macro generates calls to: `schema::<name>` in its source.
const deriveCalls = new Set();
for (const file of readdirSync(DERIVE_SOURCE)) {
  if (!file.endsWith(".rs")) continue;
  for (const m of readFileSync(new URL(file, DERIVE_SOURCE), "utf8").matchAll(/schema::([a-z_]+)/g)) deriveCalls.add(m[1]);
}
const deriveTargets = [...defined.keys()].filter(
  (s) => crateOf(s) === "runx-contracts" && (deriveCalls.has(/^schema\/([a-z_]+)\(\)\.$/.exec(descriptor(s))?.[1]) || /^schema\/RunxSchema#[a-z_]+\(\)\.$/.test(descriptor(s))),
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
function walk(starts, tier) {
  const stack = [...starts];
  const reach = () => {
    while (stack.length) {
      const s = stack.pop();
      if (reachedBy.has(s) || cut(s)) continue;
      reachedBy.set(s, tier);
      const parent = parentOf(s);
      if (parent && defined.has(parent)) stack.push(parent);
      for (const member of membersOf.get(s) ?? []) stack.push(member);
      for (const next of edges.get(s) ?? []) stack.push(next);
    }
  };
  reach();
  for (let changed = true; changed; ) {
    changed = false;
    for (const rule of implRules) {
      if (reachedBy.has(rule.symbol)) continue;
      // A type from outside the workspace is always there; one of ours must be reached.
      const typeLive = rule.types.length === 0 || rule.types.some((t) => reachedBy.has(t));
      const traitLive = rule.traits.length === 0 || rule.traits.some((t) => reachedBy.has(t));
      if (typeLive && traitLive) {
        stack.push(rule.symbol);
        reach();
        changed = true;
      }
    }
  }
}
const mains = (test) => [...defined.entries()].filter(([s, info]) => /(^|\/)main\(\)\.$/.test(descriptor(s)) && test(s, info)).map(([s]) => s);
const productRoots = mains((s, info) => !info.example && /^runx-(cli|js-worker)$/.test(crateOf(s)));
walk([...productRoots, ...deriveTargets, ...serdeTargets], "product");
walk(mains((s, info) => PUBLISHING.includes(info.example)), "publishing");
walk(mains((s, info) => info.example && !PUBLISHING.includes(info.example)), "tooling");

// Items, not members: functions, methods, types, traits, consts, statics, macros, modules.
const ITEM = /(\(\)\.|#|!)$|\/$/;
const report = {};
for (const [symbol, info] of defined) {
  if (!isOurs(symbol) || info.test || info.example) continue;
  const d = descriptor(symbol);
  if (!ITEM.test(d) || (parentOf(symbol) && !/\(\)\.$/.test(d))) continue;
  const file = path.join(crateOf(symbol), info.path.replace(/^[^/]+\//, ""));
  const entry = (report[file] ??= { items: 0, product: 0, publishing: [], tooling: [], unreached: [] });
  entry.items += 1;
  const tier = reachedBy.get(symbol);
  if (tier === "product") entry.product += 1;
  else (tier ? entry[tier] : entry.unreached).push(`${info.line} ${d}`);
}
writeFileSync(outFile, `${JSON.stringify({ roots: productRoots, cuts, report }, null, 1)}\n`);
const totals = { items: 0, product: 0, publishing: 0, tooling: 0, unreached: 0 };
for (const e of Object.values(report)) {
  totals.items += e.items;
  totals.product += e.product;
  totals.publishing += e.publishing.length;
  totals.tooling += e.tooling.length;
  totals.unreached += e.unreached.length;
}
process.stdout.write(`${JSON.stringify(totals)}\n`);
