import { mkdir, mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { afterEach, describe, expect, it } from "vitest";

import { createRunxClient } from "../packages/client/src/index.js";
import { runxBinary } from "./native-test-environment.js";

// The client's commands against the native CLI itself, so what the client
// sends is held to the CLI's grammar, not to a fake's.
describe("the client against the native CLI", () => {
  const roots: string[] = [];
  afterEach(async () => {
    await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
  });

  it("names a method as <ref>#<method>, and the CLI inspects that method", async () => {
    const root = await mkdtemp(path.join(os.tmpdir(), "runx-client-native-"));
    roots.push(root);
    const [cwd, projectDir, home] = ["workspace", "project", "home"].map((name) => path.join(root, name)) as [string, string, string];
    await Promise.all([cwd, projectDir, home].map((dir) => mkdir(dir)));
    const client = createRunxClient({ command: runxBinary, cwd, projectDir, home, environment: { PATH: process.env.PATH } });
    const inspection = await client.skills.inspect({ skillPath: path.resolve("skills/agent"), method: "write" });
    expect(inspection.method?.name).toBe("write");
  });
});
