import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { promisify } from "node:util";

import { describe, expect, it } from "vitest";

const execFileAsync = promisify(execFile);
const nativeRunx = process.env.RUNX_RUST_CLI_BIN
  ? path.resolve(process.env.RUNX_RUST_CLI_BIN)
  : path.resolve("crates", "target", "debug", process.platform === "win32" ? "runx.exe" : "runx");

describe("hello-workflow example", () => {
  it("runs its two-step method through the native test runner", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-hello-workflow-example-"));
    try {
      const { stdout, stderr } = await execFileAsync(
        requireNativeRunx(),
        ["test", "examples/hello-workflow", "--json"],
        {
          env: {
            ...process.env,
            NO_COLOR: "1",
            RUNX_HOME: path.join(tempDir, "home"),
            RUNX_RECEIPT_DIR: path.join(tempDir, "receipts"),
          },
        },
      );

      expect(stderr).toBe("");
      // The test asserts the sealed receipt itself: completed, with a child
      // for each of the two steps.
      const report = JSON.parse(stdout) as {
        readonly status?: string;
        readonly case_names?: readonly string[];
        readonly assertion_errors?: readonly string[];
        readonly receipt_ids?: readonly string[];
      };
      expect(report.status).toBe("passed");
      expect(report.case_names).toEqual(["hello-workflow"]);
      expect(report.assertion_errors).toEqual([]);
      expect(report.receipt_ids).toHaveLength(1);
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });
});

function requireNativeRunx(): string {
  if (!existsSync(nativeRunx)) {
    throw new Error(`native example tests require a built runx binary at ${nativeRunx}`);
  }
  return nativeRunx;
}
