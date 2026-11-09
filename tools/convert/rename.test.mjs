import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { decisionsFor } from "./words.mjs";

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
  for (const file of ["words.mjs", "script-collisions.mjs", "rust-collisions.mjs", "identifiers.tsv"]) copyFileSync(new URL(`./${file}`, import.meta.url), path.join(program, file));
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

test("a bare word renamed onto a name a shell script already holds is refused at each site, and nothing is written", (t) => {
  const code = 'method="$1"\nrunner="$2"\necho "${runner} ${method}"\n';
  const prose = "Run the harness before the tests.\n";
  const root = tree(t, { "scripts/release.sh": code, "docs/testing.md": prose });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 1, out);
  assert.match(out, /^refused oss: 2 renamed bare words collide with a name their code already holds, and no row of bare-words\.tsv decides what they become$/m);
  for (const line of [2, 3]) assert.match(out, new RegExp(`^  scripts/release\\.sh:${line}\\trunner -> method\\tits file already holds method\\t`, "m"));
  assert.equal(read(root, "scripts/release.sh"), code);
  assert.equal(read(root, "docs/testing.md"), prose);
});

test("TypeScript's AST decides a landing: a name held as a member, a string or another scope's binding is no collision", (t) => {
  const code = '// The harness checks the test.\nexport const run = (harness: Fixture) => harness.test("the test");\nexport function other(test: number) {\n  return test;\n}\n';
  const root = tree(t, { "packages/client/src/run.ts": code });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "packages/client/src/run.ts"), code.replaceAll("harness", "test"));
});

test("TypeScript's AST refuses the store contract's duplicate binding, and the reference it would move", (t) => {
  const code = "export async function withFixture(test: (harness: Fixture) => Promise<void>): Promise<void> {\n  const harness = await create();\n  await test(harness);\n}\n";
  const root = tree(t, { "packages/registry/src/contract.ts": code });
  const { status, out } = run(RENAME, "apply", "oss", root, "--write");
  assert.equal(status, 1, out);
  assert.match(out, /^refused oss: 2 renamed bare words collide with a name their code already holds/m);
  assert.match(out, /^  packages\/registry\/src\/contract\.ts:2\tharness -> test\ta second binding in one scope \(TS2300: Duplicate identifier 'test'\.\)/m);
  assert.match(out, /^  packages\/registry\/src\/contract\.ts:3\tharness -> test\tnow resolves to line 1's binding, not line 2's binding\t/m);
  assert.doesNotMatch(out, /contract\.ts:1\t/);
  assert.equal(read(root, "packages/registry/src/contract.ts"), code);
});

// A Cargo workspace of one crate on the repo's own toolchain, so clippy is there.
const TOOLCHAIN = readFileSync(new URL("../../rust-toolchain.toml", import.meta.url), "utf8");
const crate = (lib) => ({
  "rust-toolchain.toml": TOOLCHAIN,
  "crates/Cargo.toml": '[workspace]\nmembers = ["demo"]\nresolver = "2"\n',
  "crates/demo/Cargo.toml": '[package]\nname = "demo"\nversion = "0.1.0"\nedition = "2021"\n',
  "crates/demo/src/lib.rs": lib,
});
function runInCache(t, ...args) {
  const cache = mkdtempSync(path.join(tmpdir(), "runx-rename-cargo-"));
  t.after(() => rmSync(cache, { recursive: true, force: true }));
  const ran = spawnSync(process.execPath, [RENAME, ...args], { encoding: "utf8", env: { ...process.env, RUNX_CONVERT_CACHE: cache } });
  return { status: ran.status, out: ran.stdout + ran.stderr };
}

test("Rust's compiler decides a landing: a field named method is no collision for a local renamed to method", (t) => {
  const lib = "pub struct Call {\n    pub method: String,\n}\n\npub fn name(call: &Call) -> usize {\n    let runner = call.method.len();\n    runner\n}\n";
  const root = tree(t, crate(lib));
  const { status, out } = runInCache(t, "apply", "oss", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "crates/demo/src/lib.rs"), lib.replaceAll("runner", "method"));
});

test("Rust's compiler refuses a rename that shadows a binding, at the shadowing site", (t) => {
  const lib = "pub fn pick(method: &str) -> usize {\n    let runner = method.len();\n    runner\n}\n";
  const root = tree(t, crate(lib));
  const { status, out } = runInCache(t, "apply", "oss", root, "--write");
  assert.equal(status, 1, out);
  assert.match(out, /^  crates\/demo\/src\/lib\.rs:2\trunner -> method\tclippy::shadow_[a-z]+: /m);
  assert.doesNotMatch(out, /lib\.rs:3\t/);
  assert.equal(read(root, "crates/demo/src/lib.rs"), lib);
});

