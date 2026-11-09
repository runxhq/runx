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

// A word renamed in prose alone: Markdown outside its code. In code, a graph
// is an identifier the table decides, a path stays where it is, and an inline
// or fenced code span quotes code, a command or a file as it is.
export const PROSE_ONLY = new Set(["graph"]);

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
// holds in (all when it says none), and what must sit just before or just
// after the word on its line.
export const STAYS = [
  // runner: the machinery that executes runs (§4.2). The test runner's module
  // is named at its path before and after the rename moves it, so the rename
  // runs again over its own output.
  { word: "runner", why: "the execution runner module", before: /(execution|harness|test)(::|\/)$/ },
  { word: "runner", why: "the execution runner module", file: /^crates\/runx-runtime\/src\/(execution\.rs|execution\/(harness|test)\.rs)$/, before: /\bmod $/ },
  { word: "runner", why: "the execution runner module", file: /^crates\/runx-runtime\/src\/(lib\.rs|execution\/run_record\.rs|execution\/(harness|test)\/)/, after: /^::/ },
  { word: "runner", why: "the test runner's module", file: /^crates\/runx-runtime\/src\/execution\/(harness|test)\.rs$/, before: /\buse $/, after: /^::/ },
  { word: "runner", why: "the execution runner module", file: /^crates\/runx-runtime\/src\/(execution|lib)\.rs$/, before: /`$/, after: /^`/ },
  { word: "runner", why: "the execution runner", before: /\b(graph|kernel['’]s) $/ },
  { word: "runner", why: "the operational policy's runners", file: /^(crates\/runx-contracts\/src\/operational_policy(\.rs$|\/)|crates\/runx-contracts\/tests\/operational_policy\.rs$|crates\/runx-cli\/(src|tests)\/policy\.rs$|packages\/contracts\/src\/schemas\/operational-policy\.|fixtures\/operational-policy\/|skills\/policy-draft\/|tests\/policy-draft-validation\.test\.ts$)/ },
  { word: "runner", why: "the operational policy's runners, in a corpus or test that holds one policy among others", file: /^(crates\/runx-contracts\/tests\/schema_wire_conformance\/corpora\.rs|packages\/contracts\/src\/index\.test\.ts)$/, before: /^\s*"?$/, after: /^"?: \[\{/ },
  { word: "runner", why: "the kernel fixture runner", file: /^(fixtures\/kernel\/|scripts\/generate-kernel-parity-fixtures\.ts$|tests\/kernel-parity-fixtures\.test\.ts$)/ },
  { word: "runner", why: "GitHub's runner", file: /^\.github\// },
  { word: "runner", why: "GitHub's runner", before: /(GitHub(['’]s)?|[Hh]osted|self-hosted|Actions|CI) $/ },
  { word: "runner", why: "a test runner", before: /\btest $/i },
  { word: "runner", why: "the retired flag a test proves is refused", file: /^crates\/runx-cli\/tests\/router\.rs$/, before: /--$/ },
  // runner in Ausca: a shell variable, a GPU job runner, its sandbox's own
  // runner, and the buyer canary's driver beside GitHub's runner limit.
  { word: "runner", why: "a shell variable holding the command it runs", file: /^deploy\/(resource-release-common|worker-common)\.sh$/ },
  { word: "runner", why: "a GPU job runner", before: /\bGPU job $/ },
  { word: "runner", why: "the code-execution sandbox's own runner", file: /^docs\/plans\/code-execution\.md$/ },
  { word: "runner", why: "the buyer canary's own driver, and GitHub's runner limit", file: /^scripts\/buyer-canary(\.test)?\.mjs$/ },
  // graph: another graph than a method's steps, which keeps the word.
  { word: "graph", why: "a dependency graph", before: /\b(crate|package|dependency|module|import)s? $/i },
  { word: "graph", why: "a call graph", before: /\bcall $/i },
  { word: "graph", why: "a knowledge graph", before: /\b(knowledge|claim|entity|semantic) $/i },
  { word: "graph", why: "a graph of receipts", before: /\breceipt $/i },
  { word: "graph", why: "a graph database", after: /^ (database|databases|db|store|query)\b/i },
  { word: "graph", why: "a chart", before: /\b(bar|line|pie|time-series|latency|throughput) $/i },
  // harness: a receipt's harness reference, whose words receipts keep (§4.6):
  // its type, the runx:harness: references a run's receipts carry, and the
  // fixtures and tests that hold them.
  { word: "harness", why: "a receipt's harness reference type, which receipts keep (§4.6)", before: /\bReferenceType::$/ },
  { word: "harness", why: "a receipt's harness reference type, which receipts keep (§4.6)", file: /^crates\/runx-contracts\/src\/reference\.rs$/ },
  { word: "harness", why: "a runx:harness: reference a receipt carries, which receipts keep (§4.6)", before: /\brunx:$/, after: /^:/ },
  { word: "harness", why: "a receipt's harness reference type, which receipts keep (§4.6)", file: /^(fixtures\/contracts\/credential-delivery\/observation\.json|packages\/contracts\/src\/schemas\/credential-delivery\.test\.ts)$/, before: /\btype"?: ?"$/ },
  // harness: the retired command a test names to prove native help dropped it.
  { word: "harness", why: "the retired command native help must not advertise", file: /^crates\/runx-cli\/tests\/router\.rs$/, before: /\b(runx|advertise) $/ },
  // harness: a fixture post's own words, which its approved payload digest binds.
  { word: "harness", why: "a fixture post's own words, which its approved payload digest binds", file: /^skills\/moltbook\/tests\/publish-creates-and-reads-back-post\.yaml$/ },
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
  return STAYS.find((stay) => stay.word === word && (!stay.file || stay.file.test(file)) && (!stay.before || stay.before.test(before)) && (!stay.after || stay.after.test(after)));
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

// A file's own name keeps the word where its folder does: the engine's
// module, the test runner, and the kernel fixture runner's folder.
export const STAYING_PATHS = /^crates\/runx-runtime\/src\/execution\/(runner(\.rs|\/)|(harness|test)\/runner\.rs$)|^fixtures\/kernel\/runner\//;

// Files a generator writes, regenerated after a rename and never edited.
// Cloud's two API schema modules are scripts/generate-public-schemas.ts's, and
// docs/api-surface.md is scripts/gen-api-index.ts's.
export const GENERATED = /^(schemas\/[^/]+\.schema\.json|docs\/api-surface\.md|packages\/contracts\/src\/generated\/.+|packages\/contracts\/src\/schema-artifacts\.ts|crates\/runx-runtime\/bundled-skills\.json|packages\/api\/src\/generated\/(public-protocol-schemas|purchase-schemas)\.ts)$|\.gen\.go$|^contracts\/generated\//;

// The sources a rename reads, and the guard with it: code, data and prose
// by extension, Go and stylesheets among them, and Makefiles by name. A
// stylesheet's class and id names are names like any other, so a class an
// element takes renames in the rules that style it.
export const SOURCE = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|css|scss|json|ya?ml|toml|sql|rb|py|sh|md|go|mk)$|(^|\/)Makefile$/;

// What can't truthfully change (§4.6), in its text or its path: history
// documents; Ausca's catalog history, content-addressed immutable purchase
// data, and its dated run-through records; and Cloud's copy of the CLI release
// it adopted, as that release shipped, each file's digest in its release.json.
// Signed data is found by its signature.
export const HISTORY = /(^|\/)(release\/notes|plans\/(archive|evidence)|\.scafld\/specs\/archive|\.ai|mockups|proof)\/|^catalog\/history\/|^docs\/run-through-findings-[0-9-]+\.md$|^apps\/web\/src\/content\/release\//;
