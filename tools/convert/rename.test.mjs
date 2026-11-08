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
  copyFileSync(new URL("./words.mjs", import.meta.url), path.join(program, "words.mjs"));
  copyFileSync(new URL("./identifiers.tsv", import.meta.url), path.join(program, "identifiers.tsv"));
  const root = tree(t, { "src/case.mjs": 'export const id = "checkout-harness-references";\nexport const ref = "checkout-harness-ref";\n' });
  const { status, out } = run(path.join(program, "rename.mjs"), "scan", "oss", root);
  assert.equal(status, 0, out);
  const rows = read(program, "identifiers.tsv").split("\n").map((line) => line.split("\t"));
  assert.deepEqual(rows.find((row) => row[1] === "checkout-harness-references")?.slice(4, 6), ["checkout-test-references", "the test sense"]);
  assert.deepEqual(rows.find((row) => row[1] === "checkout-harness-ref")?.slice(4, 6), ["checkout-run-ref", "the run sense"]);
});

const agentManual = `---
name: agent-session
description: One turn of a host's agent.
---

# Agent session

The agent-session skill is one turn.

## Agent task: read

Read and answer.
`;

test("a package's name renames in its SKILL.md, prose included, and in its tests and its evals", (t) => {
  const root = tree(t, {
    "skills/agent-session/SKILL.md": agentManual,
    "skills/agent-session/run.yaml": "minimum_runx: 0.12.0\nmethods:\n  read:\n    steps:\n      - id: read\n        agent: read\n",
    "skills/agent-session/tests/agent-session-read-answers-with-a-summary.yaml": "name: agent-session-read-answers-with-a-summary\nmethod: read\nexpect:\n  status: sealed\n",
    "skills/agent-session/evals/evals.json": '{ "skill_name": "agent-session", "evals": [] }\n',
  });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "skills/agent/SKILL.md"), agentManual.replaceAll("agent-session", "agent"));
  assert.equal(read(root, "skills/agent/tests/agent-read-answers-with-a-summary.yaml"), "name: agent-read-answers-with-a-summary\nmethod: read\nexpect:\n  status: sealed\n");
  assert.equal(read(root, "skills/agent/evals/evals.json"), '{ "skill_name": "agent", "evals": [] }\n');
});

test("a name a run.yaml shares with its module renames in both", (t) => {
  const root = tree(t, {
    "skills/diagnose/SKILL.md": "---\nname: diagnose\ndescription: Diagnose a run.\n---\n",
    "skills/diagnose/run.yaml": "minimum_runx: 0.12.0\nmethods:\n  review:\n    inputs:\n      harness_output: { type: string }\n    steps:\n      - id: finish\n        tool: { module: diagnose.mjs, export: finish }\n        inputs: { harness_output: $input.harness_output }\n",
    "skills/diagnose/diagnose.mjs": "export const finish = (input) => input.harness_output;\n",
  });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.match(read(root, "skills/diagnose/run.yaml"), /      test_output: \{ type: string \}\n[\s\S]*inputs: \{ test_output: \$input\.test_output \}/);
  assert.equal(read(root, "skills/diagnose/diagnose.mjs"), "export const finish = (input) => input.test_output;\n");
});

test("the rename refuses a package file the converter hasn't converted", (t) => {
  const root = tree(t, {
    "skills/diagnose/SKILL.md": "---\nname: diagnose\ndescription: Diagnose a run.\n---\n",
    "skills/diagnose/run.yaml": "skill: diagnose\nrunners:\n  review:\n    steps: []\n",
    "connectors/demo/connector.yaml": "schema: runx.connector.v1\nid: demo\nname: Demo\n",
  });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 1);
  assert.match(out, /refused oss: 2 package files aren't in the 0\.12 shape; the rename reads what the converter wrote, so convert them first/);
  assert.match(out, /skills\/diagnose\/run\.yaml   \(a run\.yaml with no minimum_runx\)/);
  assert.match(out, /connectors\/demo\/connector\.yaml   \(a connector\.yaml that says id\)/);
});

