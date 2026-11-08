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

const invoiceCase = (env, path) => `name: reads-invoice
runner: pay
env:
${env}
inputs:
  path: ${path}
expect:
  status: sealed
`;

const payPackage = (cases) => ({
  "SKILL.md": manual("payer"),
  "fixtures/support/invoice.txt": "INV-1 USD 42.00\n",
  "run.yaml": `skill: payer
version: "0.1.0"
runners:
  pay:
    inputs:
      path: { type: string, required: true }
${digestStep}`,
  ...cases,
});

test("a test that ran in its package's directory and read only fixtures/ loses RUNX_CWD, which the runtime owns", (t) => {
  const dir = tree(t, payPackage({ "fixtures/reads-invoice.yaml": invoiceCase("  RUNX_CWD: .\n  PAYEE: acme", "fixtures/support/invoice.txt") }));
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.equal(read(dir, "tests/reads-invoice.yaml"), "name: reads-invoice\nmethod: pay\ninputs:\n  path: fixtures/support/invoice.txt\nenv:\n  PAYEE: acme\nexpect:\n  status: sealed\n");
  assert.match(out, /RUNX_CWD removed: the test's workspace holds the fixtures\/ it read from its package's directory/);
  assert.equal(read(dir, "fixtures/support/invoice.txt"), "INV-1 USD 42.00\n");
});

test("a test whose RUNX_CWD reached more than fixtures/ is refused", (t) => {
  const outside = tree(t, payPackage({ "fixtures/reads-invoice.yaml": invoiceCase("  RUNX_CWD: .", "SKILL.md") }));
  const refusedOutside = convert(outside);
  assert.equal(refusedOutside.status, 1);
  assert.match(refusedOutside.out, /its inputs read SKILL\.md, which a test's workspace doesn't hold: only fixtures\/ is staged/);

  const elsewhere = tree(t, payPackage({ "fixtures/reads-invoice.yaml": invoiceCase("  RUNX_CWD: ../..", "fixtures/support/invoice.txt") }));
  const refusedElsewhere = convert(elsewhere);
  assert.equal(refusedElsewhere.status, 1);
  assert.match(refusedElsewhere.out, /env\.RUNX_CWD is "\.\.\/\.\.": a test runs in the workspace the runtime makes/);
});

test("a converted package and connector convert again to themselves", (t) => {
  const dir = tree(t, payPackage({ "fixtures/reads-invoice.yaml": invoiceCase("  RUNX_CWD: .", "fixtures/support/invoice.txt") }));
  const connector = tree(t, {
    "connector.yaml": "schema: runx.connector.v1\nid: demo\nname: Demo\ndescription: A test connector.\noperations:\n  thing.read:\n    description: Read a thing.\n    verb: read\n",
  });
  assert.equal(convert("--write", dir, connector).status, 0);
  const converted = { package: snapshot(dir), connector: snapshot(connector) };
  assert.match(converted.connector["connector.yaml"], /^schema: runx\.connector\.v1\nname: demo\ntitle: Demo\n/);

  const again = convert("--write", dir, connector);
  assert.equal(again.status, 0, again.out);
  assert.match(again.out, new RegExp(`converted ${dir}: 1 tests, already in the 0\\.12 shape, 0 files changed`));
  assert.match(again.out, new RegExp(`converted ${connector}: connector, already in the 0\\.12 shape, 0 files changed`));
  assert.deepEqual({ package: snapshot(dir), connector: snapshot(connector) }, converted);
});

test("a converted package's test still loses RUNX_CWD, as converting it from the old shape does", (t) => {
  const dir = tree(t, payPackage({}));
  assert.equal(convert("--write", dir).status, 0);
  mkdirSync(path.join(dir, "tests"));
  writeFileSync(path.join(dir, "tests/reads-invoice.yaml"), "name: reads-invoice\nmethod: pay\ninputs:\n  path: fixtures/support/invoice.txt\nenv:\n  RUNX_CWD: .\nexpect:\n  status: sealed\n");
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.match(out, /already in the 0\.12 shape, 1 files changed/);
  assert.equal(read(dir, "tests/reads-invoice.yaml"), "name: reads-invoice\nmethod: pay\ninputs:\n  path: fixtures/support/invoice.txt\nexpect:\n  status: sealed\n");
});
