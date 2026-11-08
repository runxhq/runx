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

test("a name every object inherits, such as constructor, is no bare word", (t) => {
  const code = "export class Store {\n  constructor(runner) {\n    this.runner = runner;\n  }\n}\n// Constructor note.\n";
  const root = tree(t, { "packages/client/src/store.ts": code });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "packages/client/src/store.ts"), code.replaceAll("runner", "method"));
});

test("prose may already hold the word a name becomes", (t) => {
  const root = tree(t, { "docs/plans.md": "A harness-only package is test-only by design.\n" });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "docs/plans.md"), "A test-only package is test-only by design.\n");
});

test("Go files and Makefiles are sources, and a generated Go file is its generator's", (t) => {
  const generated = "type Purchase struct {\n\tRunner string `json:\"runner\"`\n}\n";
  const root = tree(t, {
    "internal/runx/execution.go": "type Execution struct {\n\tRunner string `json:\"runner\"`\n}\n",
    "internal/cloud/models.gen.go": generated,
    "Makefile": "check:\n\trunx harness . --json\n",
    "build/release.mk": "test-skills:\n\trunx harness skills --json\n",
  });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "internal/runx/execution.go"), "type Execution struct {\n\tMethod string `json:\"method\"`\n}\n");
  assert.equal(read(root, "internal/cloud/models.gen.go"), generated);
  assert.equal(read(root, "Makefile"), "check:\n\trunx test . --json\n");
  assert.equal(read(root, "build/release.mk"), "test-skills:\n\trunx test skills --json\n");
});

test("Ausca's catalog history and run-through record are history, and its generated contracts their generator's", (t) => {
  const kept = { runner: "check", harness_output: "stored" };
  const files = {
    "catalog/history/sha256-abc.json": `${JSON.stringify(kept)}\n`,
    "docs/run-through-findings-2026-09.md": "The runner failed its harness_output check.\n",
    "docs/run-through-findings-2026-10-02.md": "The runner failed again.\n",
    "contracts/generated/types.ts": "export type Purchase = { runner: string };\n",
  };
  const root = tree(t, { ...files, "src/purchase.ts": "export type Purchase = { runner: string };\n" });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  for (const [file, text] of Object.entries(files)) assert.equal(read(root, file), text, file);
  assert.equal(read(root, "src/purchase.ts"), "export type Purchase = { method: string };\n");
});

// Ausca's pinned files, each holding the literal its catalog keeps.
const auscaPins = {
  "internal/adapters/catalog/release.go": "type Revision struct {\n\tRunner string `json:\"runner\"`\n}\n",
  "internal/catalogcompiler/source.go": "type Source struct {\n\tRunner string `yaml:\"runner\"`\n}\n",
  "catalog/source/offers.yaml": "- offer: transcription\n  runner: ocr\n",
  "internal/catalogcompiler/compiler_test.go": "const source = \"runner: ocr\"\n",
};

test("Ausca's lease names take the lease's word", (t) => {
  const root = tree(t, { ...auscaPins, "internal/app/resources.go": "type ClaimedHold struct{}\n\nvar heldCapability = ClaimedHold{}\n" });
  const { status, out } = run(RENAME, "apply", "ausca", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "internal/app/resources.go"), "type ClaimedLease struct{}\n\nvar leasedCapability = ClaimedLease{}\n");
});

test("Ausca's own runners stay: a shell variable, a GPU job runner, its sandbox's and its buyer canary's", (t) => {
  const kept = {
    "deploy/worker-common.sh": "local runner=\"$4\"\n\"${runner}\" rollback\n",
    "docs/gpu.md": "Each GPU job runner pulls one job.\n",
    "docs/plans/code-execution.md": "The runner executes the snippet in its sandbox.\n",
    "scripts/buyer-canary.mjs": "const runner = drive;\n",
  };
  const root = tree(t, { ...auscaPins, ...kept, "docs/methods.md": "The default runner reads the catalog.\n" });
  const { status, out } = run(RENAME, "apply", "ausca", root, "--write");
  assert.equal(status, 0, out);
  for (const [file, text] of Object.entries(kept)) assert.equal(read(root, file), text, file);
  assert.equal(read(root, "docs/methods.md"), "The default method reads the catalog.\n");
});

// Cloud's pinned cipher label, which every Cloud tree holds.
const cloudPin = { "packages/api/src/payments/holdings-postgres.ts": 'export const label = "purchase-holdings";\n' };

test("Cloud's generated API schema modules are their generator's", (t) => {
  const generated = {
    "packages/api/src/generated/public-protocol-schemas.ts": 'export const publicProtocolSchemas = { Summary: { description: "One harness run, by its runner." } };\n',
    "packages/api/src/generated/purchase-schemas.ts": 'export const purchaseSchemas = { Offer: { description: "The runner a buyer pays for." } };\n',
  };
  const root = tree(t, { ...cloudPin, ...generated, "packages/api/src/generated/index.ts": "// Each harness schema.\n" });
  const { status, out } = run(RENAME, "apply", "cloud", root, "--write");
  assert.equal(status, 0, out);
  for (const [file, text] of Object.entries(generated)) assert.equal(read(root, file), text, file);
  assert.equal(read(root, "packages/api/src/generated/index.ts"), "// Each test schema.\n");
});

test("Ausca's catalog keys are pinned, so retained history decodes, while the code around them renames", (t) => {
  const root = tree(t, auscaPins);
  const { status, out } = run(RENAME, "apply", "ausca", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "internal/adapters/catalog/release.go"), "type Revision struct {\n\tMethod string `json:\"runner\"`\n}\n");
  assert.equal(read(root, "internal/catalogcompiler/source.go"), "type Source struct {\n\tMethod string `yaml:\"runner\"`\n}\n");
  assert.equal(read(root, "catalog/source/offers.yaml"), auscaPins["catalog/source/offers.yaml"]);
  assert.equal(read(root, "internal/catalogcompiler/compiler_test.go"), auscaPins["internal/catalogcompiler/compiler_test.go"]);
});
