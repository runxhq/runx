import assert from "node:assert/strict";
import test from "node:test";

import { bundledCoreSkills, classifyFiles } from "./classify-ci-changes.mjs";

const core = ["receipt-audit", "skill-build"];

test("documentation changes stay in the light lane", () => {
  assert.deepEqual(classifyFiles(["docs/reference.md", "README.md"], core), {
    files: ["README.md", "docs/reference.md"],
    full: false,
    skills: false,
    core_skills: [],
    windows: false,
    release: false,
    light: true,
  });
});

test("a vendor skill is product content: light, never the runtime or a harness", () => {
  const result = classifyFiles(
    ["skills/reddit/run.yaml", "skills/reddit/reddit.mjs", "skills/runx-maintained.lock.json"],
    core,
  );
  assert.equal(result.full, false);
  assert.equal(result.skills, false);
  assert.equal(result.light, true);
});

test("a core skill change proves that skill without the runtime corpus", () => {
  const result = classifyFiles(["skills/skill-build/run.yaml"], core);
  assert.equal(result.full, false);
  assert.equal(result.skills, true);
  assert.deepEqual(result.core_skills, ["skill-build"]);
});

test("a runtime change proves every core skill and runs the full gate", () => {
  assert.deepEqual(classifyFiles(["crates/runx-runtime/src/process.rs"], core), {
    files: ["crates/runx-runtime/src/process.rs"],
    full: true,
    skills: true,
    core_skills: ["receipt-audit", "skill-build"],
    windows: true,
    release: false,
    light: false,
  });
});

test("non-platform runtime changes skip the Windows process lane", () => {
  const result = classifyFiles(["crates/runx-runtime/src/effects/provider_permission/readback.rs"], core);
  assert.equal(result.full, true);
  assert.equal(result.windows, false);
});

test("release tooling alone runs its own checks, not the runtime corpus", () => {
  assert.deepEqual(classifyFiles(["scripts/publish-npm-tarball.sh", "release/notes/0.10.1.md"], core), {
    files: ["release/notes/0.10.1.md", "scripts/publish-npm-tarball.sh"],
    full: false,
    skills: false,
    core_skills: [],
    windows: false,
    release: true,
    light: true,
  });
});

test("the release candidate smoke stays behind the full wall", () => {
  const result = classifyFiles(["scripts/smoke-release-candidate.mjs"], core);
  assert.equal(result.full, true);
  assert.equal(result.release, false);
});

test("unknown files run the full gate", () => {
  assert.equal(classifyFiles(["Makefile"], core).full, true);
});

test("an unavailable diff runs every lane", () => {
  assert.deepEqual(classifyFiles([], core), {
    files: [],
    full: true,
    skills: true,
    core_skills: ["receipt-audit", "skill-build"],
    windows: true,
    release: true,
    light: false,
  });
});

test("the core skills are the ten the binary bundles", () => {
  const names = bundledCoreSkills();
  assert.equal(names.length, 10);
  assert.ok(names.includes("skill-build"));
  assert.ok(!names.includes("reddit"));
});
