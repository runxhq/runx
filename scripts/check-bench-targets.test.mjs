import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const checker = fileURLToPath(new URL("./check-bench-targets.mjs", import.meta.url));

function workspace(benchSection) {
  const root = mkdtempSync(path.join(os.tmpdir(), "runx-bench-targets-"));
  mkdirSync(path.join(root, "crates", "demo"), { recursive: true });
  writeFileSync(
    path.join(root, "crates", "demo", "Cargo.toml"),
    `[package]\nname = "demo"\nversion = "0.1.0"\n\n${benchSection}\n[[test]]\nname = "integration"\npath = "tests/integration.rs"\n`,
  );
  return root;
}

const check = (root) => execFileSync(process.execPath, [checker, root], { stdio: "pipe", encoding: "utf8" });

test("a bench target with test = false runs under libtest, so the check refuses it by file and target", () => {
  const root = workspace('[[bench]]\nname = "throughput"\ntest = false\n');
  try {
    assert.throws(() => check(root), (error) => {
      const said = `${error.stdout}${error.stderr}`;
      return said.includes(path.join("crates", "demo", "Cargo.toml")) && said.includes("throughput");
    });
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("a bench target that declares harness = false runs under Criterion", () => {
  const root = workspace('[[bench]]\nname = "throughput"\nharness = false\nrequired-features = ["agent"]\n');
  try {
    assert.doesNotThrow(() => check(root));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
