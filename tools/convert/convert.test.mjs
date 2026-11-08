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

test("a name any method defines its own way isn't shared, and every method keeps exactly its source's meaning", (t) => {
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
  dispute_id:
    type: string
    required: true
    description: The dispute.
  repository:
    type: string
    required: true
    description: A repository as owner/name.
runners:
  run:
    default: true
    inputs:
      account_id: { definition: account_id }
      dispute_id: { definition: dispute_id }
      repo: { definition: repository }
${digestStep}  capture:
    inputs:
      account_id:
        type: string
        required: true
        description: The account.
        schema: { pattern: "^acct_[a-z]+$" }
      dispute_id:
        description: The dispute.
        required: true
        type: string
${digestStep}`,
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  const run = read(dir, "run.yaml");
  assert.match(run, /^shared_inputs:\n  dispute_id:\n    type: string\n    description: The dispute\.\nmethods:\n/m);
  assert.match(run, /  run:\n    inputs:\n      account_id:\n        type: string\n        default: ""\n        description: The account, or empty to resolve it\.\n        pattern: "\^\(\?:acct_\[a-z\]\+\)\?\$"\n        optional: true\n      dispute_id: \{\}\n      repo:\n        type: string\n        description: A repository as owner\/name\.\n/);
  assert.match(run, /  capture:\n    inputs:\n      account_id:\n        type: string\n        description: The account\.\n        pattern: "\^acct_\[a-z\]\+\$"\n      dispute_id: \{\}\n/);
  assert.match(out, /shared input account_id isn't shared: capture define it their own way, so run say it inline and it leaves shared_inputs/);
  assert.match(out, /methods\.capture\.inputs\.dispute_id restates shared input dispute_id, so it takes it/);
  assert.match(out, /shared input repository is taken by no method, so it leaves shared_inputs/);
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

test("text before a plural heading's first task stays under ## Agent tasks, where the heading was", (t) => {
  const dir = tree(t, {
    "SKILL.md": `---
name: planner
description: A test package.
---

# Planner

## Stop conditions

Stop when the plan is empty.

## Agent task contracts

Every planning act cites its source and never invents a date.

### draft

Draft the plan.

### review

Review the plan.
`,
    "run.yaml": `skill: planner
version: "0.1.0"
runners:
  draft:
    default: true
  review: {}
`,
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.equal(
    read(dir, "SKILL.md"),
    `---
name: planner
description: A test package.
---

# Planner

## Stop conditions

Stop when the plan is empty.

## Agent tasks

Every planning act cites its source and never invents a date.

## Agent task: draft

Draft the plan.

## Agent task: review

Review the plan.
`,
  );
  assert.match(out, /SKILL\.md's "## Agent task contracts" has text before its first task, kept under "## Agent tasks", which names no task/);
});

test("a root connector nothing calls is refused, and one a turn's list or a called package reaches stays where it's called", (t) => {
  const turn = (operations) => `skill: sync
version: "0.1.0"
connectors:
  web: {operations: [page.read]}
  acme-ops: {operations: [item.sync]}
runners:
  main:
    default: true
${operations}    steps:
      - id: plan
        agent: { role: agent, task: plan }
`;
  const plan = "---\nname: sync\ndescription: Syncs items.\n---\n\n## Agent task: plan\n\nPlan.\n";
  const unreached = tree(t, { "SKILL.md": plan, "run.yaml": turn("") });
  const refused = convert(unreached);
  assert.equal(refused.status, 1);
  assert.match(refused.out, /refused .*: connectors web, acme-ops are called by no step and named in no turn's calls\. Today a connector's operations are only a ceiling/);

  const reached = tree(t, {
    "sync/SKILL.md": plan,
    "sync/run.yaml": turn("    operations: [web/page.read]\n").replace("      - id: plan\n", "      - id: items\n        skill: ../items\n      - id: plan\n"),
    "items/SKILL.md": manual("items"),
    "items/run.yaml": "skill: items\nconnectors:\n  acme-ops: {operations: [item.sync]}\nrunners:\n  sync:\n    steps:\n      - id: sync\n        operation: acme-ops/item.sync\n",
  });
  const converted = convert("--write", path.join(reached, "sync"));
  assert.equal(converted.status, 0, converted.out);
  const run = read(reached, "sync/run.yaml");
  assert.doesNotMatch(run, /^connectors:/m);
  assert.match(run, /      - id: plan\n        agent: plan\n        calls:\n          operations: \[web\/page\.read\]\n/);
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
