import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { describe, expect, it } from "vitest";

import {
  defineTool,
  failure,
  firstNonEmptyString,
  prune,
} from "./index.js";

const tsxBin = path.join(process.cwd(), "node_modules", ".bin", process.platform === "win32" ? "tsx.cmd" : "tsx");

describe("@runxhq/extension-sdk", () => {
  it("transports runtime-materialized tool inputs without reinterpreting them", async () => {
    const tool = defineTool({
      name: "demo.echo",
      run({ inputs }) {
        return { message: inputs.message };
      },
    });

    await expect(tool.runWith({ message: "hello" })).resolves.toEqual({ message: "hello" });
  });

  it("preserves structured failures", async () => {
    const tool = defineTool({
      name: "demo.fail",
      run() {
        return failure({ error: { code: "invalid_input" } }, { exitCode: 2, stderr: "bad input" });
      },
    });

    await expect(tool.runWith()).resolves.toMatchObject({
      output: { error: { code: "invalid_input" } },
      exitCode: 2,
      stderr: "bad input",
    });
  });

  it("preserves intentional empty collections in tool output contracts", async () => {
    const tool = defineTool({
      name: "demo.empty-output",
      run() {
        return { writes: [], metadata: {}, omitted: undefined };
      },
    });

    await expect(tool.runWith()).resolves.toEqual({ writes: [], metadata: {} });
  });

  it("loads the runtime-authored input file for a process tool", async () => {
    const tempDir = await mkdtemp(path.join(os.tmpdir(), "runx-tool-input-"));
    const inputPath = path.join(tempDir, "inputs.json");
    const toolPath = path.join(tempDir, "tool.mts");
    try {
      await writeFile(inputPath, JSON.stringify({ message: "from-file" }), "utf8");
      await writeFile(toolPath, [
        `import { defineTool } from ${JSON.stringify(new URL("./index.ts", import.meta.url).href)};`,
        'const tool = defineTool({ name: "demo.process", run: ({ inputs }) => inputs });',
        "await tool.main();",
      ].join("\n"), "utf8");
      const stdout = await runProcess(tsxBin, [toolPath], "", {
        RUNX_INPUTS_JSON: JSON.stringify({ message: "stale-inline" }),
        RUNX_INPUTS_PATH: inputPath,
      });
      expect(JSON.parse(stdout)).toEqual({ message: "from-file" });
    } finally {
      await rm(tempDir, { recursive: true, force: true });
    }
  });

  it("exports shared value helpers for process extensions", () => {
    expect(firstNonEmptyString("", undefined, " docs ")).toBe("docs");
    expect(prune({ keep: "yes", drop: undefined, empty: [], nested: { value: undefined } })).toEqual({ keep: "yes" });
  });

  it("keeps extension helpers free of runtime packages", async () => {
    const source = await readFile(new URL("./index.ts", import.meta.url), "utf8");
    const forbiddenPackages = ["runtime-local", "adapters"].map((name) => `@runxhq/${name}`);

    for (const packageName of forbiddenPackages) {
      expect(source).not.toContain(packageName);
    }
  });
});

async function runProcess(
  command: string,
  args: readonly string[],
  stdin: string,
  env: NodeJS.ProcessEnv = {},
): Promise<string> {
  const child = spawn(command, [...args], {
    cwd: process.cwd(),
    env: { ...process.env, ...env },
    stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.setEncoding("utf8");
  child.stderr.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    stdout += chunk;
  });
  child.stderr.on("data", (chunk: string) => {
    stderr += chunk;
  });

  const closed = new Promise<string>((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) {
        resolve(stdout);
        return;
      }
      reject(new Error(`${command} exited ${code ?? "without status"}: ${stderr}`));
    });
  });
  child.stdin.end(stdin && !stdin.endsWith("\n") ? `${stdin}\n` : stdin);
  return closed;
}
