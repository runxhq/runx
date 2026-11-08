#!/usr/bin/env node
// Renames 0.12's words in code, from one table of exact identifiers per repo.
// The plan's §4.2 and the rename table (plans/0.12/rename-table.tsv) own the
// words; identifiers.tsv beside this file is the authority for every name.
//
//   node tools/convert/rename.mjs scan  <repo> <root>             add rows for names not in the table yet
//   node tools/convert/rename.mjs apply <repo> <root> [--write] [--list] [--stays]
//                                         rename, refusing while a name is missing; --list prints each
//                                         file and its count, --stays each bare word a stay keeps
//
// `scan` keeps every row already in the table, so a decision made by hand
// stays made. `apply` replaces whole identifiers only, all at once, so a chain
// such as TurnInvocation to TurnView and AgentActInvocation to TurnInvocation
// can't run into itself. The bare words `runner` and `harness` have senses
// that stay, so they follow words.mjs: renamed everywhere but where its short
// list of stays keeps them. Prose is renamed with the code it names; history
// documents and signed data aren't, and generated files are regenerated.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, renameSync, rmdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { GENERATED, HISTORY as HISTORY_DOCS, STAYING_PATHS, bareWord, renamedBare, stayFor } from "./words.mjs";

const TABLE = new URL("./identifiers.tsv", import.meta.url);
const COLUMNS = ["repo", "identifier", "uses", "words", "to", "note", "where"];
const SOURCE = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|json|ya?ml|toml|sql|rb|py|sh|md)$/;
const TOKEN = /[A-Za-z_$][A-Za-z0-9_$-]*[A-Za-z0-9]|[A-Za-z]/g;

// Each renamed word, in the forms a segment of a name can take.
const FORMS = {
  harness: ["harness", "harnesses"],
  hold: ["hold", "holds", "held", "holding", "holdings"],
  closure: ["closure"],
  graph: ["graph", "graphs"],
  runner: ["runner", "runners"],
  agent_session: ["agentsession"],
  agent_act: ["agentact"],
};
const ORDER = Object.keys(FORMS);

// The plan's own names (§4.2, §6.1 to §6.5), exact.
const OVERRIDES = {
  ExecutionGraph: "Workflow", GraphStep: "WorkflowStep", parse_graph_yaml: "parse_workflow_yaml",
  graph_step_failed: "workflow_step_failed", graph_blocked: "workflow_blocked", TurnInvocation: "TurnView", TurnEnvelope: "TurnViewEnvelope",
  AgentAct: "Turn", AgentActInvocation: "TurnInvocation", HarnessTurnScript: "TestTurnCalls", "agent-session": "agent", $held: "$lease",
  held_connectors: "connectors_with_leases",
};

// The other senses each word has, which stay: CI and package runners, the
// consumers' own executors and playbook runner, test data, and a dependency graph.
const STAYS = {
  runner: /^\$RUNNER_|^RUNNER_(TEMP|OS|ARCH|NAME|TOOL_CACHE|DEBUG|ENVIRONMENT|WORKSPACE|LIMIT_MINUTES|PATH|VOICED)$|ToolRunner|tool_runner|test_runner|local-pr-runner|^step_runner$|^stage_runner$|^TurnRunner$|^CallRunners$|^RunnerConfig$|^runner-driven$|^mcp-runner$|^verifier-runner$/,
  graph: /GraphQL|LangGraph|dependency-advisory-graph|crate-graph/,
};
// Whole segments only, so a test's "harness-refusal" isn't a harness ref.
const RUN_HARNESS = /[Hh]arness[-_]?([Cc]ontext|[Rr]ef|[Ss]pine)s?(?![a-z])|HARNESS_?(CONTEXT|REF|SPINE)S?(?![A-Z])|capture_harness_context/;
const ENDING = /^Closure$|GraphClosure|StepSealClosure|graph_stop_closure|graph_receipt_with_closure|harness_blocks_closure|ClosureNotObject|with_default_closure|ClosureDisposition/;
const LEASE = /OperationHold|HoldAction|HoldEvidence|HOLD_EVIDENCE|DeclaredHold|HarnessProviderHoldFixture|PurchaseHold|purchase_holdings|purchaseHolding|holdPurchaseOf|HoldPurchase|HeldResource|heldValueDigest|HoldOwnerRef|HoldingOwner|held_resource|held_answer|held_connectors|validate_holds|revoke_holding|take_answers_the_hold|take_holds_nothing|purchase_credential_is|held_value_never|connect_holds|connect_could_hold|names_the_held_resource|on_a_held_resource|purchase_connect_holds|holdingStore|HoldingStore|holdings/;
const RUN_HOLD = /RunHold|run_hold|HoldEvent|hold_event|child_hold|suspended_hold|resumed_hold|run_held|NotHeld|held_lock/;