test("a row decides one exact token in its file, at every site or only where its patterns match", () => {
  const rows = [
    { file: "src/contract.ts", word: "harness", becomes: "registry", after: /^ = await / },
    { file: "src/contract.ts", word: "harness", becomes: "test" },
  ];
  const line = "const harness = await create(); // the Harness";
  const decided = (file, token) => decisionsFor(rows, file, line, line.indexOf(token), token).map((row) => row.becomes);
  assert.deepEqual(decided("src/contract.ts", "harness"), ["registry", "test"]);
  assert.deepEqual(decided("src/contract.ts", "Harness"), []);
  assert.deepEqual(decided("src/other.ts", "harness"), []);
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

// Cloud's pins, which every Cloud tree holds: the lease cipher's label, the
// receipt words a test keeps, and spec.md's three canonical receipts.
const cloudSpec = "# Runx spec\n\nThe harness context names a run.\n\n```json\n{\n  \"schema\": \"runx.receipt.v1\",\n  \"id\": \"sha256:133fa57fec05b7d488717736317f8c52484b913dc45efc0b74728ecb84b8f0f4\",\n  \"subject\": { \"uri\": \"runx:run:harness-graph/first#default\", \"runner\": \"default\" }\n}\n```\n\n```json\n{\n  \"id\": \"sha256:1cf7174123f48dc4598c7653af483c89845157250864d88f910f25b656b5f30f\",\n  \"subject\": { \"uri\": \"runx:run:harness-stated#post\" },\n  \"verification\": [\n    { \"uri\": \"runx:provider_ack:harness-sha256:e99502936df0730d12c15a5e0406da43bb6e3400ec8ee4d16a16dae7cc0be95b\" },\n    { \"uri\": \"runx:harness:provider-readback:sha256:e99502936df0730d12c15a5e0406da43bb6e3400ec8ee4d16a16dae7cc0be95b\" }\n  ]\n}\n```\n\n```json\n{\n  \"id\": \"sha256:4ecdfe15d00f7ce518117d505270e4657e6868212a4d756a95249f01f0df8f6a\",\n  \"subject\": { \"uri\": \"runx:run:harness-graph\", \"label\": \"hello-graph\" }\n}\n```\n";
const cloudPin = {
  "packages/api/src/payments/holdings-postgres.ts": 'export const label = "purchase-holdings";\n',
  "packages/api/src/receipts/notary-projection.test.ts": 'export const subject = { ref_type: "harness" };\nexport const ref = { type: "harness", uri: "runx:harness:poisoned-secret-locator" };\n',
  "spec/spec.md": cloudSpec,
};

test("a pinned block in Markdown comes out byte-exact, while the rest of its file renames", (t) => {
  const root = tree(t, cloudPin);
  const { status, out } = run(RENAME, "apply", "cloud", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "spec/spec.md"), cloudSpec.replace("The harness context names a run.", "The run context names a run."));
  assert.equal(read(root, "packages/api/src/receipts/notary-projection.test.ts"), cloudPin["packages/api/src/receipts/notary-projection.test.ts"]);
});

test("a block pin that names no fenced block refuses", (t) => {
  const root = tree(t, { ...cloudPin, "spec/spec.md": cloudSpec.replace(/^\`\`\`(json)?$/gm, "") });
  const { status, out } = run(RENAME, "apply", "cloud", root, "--write");
  assert.equal(status, 1, out);
  assert.match(out, /^refused cloud: the pinned block holding "id": "sha256:133fa57f[0-9a-f]+" is no fenced block in spec\/spec\.md/m);
});

test("a row of bare-words.tsv names what a bare word becomes at its site, so Cloud's client.harness is client.tests", (t) => {
  const root = tree(t, {
    ...cloudPin,
    "packages/api/src/registry/native-harness.ts": "const result = await createNativeRuntimeClient(bin, env).harness.run({ skillPath });\nexport const test = result;\n",
  });
  const { status, out } = run(RENAME, "apply", "cloud", root, "--write");
  assert.equal(status, 0, out);
  assert.equal(read(root, "packages/api/src/registry/native-test.ts"), "const result = await createNativeRuntimeClient(bin, env).tests.run({ skillPath });\nexport const test = result;\n");
});

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

test("Cloud's copy of the CLI release it adopted keeps its text and its paths", (t) => {
  const release = {
    "apps/web/src/content/release/release.json": '{ "files": { "schemas/harness-context.schema.json": {} } }\n',
    "apps/web/src/content/release/schemas/schemas/harness-context.schema.json": '{ "title": "The harness context a runner reads." }\n',
  };
  const root = tree(t, { ...cloudPin, ...release, "apps/web/src/lib/release.ts": "// The harness-context schema ships with the release.\n" });
  const { status, out } = run(RENAME, "apply", "cloud", root, "--write");
  assert.equal(status, 0, out);
  for (const [file, text] of Object.entries(release)) assert.equal(read(root, file), text, file);
  assert.equal(read(root, "apps/web/src/lib/release.ts"), "// The run-context schema ships with the release.\n");
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
