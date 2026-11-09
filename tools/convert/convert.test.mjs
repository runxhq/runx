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

test("an expectation loses the data envelope of an emit that is a plain output now, and keeps an artifact's", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("hygiene"),
    "run.yaml": `skill: hygiene
version: "0.1.0"
runners:
  review:
    steps:
      - id: finalize
        tool: { module: hygiene.mjs, export: finalize }
        outputs:
          result: { type: object }
          receipt: { type: object }
        artifacts:
          named_emits: { result: result, receipt: receipt }
          packets: { receipt: hygiene/receipt.v1 }
      - id: report
        tool: { module: hygiene.mjs, export: report }
        artifacts:
          wrap_as: report
harness:
  cases:
    - name: suppresses-hard-bounce
      expect:
        status: sealed
        step_outputs:
          finalize:
            subset:
              result: { data: { decision: { state: suppressed } } }
              receipt: { data: { recorded: true } }
          report:
            subset:
              report: { data: { lines: 1 } }
        output:
          subset:
            result: { data: { decision: { state: suppressed } } }
`,
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.equal(
    read(dir, "tests/suppresses-hard-bounce.yaml"),
    `name: suppresses-hard-bounce
expect:
  status: sealed
  step_outputs:
    finalize:
      subset:
        result: {decision: {state: suppressed}}
        receipt: {data: {recorded: true}}
    report:
      subset:
        report: {data: {lines: 1}}
  output:
    subset:
      result: {decision: {state: suppressed}}
`,
  );
  assert.match(out, /harness\.cases\.suppresses-hard-bounce\.expect\.step_outputs\.finalize\.subset\.result\.data: the envelope's level goes/);
});

test("a step's act reads the invoking act as $run.act", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("demo"),
    "run.yaml": `skill: demo
version: "0.1.0"
runners:
  run:
    steps:
      - id: child
        skill: ../other
        runner: read
        act:
          purpose: Read what the invoking act names.
          reason: $act.reason
          target: $act
          form: $$act
`,
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.match(read(dir, "run.yaml"), /        act:\n          purpose: Read what the invoking act names\.\n          reason: \$run\.act\.reason\n          target: \$run\.act\n          form: \$\$act\n/);
});

test("a tool manifest a package or connector owns declares its process and its interface as a method does", (t) => {
  const manifest = `{
  "schema": "runx.tool.manifest.v1",
  "name": "demo.echo",
  "source": {"command": "node", "args": ["./echo.mjs"], "input_mode": "stdin", "type": "cli-tool"},
  "inputs": {
    "text": {"type": "string", "required": true},
    "note": {"type": "json"},
    "count": {"schema": {"type": "integer"}}
  },
  "artifacts": {"wrap_as": "echo", "packet": "demo.echo.v1"},
  "scopes": ["demo.echo"]
}
`;
  const converted = `{
  "schema": "runx.tool.manifest.v1",
  "name": "demo.echo",
  "process": {
    "command": "node",
    "args": ["./echo.mjs"],
    "inputs_via": "stdin"
  },
  "inputs": {
    "text": {
      "type": "string"
    },
    "note": {
      "optional": true
    },
    "count": {
      "type": "integer",
      "optional": true
    }
  },
  "outputs": {
    "echo": {
      "packet": "demo.echo.v1"
    }
  },
  "result_as": "echo"
}
`;
  const dir = tree(t, {
    "SKILL.md": manual("demo"),
    "run.yaml": "skill: demo\nversion: \"0.1.0\"\nrunners:\n  run:\n    steps:\n      - id: echo\n        tool: demo.echo\n",
    "tools/demo/echo/manifest.json": manifest,
  });
  const connector = tree(t, {
    "connector.yaml": "schema: runx.connector.v1\nname: demo\ntitle: Demo\ndescription: A test connector.\noperations:\n  thing.read:\n    description: Read a thing.\n    verb: read\n",
    "tools/demo/echo/manifest.json": manifest,
  });
  const { status, out } = convert("--write", dir, connector);
  assert.equal(status, 0, out);
  assert.equal(read(dir, "tools/demo/echo/manifest.json"), converted);
  assert.equal(read(connector, "tools/demo/echo/manifest.json"), converted);
  assert.match(out, /tools\/demo\/echo\/manifest\.json: scopes goes/);
  assert.match(out, new RegExp(`converted ${connector}: connector, already in the 0\\.12 shape, 1 files changed`));
  // A manifest that says process is in the 0.12 shape, and converts to itself.
  const again = convert("--write", connector);
  assert.equal(again.status, 0, again.out);
  assert.match(again.out, new RegExp(`converted ${connector}: connector, already in the 0\\.12 shape, 0 files changed`));
  assert.equal(read(connector, "tools/demo/echo/manifest.json"), converted);
  // A workspace's own tool, outside any package, converts by its path.
  const workspace = tree(t, { "tools/demo/echo/manifest.json": manifest });
  const standalone = convert("--write", path.join(workspace, "tools/demo/echo/manifest.json"));
  assert.equal(standalone.status, 0, standalone.out);
  assert.equal(read(workspace, "tools/demo/echo/manifest.json"), converted);
});