const SWAPS = {
  harness: (sense) => (sense === "run" ? [["harness", "run"]] : [["harnesses", "tests"], ["harness", "test"]]),
  hold: () => [["holdings", "leases"], ["holding", "lease"], ["holds", "leases"], ["hold", "lease"], ["held", "leased"]],
  closure: () => [["closure", "ending"]],
  graph: () => [["graphs", "workflows"], ["graph", "workflow"]],
  runner: () => [["runners", "methods"], ["runner", "method"]],
  agent_session: () => [["agent-session", "agent"], ["agent_session", "agent"], ["AgentSession", "Agent"], ["agentSession", "agent"]],
  agent_act: () => [["AgentAct", "Turn"], ["agent_act", "turn"], ["agentAct", "turn"]],
};

const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The lowercase segments of a name, and each adjacent pair joined. */
function segments(token) {
  const parts = token.split(/[_\-$]+/).flatMap((part) => part.match(/[A-Z]+(?=[A-Z][a-z]|\b|[0-9_])|[A-Z]?[a-z]+|[A-Z]+|[0-9]+/g) ?? []);
  const lower = parts.map((part) => part.toLowerCase());
  return new Set([...lower, ...lower.slice(1).map((part, index) => lower[index] + part)]);
}

/** The renamed words a name holds as whole segments. */
function wordsIn(token) {
  const segs = segments(token);
  return ORDER.filter((word) => FORMS[word].some((form) => segs.has(form)));
}

/** Swaps whole segments, keeping each one's case: snake, kebab, camel and SCREAMING. */
function swap(token, pairs) {
  let out = token;
  for (const [from, to] of pairs) {
    const capFrom = from[0].toUpperCase() + from.slice(1);
    const capTo = to[0].toUpperCase() + to.slice(1);
    out = out.replace(new RegExp(`(?<![A-Za-z])${escape(from)}(?![a-z])`, "g"), to);
    out = out.replace(new RegExp(`${escape(capFrom)}(?![a-z])`, "g"), capTo);
    out = out.replace(new RegExp(`(?<![A-Z])${escape(from.toUpperCase())}(?![A-Z])`, "g"), to.toUpperCase());
  }
  return out;
}

/** One word's verdict on a name: its sense, or that it stays. */
function sense(word, token) {
  if (word === "runner") return STAYS.runner.test(token) ? ["stays", "another runner: stays"] : ["rename", ""];
  if (word === "graph") return STAYS.graph.test(token) ? ["stays", "another graph: stays"] : ["rename", ""];
  if (word === "harness") return RUN_HARNESS.test(token) ? ["run", "the run sense"] : ["test", "the test sense"];
  if (word === "closure") return ENDING.test(token) ? ["rename", "how a run ended"] : ["stays", "a package's closure, or English: stays"];
  if (word === "hold") {
    if (LEASE.test(token)) return ["rename", "the lease family"];
    return RUN_HOLD.test(token) ? ["stays", "a run waiting: stays"] : ["stays", "English, or a skill's own word: stays"];
  }
  return ["rename", ""];
}

/** The proposed row for a name the table doesn't have yet. */
function propose(token) {
  const words = wordsIn(token);
  if (!/[_\-]|[a-z][A-Z]/.test(token)) return { words, to: "?", note: "a bare word: code or prose, read in place" };
  if (token in OVERRIDES) return { words, to: OVERRIDES[token], note: "the plan's name" };
  let to = token;
  const notes = [];
  for (const word of words) {
    const [verdict, note] = sense(word, token);
    if (verdict !== "stays") to = swap(to, SWAPS[word](verdict));
    if (note) notes.push(note);
  }
  return { words, to, note: [...new Set(notes)].join("; ") };
}

