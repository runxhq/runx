// The bare words 0.12 retires in one sense and keeps in another, and the
// short, exact list of where each keeps it (plan §4.2, §6.1, §6.5). Every
// other use is renamed: `runner` meaning a skill's method becomes `method`,
// `harness` meaning a scripted test becomes `test`, or `run` in its run
// sense, and `graph` meaning a method's steps becomes `workflow`
// (vocabulary-and-loops.md §2). rename.mjs applies it, and stale-words.mjs
// fails on what's left.

export const WORDS = {
  runner: { runner: "method", runners: "methods" },
  harness: { harness: "test", harnesses: "tests" },
  graph: { graph: "workflow", graphs: "workflows" },
};

// A word renamed where a person or a program reads it as code or prose, but
// never where it's data: in code, its identifiers and comments; in prose,
// Markdown outside its code and the strings a person reads; and in a path
// that names a module or folder the rename moves. A string a program reads
// as a value, such as a wire value or a folder's name, keeps its bytes, and a
// fenced or inline code span quotes code, a command or a file as it is.
export const PROSE_ONLY = new Set(["graph"]);

/**
 * The paths the rename moves whole, each to the name its own module, document
 * or folder gives it. A folder ends in a slash and moves with everything under
 * it; a file moves alone. The rest of a moved path is then renamed segment by
 * segment, as any path is, so a file the table renames takes its new name in
 * its new folder.
 *
 * - graph's: the parser's workflow module, the runtime's two workflow files
 *   and the test fixtures' folder of workflow packages. A skill's own folder
 *   of stage packages is `workflow/` by a ruling made by hand with the
 *   refusals that name it, not by this table.
 * - The act engine, by its own doc, and its workflow steps (engine/steps.rs,
 *   by the table's workflow_engine row, since execution/workflow.rs exists).
 * - The test replay, which loads fixtures, invokes adapters, asserts receipts
 *   and seals workflow replay.
 * - The kernel fixture evaluation, whose error is kernel.fixture.evaluation_failed.
 */
export const MOVED_PATHS = [
  ["crates/runx-parser/src/graph.rs", "crates/runx-parser/src/workflow.rs"],
  ["crates/runx-parser/src/graph/", "crates/runx-parser/src/workflow/"],
  ["crates/runx-runtime/src/execution/graph.rs", "crates/runx-runtime/src/execution/workflow.rs"],
  ["crates/runx-runtime/src/execution/skill_front/graph.rs", "crates/runx-runtime/src/execution/skill_front/workflow.rs"],
  ["fixtures/graphs/", "fixtures/workflows/"],
  ["crates/runx-runtime/src/execution/runner.rs", "crates/runx-runtime/src/execution/engine.rs"],
  ["crates/runx-runtime/src/execution/runner/", "crates/runx-runtime/src/execution/engine/"],
  ["crates/runx-runtime/src/execution/test/runner.rs", "crates/runx-runtime/src/execution/test/replay.rs"],
  ["fixtures/kernel/runner/", "fixtures/kernel/evaluation/"],
];

/** Where MOVED_PATHS moves a path, or the path itself when no move holds it. */
export function movedPath(file) {
  const move = MOVED_PATHS.find(([from]) => (from.endsWith("/") ? file.startsWith(from) : file === from));
  return move ? move[1] + file.slice(move[0].length) : file;
}

// A prose-only word in a reference to one of graph's moved paths renames with
// the path, though a path is otherwise code a prose-only word keeps.
const MOVED_REFERENCE = /runx-parser\/src\/graph|src\/graph\/|execution\/graph\.rs|skill_front\/graph\.rs|fixtures\/graphs/;

/** Whether the token at `at` is part of a path naming one of the moved paths. */
export function inMovedPath(text, at) {
  let from = at;
  let to = at;
  while (from > 0 && /[\w./-]/.test(text[from - 1])) from -= 1;
  while (to < text.length && /[\w./-]/.test(text[to])) to += 1;
  return MOVED_REFERENCE.test(text.slice(from, to));
}

/**
 * Where a prose-only word renames in one file: a predicate over a token's
 * offset. Markdown renames it outside its code; a code file outside its
 * strings, in its identifiers and comments, and in a string a person reads;
 * any file in a path the rename moves. Data, a JSON or YAML value, renames it
 * only in such a path, so a wire value or a key keeps its bytes.
 */
