#!/usr/bin/env node
// Renames 0.12's words in code, from one table of exact identifiers per repo.
// The plan's §4.2 and the rename table (plans/0.12/rename-table.tsv) own the
// words; identifiers.tsv beside this file is the authority for every name.
//
//   node tools/convert/rename.mjs scan  <repo> <root>             add rows for names not in the table yet
//   node tools/convert/rename.mjs apply <repo> <root> [--write] [--list]   rename, refusing while a name is missing;
//                                                               --list prints each file and its count
//
// `scan` keeps every row already in the table, so a decision made by hand
// stays made. `apply` replaces whole identifiers only, all at once, so a chain
// such as TurnInvocation to TurnView and AgentActInvocation to TurnInvocation
// can't run into itself. A bare word ("runner" alone) is code or prose and is
// read in place, never replaced here.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const TABLE = new URL("./identifiers.tsv", import.meta.url);
const COLUMNS = ["repo", "identifier", "uses", "words", "to", "note", "where"];
const SOURCE = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|json|ya?ml|toml|sql|rb|py|sh)$/;
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
  ExecutionGraph: "Workflow", GraphStep: "WorkflowStep", SequentialGraph: "Workflow", parse_graph_yaml: "parse_workflow_yaml",
  graph_step_failed: "workflow_step_failed", graph_blocked: "workflow_blocked", TurnInvocation: "TurnView", TurnEnvelope: "TurnViewEnvelope",
  AgentAct: "Turn", AgentActInvocation: "TurnInvocation", HarnessTurnScript: "TestTurnCalls", "agent-session": "agent", $held: "$lease",
};

// The other senses each word has, which stay.
const STAYS = {
  runner: /^\$RUNNER_|ToolRunner|tool_runner|test_runner|local-pr-runner|^step_runner$|^stage_runner$/,
  graph: /GraphQL|dependency-advisory-graph/,
};
const RUN_HARNESS = /harness[-_]?(context|ref|spine)|capture_harness_context/i;
const ENDING = /^Closure$|GraphClosure|StepSealClosure|graph_stop_closure|graph_receipt_with_closure|harness_blocks_closure|ClosureInvalid|ClosureMissing|ClosureNotObject|serialize_closure|with_default_closure|ClosureDisposition/;
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
// as their rules; the package files the converter owns and runs on first; and
// scafld's own records.
const NOT_OURS = /^tools\/convert\/|(^|\/)\.scafld\/|(^|\/)(run|connector)\.yaml$|\.graph$|(^|\/)(fixtures|tests)\/[^/]+\.ya?ml$/;
// Signed data verifies as its stored bytes, so a renamed field would break it.
// Whether a signed fixture is re-signed in the new shape or kept as an old
// receipt that must still verify is its slice's call.
const SIGNED = /"signature"\s*:/;

function trackedSources(root) {
  return execFileSync("git", ["-C", root, "ls-files"], { encoding: "utf8", maxBuffer: 1 << 28 })
    .split("\n")
    .filter((file) => SOURCE.test(file) && !file.includes("node_modules/") && !/lock/.test(path.basename(file)) && !NOT_OURS.test(file));
}

function readSource(root, file) {
  try {
    const text = readFileSync(path.join(root, file), "utf8");
    if (text.length > 2_000_000) return undefined;
    if (file.endsWith(".json") && SIGNED.test(text)) return { signed: true };
    return { text };
  } catch {
    return undefined;
  }
}

/** Every name holding a renamed word in the repo's tracked sources, with its uses and two files. */
function inventory(root) {
  const found = new Map();
  for (const file of trackedSources(root)) {
    const { text } = readSource(root, file) ?? {};
    if (text === undefined) continue;
    for (const token of text.match(TOKEN) ?? []) {
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

function apply(repo, root, write, list) {
  const rows = readTable().filter((row) => row.repo === repo);
  const known = new Set(rows.map((row) => row.identifier));
  const renames = new Map(rows.filter((row) => row.to !== "?" && row.to !== row.identifier).map((row) => [row.identifier, row.to]));
  const missing = [...inventory(root).keys()].filter((token) => !known.has(token));
  if (missing.length > 0) {
    process.stdout.write(`refused ${repo}: ${missing.length} names hold a renamed word and aren't in identifiers.tsv; run scan and decide them\n`);
    for (const token of missing.slice(0, 40)) process.stdout.write(`  ${token}\n`);
    return 1;
  }
  // A new name a file already holds needs a row for the name it lands on:
  // moved aside, or kept with a note that the two are one thing. Clashes
  // across files are the compiler's to catch.
  const collisions = new Map();
  for (const file of trackedSources(root)) {
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
  for (const file of trackedSources(root)) {
    const source = readSource(root, file);
    if (source?.signed) signed += 1;
    if (source?.text === undefined) continue;
    const { text } = source;
    const full = path.join(root, file);
    let count = 0;
    const next = text.replace(TOKEN, (token) => {
      const to = renames.get(token);
      if (to === undefined) return token;
      count += 1;
      return to;
    });
    if (count === 0) continue;
    files += 1;
    replaced += count;
    if (list) process.stdout.write(`${count}\t${file}\n`);
    if (write) writeFileSync(full, next);
  }
  process.stdout.write(`${repo}: ${replaced} names in ${files} files${write ? "" : " (dry run)"}; ${signed} signed files left as signed\n`);
  return 0;
}

function main([command, repo, root, ...rest]) {
  if (!["scan", "apply"].includes(command) || !repo || !root) {
    process.stderr.write("usage: rename.mjs scan <repo> <root> | apply <repo> <root> [--write] [--list]\n");
    return 2;
  }
  return command === "scan" ? scan(repo, root) : apply(repo, root, rest.includes("--write"), rest.includes("--list"));
}

process.exitCode = main(process.argv.slice(2));