function readTable() {
  const lines = readFileSync(TABLE, "utf8").split("\n").filter(Boolean);
  if (lines[0] !== COLUMNS.join("\t")) throw new Error(`identifiers.tsv's header isn't ${COLUMNS.join(", ")}`);
  return lines.slice(1).map((line) => Object.fromEntries(line.split("\t").map((value, index) => [COLUMNS[index], value ?? ""])));
}

function writeTable(rows) {
  const ordered = [...rows].sort((a, b) => a.repo.localeCompare(b.repo) || Number(b.uses || 0) - Number(a.uses || 0) || a.identifier.localeCompare(b.identifier));
  writeFileSync(TABLE, `${[COLUMNS.join("\t"), ...ordered.map((row) => COLUMNS.map((column) => row[column] ?? "").join("\t"))].join("\n")}\n`);
}

// Never renamed here: this program and the converter, which hold the old words
// as their rules, and scafld's own records.
// Vendored copies are replaced by their sync, never edited in place.
const NOT_OURS = /^tools\/convert\/|(^|\/)\.scafld\/|(^|\/)vendor\//;
// A package's files are renamed like any other source, once the converter has
// written them in the 0.12 shape: a name a step, a test or a module shares is
// one name in all of them. So the rename refuses a package file the converter
// hasn't converted yet, which still holds the old shape's keys as its input.
const UNCONVERTED = [
  { file: /(^|\/)run\.yaml$/, old: (text) => !/^minimum_runx:/m.test(text), what: "a run.yaml with no minimum_runx" },
  { file: /(^|\/)connector\.yaml$/, old: (text) => /^id:/m.test(text), what: "a connector.yaml that says id" },
  { file: /\.graph$/, old: () => true, what: "a support graph" },
];
// Signed data verifies as its stored bytes, so a renamed field would break it.
// The count reported as "signed" includes the vectors below.
// Whether a signed fixture is re-signed in the new shape or kept as an old
// receipt that must still verify is its slice's call.
const SIGNED = /"signature"\s*:/;
// Canonicalisation and oracle vectors hold bytes and digests computed over
// their own content, so they're regenerated by their generator, never renamed.
const VECTORS = /canonical-json|oracle|c14n|stable-json/i;
// Applied migrations are history a database has already run, so they're never
// edited or moved: a fresh database would build different names from the ones
// production holds. A renamed name that also appears in one names something
// stored (a table, column, index, constraint or stored value), so its row's
// note starts "stored" and names the new migration that renames the stored
// copy, or the row stays.
const HISTORY = /(^|\/)(migrations|drizzle)\//;
// Files whose own name isn't ours to change: the converter's own files,
// scafld's, and vendored copies, which keep their upstream names.
const KEEPS_NAME = /^tools\/convert\/|(^|\/)\.scafld\/|(^|\/)vendor\//;
// Literals whose exact bytes stored data depends on, in files a token rename
// would otherwise change them in. Each is masked while its file is renamed,
// and a pin that isn't found refuses.
const PINNED = {
  cloud: [
    {
      file: "packages/api/src/payments/holdings-postgres.ts",
      literal: '"purchase-holdings"',
      why: "the lease cipher's key-derivation label: stored ciphertext opens only under these bytes",
    },
  ],
};

// The files the tree holds now, tracked or not, minus what git ignores: the
// converter runs first and writes files git doesn't know yet, and moves or
// deletes some it does.
function workingFiles(root) {
  const listed = execFileSync("git", ["-C", root, "ls-files", "--cached", "--others", "--exclude-standard"], {
    encoding: "utf8",
    maxBuffer: 1 << 28,
  });
  return [...new Set(listed.split("\n").filter(Boolean))].filter((file) => existsSync(path.join(root, file)));
}

function workingSources(root) {
  return workingFiles(root).filter((file) => SOURCE.test(file) && !file.includes("node_modules/") && !/lock/.test(path.basename(file)) && !NOT_OURS.test(file) && !HISTORY.test(file) && !HISTORY_DOCS.test(file) && !GENERATED.test(file));
}