test("the rename runs again over its own output and changes nothing, a chain of names included", (t) => {
  const root = tree(t, {
    "src/turn.ts": "export type AgentActInvocation = { act: TurnInvocation };\nexport type TurnInvocation = { id: string };\n",
  });
  const first = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(first.status, 0, first.out);
  const renamed = "export type TurnInvocation = { act: TurnView };\nexport type TurnView = { id: string };\n";
  assert.equal(read(root, "src/turn.ts"), renamed);

  const again = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(again.status, 0, again.out);
  assert.match(again.out, /oss: 0 names in 0 files, 0 of them bare words, 0 bare words kept, 0 paths moved; 0 signed files left as signed; .*TurnInvocation.* already the new names/);
  assert.equal(read(root, "src/turn.ts"), renamed);
});

test("the rename leaves a generated file to its generator", (t) => {
  const generated = '{ "properties": { "harness_output": { "type": "string" } } }\n';
  const root = tree(t, {
    "schemas/diagnosis.schema.json": generated,
    "crates/runx-contracts/src/diagnosis.rs": "pub struct Diagnosis { pub harness_output: String }\n",
  });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "crates/runx-contracts/src/diagnosis.rs"), "pub struct Diagnosis { pub test_output: String }\n");
  assert.equal(read(root, "schemas/diagnosis.schema.json"), generated);
});

test("a bare runner or harness renames everywhere but where a stay keeps it, and the dry run lists each stay", (t) => {
  const root = tree(t, {
    "crates/runx-runtime/src/execution.rs": "//! - `runner`: the graph engine.\npub mod runner;\npub mod harness;\n",
    "crates/runx-runtime/src/execution/harness/runner.rs": "use crate::execution::runner::turn;\npub fn inputs(skill: &Skill) -> &Inputs { &skill.runner.inputs }\n",
    "crates/runx-runtime/src/skill_package/inspection/runner.rs": "/// The runners a skill declares.\npub fn runners() {}\n",
    "crates/runx-contracts/src/operational_policy.rs": "pub struct Policy { pub runners: Vec<Runner> }\n",
    "docs/agents.md": "Run the harness first. The harness context travels with the run. An agent harness such as Codex keeps its own memory.\n",
  });
  const dry = run(RENAME, "apply", "oss", root, "--stays");
  assert.equal(dry.status, 0, dry.out);
  assert.match(dry.out, /^stays\tcrates\/runx-runtime\/src\/execution\.rs:1\tthe execution runner module\t/m);
  assert.match(dry.out, /^stays\tcrates\/runx-runtime\/src\/execution\.rs:2\tthe execution runner module\tpub mod runner;$/m);
  assert.match(dry.out, /^stays\tcrates\/runx-runtime\/src\/execution\/harness\/runner\.rs:1\tthe execution runner module\t/m);
  assert.match(dry.out, /^stays\tcrates\/runx-contracts\/src\/operational_policy\.rs:1\tthe operational policy's runners\t/m);
  assert.match(dry.out, /^stays\tdocs\/agents\.md:1\tan agent's host\t/m);

  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "crates/runx-runtime/src/execution.rs"), "//! - `runner`: the graph engine.\npub mod runner;\npub mod test;\n");
  assert.equal(read(root, "crates/runx-runtime/src/execution/test/runner.rs"), "use crate::execution::runner::turn;\npub fn inputs(skill: &Skill) -> &Inputs { &skill.method.inputs }\n");
  assert.equal(read(root, "crates/runx-runtime/src/skill_package/inspection/method.rs"), "/// The methods a skill declares.\npub fn methods() {}\n");
  assert.equal(read(root, "crates/runx-contracts/src/operational_policy.rs"), "pub struct Policy { pub runners: Vec<Runner> }\n");
  assert.equal(read(root, "docs/agents.md"), "Run the test first. The run context travels with the run. An agent harness such as Codex keeps its own memory.\n");
});
