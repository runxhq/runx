import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
test("the native loader admits the managed Link connector without an operator login", () => {
  const isolated = mkdtempSync(path.join(tmpdir(), "link-connector-inspect-"));
  try {
    const inspected = JSON.parse(execFileSync(process.env.RUNX_BIN ?? path.join(root, "crates/target/debug/runx"),
      ["connector", "inspect", "link", "--json"], { cwd: root, encoding: "utf8",
        env: { PATH: process.env.PATH ?? "/usr/bin:/bin", RUNX_CWD: root, RUNX_PROJECT_DIR: isolated, NO_COLOR: "1" } }));
    assert.equal(inspected.status, "success");
    assert.equal(inspected.connector.auth.session.kind, "managed_command");
    assert.equal(inspected.connector.placements.hosted, undefined);
    assert.deepEqual(Object.keys(inspected.connector.operations).sort(), ["account.read", "payment.mpp", "spend.read", "spend.request"]);
    assert.equal(inspected.connector.operations["payment.mpp"].approval, "required");
  } finally { rmSync(isolated, { recursive: true, force: true }); }
});
