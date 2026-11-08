import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

// The rename reads the table beside it, so these trees use names it holds for oss.
const RENAME = new URL("./rename.mjs", import.meta.url).pathname;

function run(script, ...args) {
  const ran = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  return { status: ran.status, out: ran.stdout + ran.stderr };
}

/** A git work tree holding the given files, removed when the test ends. */
function tree(t, files) {
  const root = mkdtempSync(path.join(tmpdir(), "runx-rename-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet", root]);
  for (const [file, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    writeFileSync(path.join(root, file), text);
  }
  return root;
}

const read = (root, file) => readFileSync(path.join(root, file), "utf8");

test("scan reads a name's run sense from whole segments, so a test's harness-references is the test sense", (t) => {
  // scan writes the table beside the program, so it runs on a copy of both.
  const program = tree(t, {});
  copyFileSync(RENAME, path.join(program, "rename.mjs"));
  copyFileSync(new URL("./identifiers.tsv", import.meta.url), path.join(program, "identifiers.tsv"));
  const root = tree(t, { "src/case.mjs": 'export const id = "checkout-harness-references";\nexport const ref = "checkout-harness-ref";\n' });
  const { status, out } = run(path.join(program, "rename.mjs"), "scan", "oss", root);
  assert.equal(status, 0, out);
  const rows = read(program, "identifiers.tsv").split("\n").map((line) => line.split("\t"));
  assert.deepEqual(rows.find((row) => row[1] === "checkout-harness-references")?.slice(4, 6), ["checkout-test-references", "the test sense"]);
  assert.deepEqual(rows.find((row) => row[1] === "checkout-harness-ref")?.slice(4, 6), ["checkout-run-ref", "the run sense"]);
});