export function proseSites(file, text) {
  if (file.endsWith(".md") || /(^|\/)llms\.txt$/.test(file)) {
    const code = markdownCode(text);
    return (at) => inMovedPath(text, at) || !code.some(([from, to]) => at >= from && at < to);
  }
  if (STRINGS.test(file)) {
    const literals = stringLiterals(text, file).map(({ pieces }) => [pieces[0][0], pieces.at(-1)[1]]);
    const prose = codeProse(text, file);
    return (at, token) => {
      if (inMovedPath(text, at)) return true;
      if (!literals.some(([from, to]) => at >= from && at < to)) return true;
      // A format placeholder, {name} or {name:?}, names the code's variable.
      if (text[at - 1] === "{" && /^[}:.]/.test(text.slice(at + token.length))) return true;
      return prose.some(([from, to]) => at >= from && at < to) && !inPath(text, at, token);
    };
  }
  if (/\.(astro|css|scss|sql|rb|sh|mk)$|(^|\/)Makefile$/.test(file)) return () => true;
  return (at) => inMovedPath(text, at);
}

/** Each fenced code block in a Markdown text: the span between its fences. */
export function fencedBlocks(text) {
  const blocks = [];
  let open;
  for (const fence of text.matchAll(/^ {0,3}(`{3,}|~{3,})([^\n]*)$/gm)) {
    const [, marks, info] = fence;
    if (!open) {
      if (marks[0] === "`" && info.includes("`")) continue;
      open = { mark: marks[0], length: marks.length, start: fence.index + fence[0].length + 1 };
    } else if (marks[0] === open.mark && marks.length >= open.length && info.trim() === "") {
      blocks.push([open.start, fence.index]);
      open = undefined;
    }
  }
  return blocks;
}

// Code whose string literals a person reads: an error, a message, help text.
export const STRINGS = /\.(rs|ts|tsx|mts|cts|js|jsx|mjs|cjs|go|py)$/;

/**
 * The prose in a code file: each string literal's text that reads as words, a
 * phrase with a space in it, outside its placeholders. A one-word string is a
 * wire value or a key, a placeholder such as {graph} or ${graph} names code,
 * a literal holding a document, YAML, front matter or Markdown a test writes,
 * is data, and a comment is no one's output, so none of them is prose.
 */
export function codeProse(text, file) {
  const spans = [];
  for (const { pieces, template } of stringLiterals(text, file)) {
    if (template) {
      // A template literal's text, outside its ${...} expressions: prose
      // when the whole literal reads as words.
      if (/\s/.test(pieces.map(([a, b]) => text.slice(a, b)).join("x").trim())) spans.push(...pieces);
      continue;
    }
    const [[from, to]] = pieces;
    const body = text.slice(from, to);
    if (!/\s/.test(body.trim()) || isDocument(body)) continue;
    // Placeholders name code: {name}, {name:?}, {0} and Python's {name!r}.
    let at = from;
    for (const m of body.matchAll(/\{[A-Za-z_0-9][^{}]*\}/g)) {
      spans.push([at, from + m.index]);
      at = from + m.index + m[0].length;
    }
    spans.push([at, to]);
  }
  return spans;
}

/**
 * Each string literal in a code file, outside its comments: its text, or a
 * template literal's text pieces between its ${...} expressions.
 */
export function stringLiterals(text, file) {
  const ext = file.slice(file.lastIndexOf("."));
  const rust = ext === ".rs";
  const py = ext === ".py";
  const go = ext === ".go";
  const js = /^\.(ts|tsx|mts|cts|js|jsx|mjs|cjs)$/.test(ext);
  const literals = [];
  const keep = (from, to) => literals.push({ pieces: [[from, to]], template: false });
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    const n = text[i + 1];
    if (!py && c === "/" && n === "/") { const e = text.indexOf("\n", i); i = e < 0 ? text.length : e; continue; }
    if (!py && c === "/" && n === "*") { const e = text.indexOf("*/", i + 2); i = e < 0 ? text.length : e + 2; continue; }
    if (py && c === "#") { const e = text.indexOf("\n", i); i = e < 0 ? text.length : e; continue; }
    if (rust) {
      const raw = /^b?r(#*)"/.exec(text.slice(i, i + 12));
      if (raw && !/[A-Za-z0-9_]/.test(text[i - 1] ?? "")) {
        const close = `"${raw[1]}`;
        const from = i + raw[0].length;
        const e = text.indexOf(close, from);
        const to = e < 0 ? text.length : e;
        keep(from, to);
        i = to + close.length;
        continue;
      }
      const char = /^'(\\.[^']{0,8}|[^\\'\n])'/.exec(text.slice(i, i + 14));
      if (c === "'" && char) { i += char[0].length; continue; }
      if (c === "'") { i += 1; continue; }
    }
    if (py && (text.startsWith('"""', i) || text.startsWith("'''", i))) {
      const q = text.slice(i, i + 3);
      const e = text.indexOf(q, i + 3);
      const to = e < 0 ? text.length : e;
      keep(i + 3, to);
      i = to + 3;
      continue;
    }
    if (c === "`" && js) {
      // A template literal's text, in pieces between its ${...} expressions.
      let j = i + 1;
      let from = j;
      const pieces = [];
      while (j < text.length && text[j] !== "`") {
        if (text[j] === "\\") { j += 2; continue; }
        if (text.startsWith("${", j)) {
          pieces.push([from, j]);
          let depth = 1;
          j += 2;
          while (j < text.length && depth > 0) { if (text[j] === "{") depth += 1; else if (text[j] === "}") depth -= 1; j += 1; }
          from = j;
          continue;
        }
        j += 1;
      }
      pieces.push([from, j]);
      literals.push({ pieces, template: true });
      i = j + 1;
      continue;
    }
    if (c === '"' || (c === "'" && (js || py)) || (c === "`" && go)) {
      let j = i + 1;
      while (j < text.length && text[j] !== c) {
        if (text[j] === "\\" && c !== "`") j += 1;
        else if (text[j] === "\n" && c !== "`" && !rust) break;
        j += 1;
      }
      keep(i + 1, j);
      i = j + 1;
      continue;
    }
    i += 1;
  }
  return literals;
}

