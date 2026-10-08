import { describe, expect, it, vi } from "vitest";
import { z } from "zod";
import type { RunxClient, SkillRunOutput } from "@runxhq/client";
import { createRunxLangChainTool } from "./index.js";

function makeTool(result: SkillRunOutput) {
  const run = vi.fn(async () => result);
  const client: Pick<RunxClient, "skills"> = { skills: { run, inspect: vi.fn() } };
  const wrapped = createRunxLangChainTool({
    name: "docs_pr", description: "Run governed docs work.", schema: z.object({ repo: z.string() }),
    client, runOptions: { skillPath: "/skills/docs", receiptDir: "/receipts" },
    mapInput: (input) => ({ repo_url: (input as { repo: string }).repo }),
  });
  return { wrapped, run };
}

describe("@runxhq/langchain", () => {
  it("maps input into the shared client operation", async () => {
    const { wrapped, run } = makeTool({ status: "sealed", outcome: "completed", result: "wrapped-output" });
    expect(await wrapped.invoke({ repo: "acme/docs" })).toBe("wrapped-output");
    expect(run).toHaveBeenCalledWith({ skillPath: "/skills/docs", receiptDir: "/receipts", inputs: { repo_url: "acme/docs" } });
  });
  it.each([
    { status: "sealed", outcome: "failed" }, { status: "sealed", outcome: "blocked" },
    { status: "needs_agent", outcome: "deferred" }, { status: "needs_approval", outcome: "deferred" },
    { status: "payment_required", outcome: "deferred" }, { status: "policy_denied", outcome: "blocked" },
  ] as const)("leaves $status/$outcome to the host", async (result) => {
    await expect(makeTool(result).wrapped.invoke({ repo: "acme/docs" })).rejects.toThrow("host must handle this outcome");
  });
});
