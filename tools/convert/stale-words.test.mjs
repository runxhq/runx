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

test("the guard fails on runner as a method and harness as a test, and passes the machinery, kept names and history", (t) => {
  const root = tree(t, {
    "crates/runx-runtime/src/skill.rs": "fn inputs(skill: &Skill) -> &Inputs { &skill.runner.inputs }\n",
    "skills/demo/SKILL.md": "Run the harness before you publish.\n",
    "crates/runx-runtime/src/execution/runner/step.rs": "pub struct Runner;\n",
    "crates/runx-runtime/src/lib.rs": "use crate::execution::runner::Runner;\n",
    "scripts/ci.sh": "echo $RUNNER_OS\n",
    "docs/release/notes/0.10.md": "The harness ran every case.\n",
  });
  const run = spawnSync(process.execPath, [GUARD, root, "--list"], { encoding: "utf8" });
  assert.equal(run.status, 1);
  assert.equal(
    run.stdout,
    "crates/runx-runtime/src/skill.rs:1\trunner\trunner\tfn inputs(skill: &Skill) -> &Inputs { &skill.runner.inputs }\n" +
      "skills/demo/SKILL.md:1\tharness\tharness\tRun the harness before you publish.\n" +
      "stale: 1 uses of runner as a method and 1 of harness as a test or run, in 2 files\n",
  );
});