function readSource(root, file) {
  try {
    const text = readFileSync(path.join(root, file), "utf8");
    if (text.length > 2_000_000) return undefined;
    if (file.endsWith(".json") && (SIGNED.test(text) || VECTORS.test(file))) return { signed: true };
    return { text };
  } catch {
    return undefined;
  }
}

/** Each name a source holds. */
function namesOf(root, file) {
  const { text } = readSource(root, file) ?? {};
  return text?.match(TOKEN) ?? [];
}

/** Which of the given names the repo's sources hold. */
function presentNames(root, names) {
  const held = new Set();
  for (const file of workingSources(root)) {
    for (const token of namesOf(root, file)) if (names.has(token)) held.add(token);
  }
  return held;
}

/** Every name holding a renamed word in the repo's tracked sources, with its uses and two files. */
function inventory(root) {
  const found = new Map();
  for (const file of workingSources(root)) {
    for (const token of namesOf(root, file)) {
      if (wordsIn(token).length === 0) continue;
      const entry = found.get(token) ?? { uses: 0, where: new Set() };
      entry.uses += 1;
      if (entry.where.size < 2) entry.where.add(file);
      found.set(token, entry);
    }
  }
  return found;
}

function scan(repo, root) {
  const rows = readTable();
  const known = new Set(rows.filter((row) => row.repo === repo).map((row) => row.identifier));
  let added = 0;
  for (const [token, { uses, where }] of inventory(root)) {
    if (known.has(token)) continue;
    const { words, to, note } = propose(token);
    rows.push({ repo, identifier: token, uses: String(uses), words: words.join(","), to, note, where: [...where].sort().join(" ") });
    added += 1;
  }
  writeTable(rows);
  process.stdout.write(`${repo}: ${added} names added to identifiers.tsv\n`);
  return 0;
}