test("a fixture package declares the inputs its tests pass and its steps read, each optional", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("demo"),
    "run.yaml": "skill: demo\nversion: \"0.1.0\"\nrunners:\n  run:\n    steps:\n      - id: digest\n        tool: data.digest\n        inputs: { value: fixed }\n",
    "fixtures/support/session.graph": "name: demo-session\nsteps:\n  - id: draft\n    skill: ../..\n    runner: run\n    inputs: { account: $input.account_id }\n",
    "fixtures/drafts.yaml": "name: drafts\nkind: graph\ntarget: support/session.graph\ninputs:\n  action: { kind: comment }\nexpect:\n  status: sealed\n",
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.match(read(dir, "fixtures/session/run.yaml"), /^methods:\n  run:\n    inputs:\n      action: \{optional: true\}\n      account_id: \{optional: true\}\n    steps:\n/m);
});

test("a step calling one of its package's methods holds that method's emits, so its expectation loses their envelope too", (t) => {
  const dir = tree(t, {
    "SKILL.md": manual("review"),
    "run.yaml": [
      "skill: review",
      "version: \"0.1.0\"",
      "runners:",
      "  review:",
      "    default: true",
      "    steps:",
      "      - id: assess",
      "        skill: .",
      "        runner: assess",
      "  assess:",
      "    steps:",
      "      - id: finalize",
      "        tool: { module: review.mjs, export: finalize }",
      "        artifacts:",
      "          named_emits: { pr_review: pr_review }",
      "harness:",
      "  cases:",
      "    - name: reviews-exact-head",
      "      expect:",
      "        step_outputs:",
      "          assess:",
      "            subset:",
      "              pr_review: { data: { decision: draft_ready } }",
      "",
    ].join("\n"),
  });
  const { status, out } = convert("--write", dir);
  assert.equal(status, 0, out);
  assert.match(read(dir, "tests/reviews-exact-head.yaml"), /    assess:\n      subset:\n        pr_review: \{decision: draft_ready\}\n/);
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

test("a connector's delete stays a verb of its own, and the other kinds of write fold into write", (t) => {
  const connector = tree(t, {
    "connector.yaml": "schema: runx.connector.v1\nid: demo\nname: Demo\ndescription: A test connector.\noperations:\n  thing.delete:\n    description: Delete a thing.\n    verb: delete\n  thing.comment:\n    description: Comment on a thing.\n    verb: comment\n",
  });
  const { status, out } = convert("--write", connector);
  assert.equal(status, 0, out);
  assert.match(read(connector, "connector.yaml"), /\n  thing\.delete:\n    description: Delete a thing\.\n    verb: delete\n  thing\.comment:\n    description: Comment on a thing\.\n    verb: write\n/);
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
