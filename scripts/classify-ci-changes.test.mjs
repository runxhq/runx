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

test("a non-core skill is product content: light, never the runtime or a test", () => {
  const result = classifyFiles(
    ["skills/reddit/run.yaml", "skills/reddit/reddit.mjs"],
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

test("the shared process environment runs the Windows startup regression", () => {
  const result = classifyFiles(["crates/runx-runtime/src/execution_environment.rs"], core);
  assert.equal(result.full, true);
  assert.equal(result.windows, true);
});

test("platform-sensitive runtime fixtures run the Windows compilation proof", () => {
  for (const file of ["crates/runx-runtime/src/services.rs", "crates/runx-runtime/tests/call_cancel.rs", "crates/runx-runtime/tests/skill_run.rs"]) {
    const result = classifyFiles([file], core);
    assert.equal(result.full, true, file);
    assert.equal(result.windows, true, file);
  }
});

test("registry installation runs its Windows filesystem regressions", () => {
  for (const file of ["crates/runx-runtime/src/registry/install.rs", "crates/runx-runtime/src/registry/connector_install.rs", "crates/runx-runtime/tests/registry_install.rs", "crates/runx-runtime/tests/registry_connector_install.rs"]) {
    const result = classifyFiles([file], core);
    assert.equal(result.full, true, file);
    assert.equal(result.windows, true, file);
  }
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

test("promotion scripts use the release checks without rebuilding the runtime", () => {
  for (const file of ["scripts/lib/release-candidate.mjs", "scripts/resolve-release-candidate.mjs", "scripts/release-candidate.test.mjs", "scripts/gen-channel-manifests.ts"]) {
    const result = classifyFiles([file], core);
    assert.equal(result.full, false, file);
    assert.equal(result.release, true, file);
  }
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

test("the core skills include the bundled memory owner", () => {
  const names = bundledCoreSkills();
  assert.equal(names.length, 13);
  assert.ok(names.includes("memory"));
  assert.ok(names.includes("skill-build"));
  assert.ok(names.includes("agent"));
  assert.ok(names.includes("revert"));
  assert.ok(!names.includes("reddit"));
});
