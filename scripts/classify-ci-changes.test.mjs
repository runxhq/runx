import assert from "node:assert/strict";
import test from "node:test";

import { classifyFiles } from "./classify-ci-changes.mjs";

test("documentation changes stay in the light lane", () => {
  assert.deepEqual(classifyFiles(["docs/reference.md", "README.md"]), {
    files: ["README.md", "docs/reference.md"],
    full: false,
    skills: false,
    windows: false,
    light: true,
  });
});

test("skill-only changes run the catalog without the workspace or Windows corpus", () => {
  assert.deepEqual(classifyFiles(["skills/twitter/SKILL.md", "skills/twitter/run.yaml"]), {
    files: ["skills/twitter/SKILL.md", "skills/twitter/run.yaml"],
    full: false,
    skills: true,
    windows: false,
    light: false,
  });
});

test("runtime changes run every affected proof", () => {
  assert.deepEqual(classifyFiles(["crates/runx-runtime/src/process.rs"]), {
    files: ["crates/runx-runtime/src/process.rs"],
    full: true,
    skills: true,
    windows: true,
    light: false,
  });
});

test("non-platform runtime changes skip the Windows process lane", () => {
  assert.deepEqual(classifyFiles(["crates/runx-runtime/src/effects/provider_permission/readback.rs"]), {
    files: ["crates/runx-runtime/src/effects/provider_permission/readback.rs"],
    full: true,
    skills: true,
    windows: false,
    light: false,
  });
});

test("known package changes skip unrelated skill and Windows sweeps", () => {
  assert.deepEqual(classifyFiles(["packages/contracts/src/internal.ts"]), {
    files: ["packages/contracts/src/internal.ts"],
    full: true,
    skills: false,
    windows: false,
    light: false,
  });
});

test("the workspace lock keeps its Windows proof", () => {
  assert.deepEqual(classifyFiles(["crates/Cargo.lock"]), {
    files: ["crates/Cargo.lock"],
    full: true,
    skills: true,
    windows: true,
    light: false,
  });
});

test("unknown paths fail closed into the full and skill lanes", () => {
  assert.deepEqual(classifyFiles(["new-surface/config.toml"]), {
    files: ["new-surface/config.toml"],
    full: true,
    skills: true,
    windows: false,
    light: false,
  });
});

test("an unavailable diff runs all lanes", () => {
  assert.deepEqual(classifyFiles([]), {
    files: [],
    full: true,
    skills: true,
    windows: true,
    light: false,
  });
});
