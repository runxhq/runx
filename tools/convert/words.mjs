// The two bare words 0.12 retires in one sense and keeps in another, and the
// short, exact list of where each keeps it (plan §4.2, §6.1, §6.5). Every
// other use is renamed: `runner` meaning a skill's method becomes `method`,
// and `harness` meaning a scripted test becomes `test`, or `run` in its run
// sense. rename.mjs applies it, and stale-words.mjs fails on what's left.

export const WORDS = {
  runner: { runner: "method", runners: "methods" },
  harness: { harness: "test", harnesses: "tests" },
};

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

// `harness` before context, ref, spine or control is the run sense (§4.2).
const RUN_SENSE = /^[ _-](context|ref|spine|control)/i;

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
  // runner: the machinery that executes runs (§4.2).
  { word: "runner", why: "the execution runner module", before: /(execution|harness|test)(::|\/)$/ },
  { word: "runner", why: "the execution runner module", file: /^crates\/runx-runtime\/src\/(execution\.rs|execution\/harness\.rs)$/, before: /\bmod $/ },
  { word: "runner", why: "the execution runner module", file: /^crates\/runx-runtime\/src\/(lib\.rs|execution\/run_record\.rs|execution\/harness\/)/, after: /^::/ },
  { word: "runner", why: "the test runner's module", file: /^crates\/runx-runtime\/src\/execution\/harness\.rs$/, before: /\buse $/, after: /^::/ },
  { word: "runner", why: "the execution runner module", file: /^crates\/runx-runtime\/src\/(execution|lib)\.rs$/, before: /`$/, after: /^`/ },
  { word: "runner", why: "the execution runner", before: /\b(graph|kernel['’]s) $/ },
  { word: "runner", why: "the operational policy's runners", file: /^(crates\/runx-contracts\/src\/operational_policy(\.rs$|\/)|crates\/runx-contracts\/tests\/operational_policy\.rs$|crates\/runx-cli\/(src|tests)\/policy\.rs$|packages\/contracts\/src\/schemas\/operational-policy\.|fixtures\/operational-policy\/|skills\/policy-draft\/|tests\/policy-draft-validation\.test\.ts$)/ },
  { word: "runner", why: "the kernel fixture runner", file: /^(fixtures\/kernel\/|scripts\/generate-kernel-parity-fixtures\.ts$|tests\/kernel-parity-fixtures\.test\.ts$)/ },
  { word: "runner", why: "GitHub's runner", file: /^\.github\// },
  { word: "runner", why: "GitHub's runner", before: /(GitHub(['’]s)?|[Hh]osted|self-hosted|Actions|CI) $/ },
  { word: "runner", why: "a test runner", before: /\btest $/i },
  // runner in Ausca: a shell variable, a GPU job runner, its sandbox's own
  // runner, and the buyer canary's driver beside GitHub's runner limit.
  { word: "runner", why: "a shell variable holding the command it runs", file: /^deploy\/(resource-release-common|worker-common)\.sh$/ },
  { word: "runner", why: "a GPU job runner", before: /\bGPU job $/ },
  { word: "runner", why: "the code-execution sandbox's own runner", file: /^docs\/plans\/code-execution\.md$/ },
  { word: "runner", why: "the buyer canary's own driver, and GitHub's runner limit", file: /^scripts\/buyer-canary(\.test)?\.mjs$/ },
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
 * exact token in one file, at every site in it or only where its `before`
 * and `after` match, as a stay's do. More than one is a conflict for the
 * caller to refuse.
 */
export function decisionsFor(rows, file, line, index, token) {
  const before = line.slice(0, index);
  const after = line.slice(index + token.length);
  return rows.filter((row) => row.file === file && row.word === token && (!row.before || row.before.test(before)) && (!row.after || row.after.test(after)));
}

// A file's own name keeps the word where its folder does: the engine's
// module, the test runner, and the kernel fixture runner's folder.
export const STAYING_PATHS = /^crates\/runx-runtime\/src\/execution\/(runner(\.rs|\/)|harness\/runner\.rs$)|^fixtures\/kernel\/runner\//;

// Files a generator writes, regenerated after a rename and never edited.
// Cloud's two API schema modules are scripts/generate-public-schemas.ts's.
export const GENERATED = /^(schemas\/[^/]+\.schema\.json|packages\/contracts\/src\/generated\/.+|packages\/contracts\/src\/schema-artifacts\.ts|crates\/runx-runtime\/bundled-skills\.json|packages\/api\/src\/generated\/(public-protocol-schemas|purchase-schemas)\.ts)$|\.gen\.go$|^contracts\/generated\//;

// The sources a rename reads, and the guard with it: code, data and prose
// by extension, Go among them, and Makefiles by name.
export const SOURCE = /\.(rs|ts|tsx|mts|mjs|js|cjs|astro|json|ya?ml|toml|sql|rb|py|sh|md|go|mk)$|(^|\/)Makefile$/;

// What can't truthfully change (§4.6), in its text or its path: history
// documents; Ausca's catalog history, content-addressed immutable purchase
// data, and its dated run-through records; and Cloud's copy of the CLI release
// it adopted, as that release shipped, each file's digest in its release.json.
// Signed data is found by its signature.
export const HISTORY = /(^|\/)(release\/notes|plans\/(archive|evidence)|\.scafld\/specs\/archive|\.ai|mockups|proof)\/|^catalog\/history\/|^docs\/run-through-findings-[0-9-]+\.md$|^apps\/web\/src\/content\/release\//;