/**
 * Whether a string literal's text is a document: a line of it starts as YAML,
 * front matter, a list item, a heading, or JSON. A line that starts with a
 * format placeholder, such as a message's `{stdout}`, starts no document.
 */
function isDocument(body) {
  return body
    .split(/\\n|\n/)
    .some((line) => /^\s*(-\s|---\s*$|[A-Za-z_$][\w$-]*:(\s|$)|#{1,6}\s|[{[])/.test(line) && !/^\s*\{[A-Za-z_0-9][^{}\s]*\}/.test(line));
}

/** Whether a token in a string is a path segment or a file name: beside a slash, or before an extension. */
export function inPath(text, at, token) {
  const before = text[at - 1] ?? "";
  const after = text.slice(at + token.length, at + token.length + 2);
  return before === "/" || before === "." || after[0] === "/" || /^\.[A-Za-z]/.test(after);
}

/** The spans of a Markdown text that are code, not prose: fenced blocks, inline code and link targets. */
export function markdownCode(text) {
  const spans = fencedBlocks(text);
  const blanked = spans.reduce((out, [from, to]) => out.slice(0, from) + " ".repeat(to - from) + out.slice(to), text);
  for (const m of blanked.matchAll(/(`+)(?:(?!\1)[\s\S])*?\1/g)) spans.push([m.index, m.index + m[0].length]);
  for (const m of blanked.matchAll(/\]\(([^)\s]*)|^\s*\[[^\]]+\]:\s*(\S+)|<(https?:[^>\s]+)>/gm)) spans.push([m.index, m.index + m[0].length]);
  return spans;
}

// A bare form of a retired word, in any case, and the word it is.
export function bareWord(token) {
  const lower = token.toLowerCase();
  for (const [word, forms] of Object.entries(WORDS)) {
    // Its own forms only: `in` would also find what every object inherits,
    // such as `constructor`.
    if (!Object.hasOwn(forms, lower)) continue;
    if (token === lower || token === lower.toUpperCase() || token === lower[0].toUpperCase() + lower.slice(1)) return word;
  }
  return undefined;
}

// `harness` before context, ref, spine or control is the run sense (§4.2):
// the whole word, so a harness that refuses or a reference stays the test sense.
const RUN_SENSE = /^[ _-](context|ref|spine|control)s?(?![A-Za-z])/i;

/** The bare token's new form, in its case. */
export function renamedBare(token, after) {
  const lower = token.toLowerCase();
  const word = bareWord(token);
  let to = WORDS[word][lower];
  if (word === "harness" && RUN_SENSE.test(after)) to = lower === "harnesses" ? "runs" : "run";
  if (token === lower.toUpperCase()) return to.toUpperCase();
  if (token[0] !== lower[0]) return to[0].toUpperCase() + to.slice(1);
  return to;
}

// Where each word keeps its old sense. A stay names its reason, the files it
// holds in (all when it says none), the exact token when its case decides it,
// and what must sit just before or just after the word on its line.
export const STAYS = [
  // runner names nothing of runx's own: the act engine, the test replay and
  // the kernel fixture evaluation are their own words (bare-words.tsv), and
  // the operational policy's runners await a name of their own.
  { word: "runner", why: "the operational policy's runners", file: /^(crates\/runx-contracts\/src\/operational_policy(\.rs$|\/)|crates\/runx-contracts\/tests\/operational_policy\.rs$|crates\/runx-cli\/(src|tests)\/policy\.rs$|packages\/contracts\/src\/schemas\/operational-policy\.|fixtures\/operational-policy\/|skills\/policy-draft\/|tests\/policy-draft-validation\.test\.ts$)/ },
  { word: "runner", why: "the operational policy's runners, in a corpus or test that holds one policy among others", file: /^(crates\/runx-contracts\/tests\/schema_wire_conformance\/corpora\.rs|packages\/contracts\/src\/index\.test\.ts)$/, before: /^\s*"?$/, after: /^"?: \[\{/ },
  { word: "runner", why: "GitHub's runner", file: /^\.github\// },
  { word: "runner", why: "GitHub's runner", before: /(GitHub(['’]s)?|[Hh]osted|self-hosted|Actions|CI) $/ },
  // A test runner: libtest's, Vitest's or node's. runx's own is its test replay.
  { word: "runner", why: "a test runner", before: /(?<!\bnative )\btest $/i },
  { word: "runner", why: "the retired flag a test proves is refused", file: /^crates\/runx-cli\/tests\/router\.rs$/, before: /--$/ },
  // runner in Ausca: a GPU job runner, its sandbox's own runner, and GitHub's
  // runner limit beside the buyer canary. The canary itself and a deploy
  // script's command take their own words (bare-words.tsv).
  { word: "runner", why: "a GPU job runner", before: /\bGPU job $/ },
  { word: "runner", why: "the code-execution sandbox's own runner", file: /^docs\/plans\/code-execution\.md$/ },
  { word: "runner", why: "GitHub's runner limit, which bounds the buyer canary", file: /^scripts\/buyer-canary(\.test)?\.mjs$/, after: /^ limit\b/ },
  // graph: another graph than a method's steps, which keeps the word.
  { word: "graph", why: "a dependency graph", before: /\b(crate|package|dependency|module|import)s? $/i },
  { word: "graph", why: "a call graph", before: /\bcall $/i },
  { word: "graph", why: "a knowledge graph", before: /\b(knowledge|claim|entity|semantic) $/i },
  { word: "graph", why: "a graph database", after: /^ (database|databases|db|store|query)\b/i },
  { word: "graph", why: "a chart", before: /\b(bar|line|pie|time-series|latency|throughput) $/i },
  // harness: Cargo's own key in a [[bench]] or [[test]] table, the only
  // tables that take it: whether libtest's harness runs the target.
  { word: "harness", why: "Cargo's harness key, which says whether libtest runs a bench or test target", file: /(^|\/)Cargo\.toml$/, before: /^\s*$/, after: /^\s*=/ },
  // The same key as written, `harness = false`, in the check that holds
  // every bench target to it and in that check's test.
  { word: "harness", why: "Cargo's key as written, harness = false, which the bench targets check reads", file: /^scripts\/check-bench-targets(\.test)?\.mjs$/, after: /^\s*=\s*false\b/ },
  // harness: the retired command a test names to prove native help dropped it.
  { word: "harness", why: "the retired command native help must not advertise", file: /^crates\/runx-cli\/tests\/router\.rs$/, before: /\b(runx|advertise) $/ },
  // graph: JSON-LD's @graph keyword, and Microsoft Graph, a product's name.
  { word: "graph", why: "JSON-LD's @graph keyword", before: /@$/ },
  { word: "graph", why: "Microsoft Graph, a product's name", token: "Graph", before: /\b(Microsoft|through) $/ },
  // runner: Rails' runner command, which runs a script in the app.
  { word: "runner", why: "Rails' runner command", before: /\brails $/i },
  // runner: a consumer's own runx operational policy, whose runners are
  // machinery as OSS's are, and the code that reads it.
  { word: "runner", why: "the operational policy's runners, in a consumer's own policy", file: /^(config\/runx-frantic-policy\.json|\.runx\/runx-issue-flow\.json)$/ },
  { word: "runner", why: "the operational policy's runners, as the code reading a consumer's policy names them", file: /^scripts\/runx-issue-flow\.test\.mjs$/ },
  // harness: a product's own production check, which its dogfood target
  // verifies (Nitrosend's Makefile help), not a runx test.
  { word: "harness", why: "a product's own production check harness, which a dogfood target verifies", file: /^Makefile$/, before: /\bproduction $/ },
  // harness: an agent's host, such as Claude Code or Codex (D23, D39).
  { word: "harness", why: "an agent's host", before: /\b(agent|coding) $/i },
  { word: "harness", why: "an agent's host", after: /^(['’]s own | probe)/ },
];

/** The stay that keeps this occurrence, if one does. */
export function stayFor(file, line, index, token) {
  const word = bareWord(token);
  if (word === undefined) return undefined;
  const before = line.slice(0, index);
  const after = line.slice(index + token.length);
  return STAYS.find((stay) => stay.word === word && (!stay.token || stay.token === token) && (!stay.file || stay.file.test(file)) && (!stay.before || stay.before.test(before)) && (!stay.after || stay.after.test(after)));
}

/**
 * The rows of bare-words.tsv that decide this occurrence: each names one
 * exact token in one file, or in every file when its file is `*`, at every
 * site or only where its `before` and `after` match, as a stay's do. More
 * than one is a conflict for the caller to refuse.
 */
export function decisionsFor(rows, file, line, index, token) {
  const before = line.slice(0, index);
  const after = line.slice(index + token.length);
  return rows.filter((row) => (row.file === file || row.file === "*") && row.word === token && (!row.before || row.before.test(before)) && (!row.after || row.after.test(after)));
}

// The paths that keep a retired word, each for a meaning outside runx, and
// what the rename and the guard read them by. A history document keeps its
// whole path as it keeps its text (HISTORY), release/notes/ among them.
export const PATH_STAYS = [
  { path: /^\.github\//, why: "GitHub's runner" },
  { path: /^scripts\/check-rust-crate-graph\.mjs$/, why: "a dependency graph" },
  // Not renamed yet: the policy's runners await a name (the operational policy's stay above).
  { path: /^fixtures\/operational-policy\/invalid-(no-available|unknown)-runner\.json$/, why: "the operational policy's runners" },
];

/** The stay that keeps a path's retired word, if one does. */
export function pathStayFor(file) {
  return HISTORY.test(file) ? { why: "history" } : PATH_STAYS.find((stay) => stay.path.test(file));
}

/**
 * The retired word a path's own name holds, the last component of a file or
 * a folder, split on `.`, `-` and `_`: its word and the component.
 */
export function retiredInPath(file) {
  const name = file.replace(/\/$/, "").split("/").at(-1);
  for (const piece of name.split(/[._-]+/)) {
    const word = bareWord(piece.toLowerCase());
    if (word !== undefined) return { word, name };
  }
  return undefined;
}

// A lockfile is its package manager's: Cargo.lock, pnpm-lock.yaml,
// package-lock.json, packets.lock.json and their kind. A name that only holds
// the letters, such as a test's blocked-push.yaml, is a source like any other.
export const LOCKFILE = /(^|[.-])lock\.(json|ya?ml)$|\.lockb?$/;

// Files a generator writes, regenerated after a rename and never edited.
// Cloud's two API schema modules are scripts/generate-public-schemas.ts's, and
// docs/api-surface.md is scripts/gen-api-index.ts's.
export const GENERATED = /^(schemas\/[^/]+\.schema\.json|docs\/api-surface\.md|packages\/contracts\/src\/generated\/.+|packages\/contracts\/src\/schema-artifacts\.ts|crates\/runx-runtime\/bundled-skills\.json|packages\/api\/src\/generated\/(public-protocol-schemas|purchase-schemas)\.ts)$|\.gen\.go$|^contracts\/generated\//;

// The sources a rename reads, and the guard with it: code, data and prose
// by extension, Go and stylesheets among them, and Makefiles by name. A
// stylesheet's class and id names are names like any other, so a class an
// element takes renames in the rules that style it.
export const SOURCE = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|css|scss|json|ya?ml|toml|sql|rb|py|sh|md|go|mk)$|(^|\/)(Makefile|llms\.txt)$/;

// What can't truthfully change (§4.6), in its text or its path: history
// documents; Ausca's catalog history, content-addressed immutable purchase
// data, and its dated run-through records; and Cloud's copy of the CLI release
// it adopted, as that release shipped, each file's digest in its release.json.
// Signed data is found by its signature.
export const HISTORY = /(^|\/)(release\/notes|plans\/(archive|evidence)|\.scafld\/specs\/archive|\.ai|mockups|proof)\/|^catalog\/history\/|^docs\/run-through-findings-[0-9-]+\.md$|^apps\/web\/src\/content\/release\//;
