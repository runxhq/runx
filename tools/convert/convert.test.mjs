import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

const CONVERT = new URL("./convert.mjs", import.meta.url).pathname;

function convert(...args) {
  const run = spawnSync(process.execPath, [CONVERT, ...args], { encoding: "utf8" });
  return { status: run.status, out: run.stdout + run.stderr };
}

/** A package directory holding the given files, removed when the test ends. */
function tree(t, files) {
  const dir = mkdtempSync(path.join(tmpdir(), "runx-convert-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  for (const [file, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(dir, file)), { recursive: true });
    writeFileSync(path.join(dir, file), text);
  }
  return dir;
}

const manual = (name) => `---\nname: ${name}\ndescription: A test package.\n---\n\n# Demo\n`;
const read = (dir, file) => readFileSync(path.join(dir, file), "utf8");
/** Every file under a directory, by its relative path, with its text. */
const snapshot = (dir) =>
  Object.fromEntries(
    readdirSync(dir, { recursive: true, withFileTypes: true })
      .filter((entry) => entry.isFile())
      .map((entry) => path.relative(dir, path.join(entry.parentPath, entry.name)))
      .map((file) => [file, read(dir, file)]),
  );

const digestStep = "    steps:\n      - id: digest\n        tool: data.digest\n        inputs: { value: fixed }\n";

test("an own input that a shared input of its name would change is refused, naming both", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("demo"),
    "run.yaml": `skill: demo
version: "0.1.0"
input_definitions:
  account_id:
    type: string
    required: false
    default: ""
    description: The account, or empty to resolve it.
    schema: { pattern: "^(?:acct_[a-z]+)?$" }
runners:
  run:
    default: true
    inputs:
      account_id: { definition: account_id }
${digestStep}  capture:
    inputs:
      account_id:
        type: string
        required: true
        description: The account, or empty to resolve it.
        schema: { pattern: "^acct_[a-z]+$" }
${digestStep}`,
  });
  const before = snapshot(dir);
  const { status, out } = convert("--write", dir);
  assert.equal(status, 1);
  assert.match(out, /refused .*methods\.capture\.inputs\.account_id is its own input, and shared input account_id would give it default, optional: two inputs share one name/);
  assert.match(out, /Shared: \{.*"default":"".*\}\. This method's: \{.*"pattern":"\^acct_\[a-z\]\+\$".*\}/);
  assert.deepEqual(snapshot(dir), before, "a refused package is left as it was");
});

test("an own input restating a shared input takes it, and keeps only the keys it overrides", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("demo"),
    "run.yaml": `skill: demo
version: "0.1.0"
input_definitions:
  dispute_id:
    type: string
    required: true
    description: The dispute.
  repository:
    type: string
    required: true
    description: A repository as owner/name.
    schema: { pattern: "^[a-z]+/[a-z]+$", examples: [runxhq/runx] }
runners:
  run:
    default: true
    inputs:
      dispute_id: { definition: dispute_id }
      repository: { definition: repository }
${digestStep}  capture:
    inputs:
      dispute_id:
        type: string
        required: true
        description: The dispute.
      repository:
        type: string
        required: true
        description: The repository to comment on.
        schema: { pattern: "^[a-z]+/[a-z]+$", maxLength: 201 }
${digestStep}`,
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  const run = read(dir, "run.yaml");
  assert.match(run, /  capture:\n    inputs:\n      dispute_id: \{\}\n      repository:\n        description: The repository to comment on\.\n        maxLength: 201\n/);
  assert.match(out, /methods\.capture\.inputs\.repository takes shared input repository's examples, which change nothing it accepts/);
});

test("a step's act loses decision, which the 0.12 format removes, and keeps its other fields", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("demo"),
    "run.yaml": `skill: demo
version: "0.1.0"
runners:
  run:
    steps:
      - id: first
        tool: data.digest
        inputs: { value: fixed }
      - id: child
        skill: ../other
        runner: read
        act:
          purpose: Read the record the first step named.
          reason: The digest names it.
          decision: proceed
          previous: $steps.first.digest_result.digest
`,
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.match(read(dir, "run.yaml"), /        skill: \.\.\/other#read\n        act:\n          purpose: Read the record the first step named\.\n          reason: The digest names it\.\n          previous: \$steps\.first\.digest_result\.digest\n/);
  assert.match(out, /methods\.run\.steps\.child\.act\.decision removed: nothing in runx decides with it/);
});
