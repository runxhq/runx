import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";

const GUARD = new URL("./stale-words.mjs", import.meta.url).pathname;

function tree(t, files) {
  const root = mkdtempSync(path.join(tmpdir(), "runx-stale-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  execFileSync("git", ["init", "--quiet", root]);
  for (const [file, text] of Object.entries(files)) {
    mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
    writeFileSync(path.join(root, file), text);
  }
  return root;
}

test("the guard fails on runner as a method and harness as a test, and passes the stays, kept names and history", (t) => {
  const root = tree(t, {
    "crates/runx-runtime/src/skill.rs": "fn inputs(skill: &Skill) -> &Inputs { &skill.runner.inputs }\n",
    "skills/demo/SKILL.md": "Run the harness before you publish.\n",
    "crates/runx-cli/tests/router.rs": 'assert!(refused("--runner"));\n',
    "scripts/ci.sh": "echo $RUNNER_OS\n",
    "docs/release/notes/0.10.md": "The harness ran every case.\n",
    "pnpm-lock.yaml": "packages:\n  '@vitest/runner@4.1.12':\n    resolution: {}\n",
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    "crates/runx-runtime/src/skill.rs:1\trunner\trunner\tfn inputs(skill: &Skill) -> &Inputs { &skill.runner.inputs }\n" +
      "skills/demo/SKILL.md:1\tharness\tharness\tRun the harness before you publish.\n" +
      "stale: 1 uses of runner as a method, 1 of harness as a test or run, 0 of graph as a workflow in prose, 0 of execution closure, which is closure, 0 of a key the format removes and 0 paths holding a retired word, in 2 files\n",
  );
});

test("the guard fails on graph as a workflow in prose and in code, and passes another graph and code spans", (t) => {
  const root = tree(t, {
    "README.md": "Compose skills into a graph.\nThe Rust crate graph is enforced.\nRun `runx list graphs`.\n\n```yaml\ngraph: demo\n```\n",
    "crates/runx-parser/src/workflow.rs": "pub struct Method { pub graph: Graph }\n",
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    "README.md:1\tgraph\tgraph\tCompose skills into a graph.\n" +
      "crates/runx-parser/src/workflow.rs:1\tgraph\tgraph\tpub struct Method { pub graph: Graph }\n" +
      "crates/runx-parser/src/workflow.rs:1\tgraph\tGraph\tpub struct Method { pub graph: Graph }\n" +
      "stale: 0 uses of runner as a method, 0 of harness as a test or run, 3 of graph as a workflow in prose, 0 of execution closure, which is closure, 0 of a key the format removes and 0 paths holding a retired word, in 2 files\n",
  );
});

test("the guard counts graph in a message a person reads and in a name, and not a wire value", (t) => {
  const root = tree(t, { "crates/runx-runtime/src/error.rs": 'pub const FAILED: &str = "graph step failed";\npub const KIND: &str = "graph";\nfn graph() {}\n' });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    'crates/runx-runtime/src/error.rs:1\tgraph\tgraph\tpub const FAILED: &str = "graph step failed";\n' +
      "crates/runx-runtime/src/error.rs:3\tgraph\tgraph\tfn graph() {}\n" +
      "stale: 0 uses of runner as a method, 0 of harness as a test or run, 2 of graph as a workflow in prose, 0 of execution closure, which is closure, 0 of a key the format removes and 0 paths holding a retired word, in 1 files\n",
  );
});

test("the guard fails on a key the format removes where it's written as one, and passes another sense of its spelling", (t) => {
  const root = tree(t, {
    "skills/demo/run.yaml":
      "minimum_runx: 0.12.0\nmethods:\n  run:\n    inputs:\n      message: {type: string, required: true}\n    steps:\n      - id: read\n        tool: demo.read\n        outputs:\n          expected_result: {type: object}\n      - id: send\n        operation: demo/send\n        expected_result: {sent: true}\n",
    "connectors/demo/connector.yaml": "name: demo\noperations:\n  send:\n    result_fields: [sent]\n",
    "tools/demo/read/manifest.json": '{"schema": "runx.tool.manifest.v1", "name": "demo.read", "source": {"command": "node"}, "scopes": []}\n',
    "skills/demo/tests/reads.yaml": "name: reads\nmethod: run\ninputs:\n  criteria:\n    - {id: fit, required: true}\nexpect:\n  status: sealed\n",
    "docs/reference.md": "Each package's metadata records `manifest_digest`, `connector_digest` and `runtime_capability`.\n",
    "docs/demo.md": "Declare `context_skills` on a step.\n\n```yaml\nmethods:\n  run:\n    steps:\n      - id: a\n        tool: demo.read\n        artifacts: {wrap_as: result}\n```\n\nA connector's `result_fields` name what it returns.\n",
    "crates/runx-parser/tests/refusals.rs": 'const OLD: &str = "minimum_runx: 0.12.0\\nmethods:\\n  run:\\n    steps:\\n      - id: a\\n        tool: demo.read\\n        scopes: [fs.read]\\n";\nfn operation_name() {}\n',
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  const flagged = run.stdout.split("\n").filter((line) => line.includes("\tkey\t")).map((line) => line.split("\t").slice(0, 3).join(" "));
  assert.deepEqual(flagged, [
    "crates/runx-parser/tests/refusals.rs:1 key scopes",
    "docs/demo.md:1 key context_skills",
    "docs/demo.md:9 key artifacts",
    "docs/demo.md:9 key wrap_as",
    "skills/demo/run.yaml:5 key required",
    "skills/demo/run.yaml:13 key expected_result",
    "tools/demo/read/manifest.json:1 key source",
    "tools/demo/read/manifest.json:1 key scopes",
  ]);
  assert.match(run.stdout, /, 0 of execution closure, which is closure, 8 of a key the format removes and 0 paths holding a retired word, in 4 files\n$/);
});

test("the guard fails on a path whose own name holds a retired word, once per path, and passes a path stay and another word", (t) => {
  const root = tree(t, {
    "crates/runx-runtime/src/skill_package/runner.rs": "pub mod turn;\n",
    "crates/runx-runtime/src/skill_package/runner/turn.rs": "pub fn turn() {}\n",
    "crates/runx-runtime/src/skill_package/runner/floor/tests.rs": "fn floor() {}\n",
    "docs/harness-control-plane.md": "# Run control plane\n",
    "fixtures/kernel/graphs/one.json": "{}\n",
    ".github/workflows/runner-cache.yml": "name: cache\n",
    "scripts/check-rust-crate-graph.mjs": "// Checks the crate graph.\n",
    "release/notes/0.10-runner.md": "Old notes.\n",
    "docs/paragraph_runners-guide.txt": "A paragraph.\n",
    "docs/paragraph.md": "A paragraph.\n",
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    "crates/runx-runtime/src/skill_package/runner.rs\tpath\trunner\trunner.rs\n" +
      "crates/runx-runtime/src/skill_package/runner/\tpath\trunner\trunner\n" +
      "docs/harness-control-plane.md\tpath\tharness\tharness-control-plane.md\n" +
      "docs/paragraph_runners-guide.txt\tpath\trunner\tparagraph_runners-guide.txt\n" +
      "fixtures/kernel/graphs/\tpath\tgraph\tgraphs\n" +
      "stale: 0 uses of runner as a method, 0 of harness as a test or run, 0 of graph as a workflow in prose, 0 of execution closure, which is closure, 0 of a key the format removes and 5 paths holding a retired word, in 0 files\n",
  );
});

test("the guard fails on execution closure in any form, and passes a closure and a publish closure", (t) => {
  const root = tree(t, {
    "docs/inspect.md": "Inspect the execution closure. Execution closures bind packages.\nThe closure binds them; a publish closure signs them.\n",
    "crates/runx-runtime/src/inspect.rs": "struct LocalExecutionClosure;\nfn f(expected_execution_closure_digest: &str, closure_digest: &str) {}\n",
    "packages/client/src/inspect.ts": 'const executionClosureDigest = run("--execution-closure-digest");\n',
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    "crates/runx-runtime/src/inspect.rs:1\tclosure\tExecutionClosure\tstruct LocalExecutionClosure;\n" +
      "crates/runx-runtime/src/inspect.rs:2\tclosure\texecution_closure\tfn f(expected_execution_closure_digest: &str, closure_digest: &str) {}\n" +
      "docs/inspect.md:1\tclosure\texecution closure\tInspect the execution closure. Execution closures bind packages.\n" +
      "docs/inspect.md:1\tclosure\tExecution closures\tInspect the execution closure. Execution closures bind packages.\n" +
      "packages/client/src/inspect.ts:1\tclosure\texecutionClosure\tconst executionClosureDigest = run(\"--execution-closure-digest\");\n" +
      "packages/client/src/inspect.ts:1\tclosure\texecution-closure\tconst executionClosureDigest = run(\"--execution-closure-digest\");\n" +
      "stale: 0 uses of runner as a method, 0 of harness as a test or run, 0 of graph as a workflow in prose, 6 of execution closure, which is closure, 0 of a key the format removes and 0 paths holding a retired word, in 3 files\n",
  );
});

test("Cargo's harness key in a bench or test table is Cargo's, and stays, as does the key as the bench targets check writes it", (t) => {
  const root = tree(t, {
    "crates/runx-runtime/Cargo.toml": '[package]\nname = "runx-runtime"\n\n[[bench]]\nname = "throughput"\nharness = false\n\n[[test]]\nname = "integration"\nharness = true\n',
    "crates/runx-runtime/benches/throughput.rs": "// Run the harness over each case.\nfn main() {}\n",
    "scripts/check-bench-targets.mjs":
      '// A target without it runs under libtest\'s test runner.\nconst CRITERION_MAIN = "harness = false";\n// The harness ran.\n',
    "scripts/verify-fast.mjs":
      'step("bench targets", "node", ["scripts/check-bench-targets.mjs"]),\n// Benches run only as harness = false targets.\n',
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    "crates/runx-runtime/benches/throughput.rs:1\tharness\tharness\t// Run the harness over each case.\n" +
      "scripts/check-bench-targets.mjs:3\tharness\tharness\t// The harness ran.\n" +
      "scripts/verify-fast.mjs:2\tharness\tharness\t// Benches run only as harness = false targets.\n" +
      "stale: 0 uses of runner as a method, 3 of harness as a test or run, 0 of graph as a workflow in prose, 0 of execution closure, which is closure, 0 of a key the format removes and 0 paths holding a retired word, in 3 files\n",
  );
});
