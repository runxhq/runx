import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const checker = fileURLToPath(new URL("./check-publishable-package-manifests.mjs", import.meta.url));

test("publishable package check refuses an incompatible workspace range", () => {
  const root = mkdtempSync(path.join(os.tmpdir(), "runx-package-ranges-"));
  try {
    mkdirSync(path.join(root, "scripts"));
    mkdirSync(path.join(root, "packages/contracts"), { recursive: true });
    mkdirSync(path.join(root, "packages/client"), { recursive: true });
    copyFileSync(checker, path.join(root, "scripts/check-publishable-package-manifests.mjs"));
    writeFileSync(path.join(root, "packages/contracts/package.json"), JSON.stringify({ name: "@runxhq/contracts", version: "0.4.1" }));
    const clientManifest = path.join(root, "packages/client/package.json");
    const run = () => execFileSync(process.execPath, [path.join(root, "scripts/check-publishable-package-manifests.mjs")], { stdio: "pipe" });

    writeFileSync(clientManifest, JSON.stringify({ name: "@runxhq/client", version: "0.1.0", dependencies: { "@runxhq/contracts": "workspace:^0.3.0" } }));
    assert.throws(run, /does not allow workspace version 0\.4\.1/u);

    writeFileSync(clientManifest, JSON.stringify({ name: "@runxhq/client", version: "0.1.0", dependencies: { "@runxhq/contracts": "workspace:^0.4.0" } }));
    assert.doesNotThrow(run);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