function apply(repo, root, write, list, showStays) {
  const rows = readTable().filter((row) => row.repo === repo);
  const known = new Set(rows.map((row) => row.identifier));
  const renames = new Map(rows.filter((row) => row.to !== "?" && row.to !== row.identifier).map((row) => [row.identifier, row.to]));
  const unconverted = workingFiles(root)
    .filter((file) => !NOT_OURS.test(file) && !file.includes("node_modules/"))
    .flatMap((file) => UNCONVERTED.filter((rule) => rule.file.test(file) && rule.old(readFileSync(path.join(root, file), "utf8"))).map((rule) => `${file}   (${rule.what})`));
  if (unconverted.length > 0) {
    process.stdout.write(`refused ${repo}: ${unconverted.length} package files aren't in the 0.12 shape; the rename reads what the converter wrote, so convert them first\n`);
    for (const line of unconverted.slice(0, 40)) process.stdout.write(`  ${line}\n`);
    return 1;
  }
  // A chain renames a name to another's old name: AgentActInvocation becomes
  // TurnInvocation as TurnInvocation becomes TurnView. A tree holding the
  // chain's first name hasn't been renamed, so both apply at once. A tree
  // holding none of them has, so the middle name is already the new one and
  // stays, and the rename can run again over its own output.
  const inTree = presentNames(root, new Set(rows.map((row) => row.identifier)));
  const settled = [];
  for (const [name] of renames) {
    const from = rows.filter((row) => row.to === name && row.identifier !== name && renames.has(row.identifier)).map((row) => row.identifier);
    if (from.length > 0 && !from.some((first) => inTree.has(first))) settled.push(name);
  }
  for (const name of settled) renames.delete(name);
  const missing = [...inventory(root).keys()].filter((token) => !known.has(token));
  if (missing.length > 0) {
    process.stdout.write(`refused ${repo}: ${missing.length} names hold a renamed word and aren't in identifiers.tsv; run scan and decide them\n`);
    for (const token of missing.slice(0, 40)) process.stdout.write(`  ${token}\n`);
    return 1;
  }
  const pins = PINNED[repo] ?? [];
  for (const pin of pins) {
    if (!readSource(root, pin.file)?.text?.includes(pin.literal)) {
      process.stdout.write(`refused ${repo}: the pinned ${pin.literal} isn't in ${pin.file} (${pin.why}); find where it went and pin it there\n`);
      return 1;
    }
  }
  const notes = new Map(rows.map((row) => [row.identifier, row.note]));
  const stored = new Map();
  for (const file of workingFiles(root).filter((file) => HISTORY.test(file) && SOURCE.test(file))) {
    for (const name of new Set(readFileSync(path.join(root, file), "utf8").match(TOKEN) ?? [])) {
      if (renames.has(name) && !notes.get(name).startsWith("stored")) stored.set(name, file);
    }
  }
  if (stored.size > 0) {
    process.stdout.write(`refused ${repo}: ${stored.size} renamed names are stored, because an applied migration holds them; each row's note starts "stored" and names the migration that renames the stored copy, or the row stays\n`);
    for (const [name, file] of [...stored].slice(0, 40)) process.stdout.write(`  ${name}   (${file})\n`);
    return 1;
  }
  // A name an outside package defines is that package's, so it never changes
  // here: an import from anything but a relative path or runx's own packages.
  const outside = new Map();
  for (const file of workingSources(root).filter((file) => /\.(ts|tsx|mts|mjs|js|cjs|astro)$/.test(file))) {
    for (const m of readSource(root, file)?.text?.matchAll(/import\s*(?:type\s*)?\{([^}]*)\}\s*from\s*["']([^"'.][^"']*)["']/g) ?? []) {
      if (/^(@runxhq\/|@runx\/|runx)/.test(m[2])) continue;
      for (const name of m[1].split(",").map((part) => part.trim().replace(/^type\s+/, "").split(/\s+as\s+/)[0])) {
        if (renames.has(name)) outside.set(name, `${m[2]}, in ${file}`);
      }
    }
  }
  if (outside.size > 0) {
    process.stdout.write(`refused ${repo}: ${outside.size} renamed names are imported from outside packages, which own them; each row stays\n`);
    for (const [name, from] of [...outside].slice(0, 40)) process.stdout.write(`  ${name}   (${from})\n`);
    return 1;
  }
  // A new name a code file already holds needs a row for the name it lands
  // on: moved aside, or kept with a note that the two are one thing. Clashes
  // across files are the compiler's to catch, and prose landing on a word it
  // already uses, such as "tests", is only English.
  const collisions = new Map();
  for (const file of workingSources(root).filter((file) => !file.endsWith(".md"))) {
    const names = new Set(readSource(root, file)?.text?.match(TOKEN) ?? []);
    for (const name of names) {
      const to = renames.get(name);
      if (to !== undefined && names.has(to) && !known.has(to)) collisions.set(`${name} -> ${to}`, file);
    }
  }
  if (collisions.size > 0) {
    process.stdout.write(`refused ${repo}: ${collisions.size} new names land on a name the same file already has, and the table doesn't say what happens to it\n`);
    for (const [clash, file] of [...collisions].slice(0, 40)) process.stdout.write(`  ${clash}   (${file})\n`);
    return 1;
  }
  let files = 0;
  let replaced = 0;
  let signed = 0;
  // The bare words (words.mjs): each one renamed unless a stay keeps it, the
  // stays listed for review, and each code file where a renamed bare word
  // lands on a name the file already holds, which only its compiler or tests
  // can clear.
  let bare = 0;
  const stays = [];
  const landsOn = new Set();
  const fileNames = new Map();
  const names = (file, text) => {
    if (!fileNames.has(file)) fileNames.set(file, new Set(text.match(TOKEN) ?? []));
    return fileNames.get(file);
  };
  for (const file of workingSources(root)) {
    const source = readSource(root, file);
    if (source?.signed) signed += 1;
    if (source?.text === undefined) continue;
    const held = pins.filter((pin) => pin.file === file);
    const text = held.reduce((masked, pin, index) => masked.split(pin.literal).join(`\u0000${index}\u0000`), source.text);
    const full = path.join(root, file);
    let count = 0;
    const lines = text.split("\n");
    const renamed = lines
      .map((line, number) =>
        line.replace(TOKEN, (token, index) => {
          const to = renames.get(token);
          if (to !== undefined) {
            count += 1;
            return to;
          }
          if (bareWord(token) === undefined) return token;
          const stay = stayFor(file, line, index, token);
          if (stay) {
            stays.push(`${file}:${number + 1}\t${stay.why}\t${line.trim().slice(0, 140)}`);
            return token;
          }
          count += 1;
          bare += 1;
          const next = renamedBare(token, line.slice(index + token.length));
          if (/\.(rs|ts|tsx|mts|mjs|js|cjs)$/.test(file) && names(file, text).has(next)) landsOn.add(`${file}\t${token} -> ${next}`);
          return next;
        }),
      )
      .join("\n");
    const next = held.reduce((restored, pin, index) => restored.split(`\u0000${index}\u0000`).join(pin.literal), renamed);
    if (count === 0) continue;
    files += 1;
    replaced += count;
    if (list) process.stdout.write(`${count}\t${file}\n`);
    if (write) writeFileSync(full, next);
  }
  // A name that's also a file or folder moves with it: a bench, a module and
  // its file, a test. Each segment of a path, a folder or a file's stem, is
  // renamed when the table renames that exact name. A renamed folder moves
  // whole, its run.yaml and vendored files included, so no package is split.
  // A file keeps its own name only where the name isn't ours to change.
  const moves = new Map();
  const stranded = [];
  for (const file of workingFiles(root)) {
    const parts = file.split("/");
    const keepsName = KEEPS_NAME.test(file);
    const renamed = parts.map((part, index) => {
      const last = index === parts.length - 1;
      if (last && keepsName) return part;
      const dot = last ? part.indexOf(".") : -1;
      const stem = dot > 0 ? part.slice(0, dot) : part;
      let to = renames.get(stem);
      const upTo = parts.slice(0, index + 1).join("/") + (last ? "" : "/");
      if (to === undefined && bareWord(stem) !== undefined && !STAYING_PATHS.test(upTo) && !HISTORY_DOCS.test(file)) to = renamedBare(stem, "");
      return to === undefined ? part : to + (dot > 0 ? part.slice(dot) : "");
    });
    const target = renamed.join("/");
    if (target === file) continue;
    if (HISTORY.test(file)) stranded.push(file);
    else moves.set(file, target);
  }
  if (stranded.length > 0) {
    process.stdout.write(`refused ${repo}: ${stranded.length} applied migrations sit under a renamed path\n`);
    for (const file of stranded.slice(0, 20)) process.stdout.write(`  ${file}\n`);
    return 1;
  }
  const targets = new Set(moves.values());
  const taken = [...targets].filter((target) => existsSync(path.join(root, target)) && !moves.has(target));
  if (taken.length > 0) {
    process.stdout.write(`refused ${repo}: ${taken.length} renamed paths already exist\n`);
    for (const target of taken.slice(0, 20)) process.stdout.write(`  ${target}\n`);
    return 1;
  }
  for (const [from, to] of moves) {
    if (list) process.stdout.write(`move\t${from}\t${to}\n`);
    if (!write) continue;
    mkdirSync(path.dirname(path.join(root, to)), { recursive: true });
    renameSync(path.join(root, from), path.join(root, to));
  }
  // A folder the moves emptied goes too, and so does a parent it empties.
  if (write) {
    for (const from of moves.keys()) {
      for (let dir = path.dirname(from); dir !== "."; dir = path.dirname(dir)) {
        const full = path.join(root, dir);
        if (!existsSync(full) || readdirSync(full).length > 0) break;
        rmdirSync(full);
      }
    }
  }
  if (showStays) {
    for (const stay of stays) process.stdout.write(`stays\t${stay}\n`);
    for (const clash of landsOn) process.stdout.write(`lands on a held name\t${clash}\n`);
  }
  process.stdout.write(`${repo}: ${replaced} names in ${files} files, ${bare} of them bare words, ${stays.length} bare words kept, ${moves.size} paths moved${write ? "" : " (dry run)"}; ${signed} signed files left as signed${settled.length ? `; ${settled.join(", ")} already the new names` : ""}${landsOn.size ? `; ${landsOn.size} renamed bare words land on a name their file holds` : ""}\n`);
  return 0;
}

function main([command, repo, root, ...rest]) {
  if (!["scan", "apply"].includes(command) || !repo || !root) {
    process.stderr.write("usage: rename.mjs scan <repo> <root> | apply <repo> <root> [--write] [--list] [--stays]\n");
    return 2;
  }
  return command === "scan" ? scan(repo, root) : apply(repo, root, rest.includes("--write"), rest.includes("--list"), rest.includes("--stays"));
}

process.exitCode = main(process.argv.slice(2));
