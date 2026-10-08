import { describe, expect, it } from "vitest";
import { toVercelAiResponse } from "@runxhq/client/adapters";
import type { SkillOutputContract } from "@runxhq/contracts";

describe("vercel-ai native result formatting", () => {
  it.each(["completed", "failed", "blocked", "deferred"] as const)("preserves a sealed %s outcome", (outcome) => {
    const result: SkillOutputContract = { schema: "runx.skill_run.v1", status: "sealed", outcome, result: { message: "evidence" } };
    expect(toVercelAiResponse(result).data.runx).toBe(result);
  });
  it.each(["needs_agent", "needs_approval", "payment_required", "policy_denied", "failure"] as const)("preserves %s without turning it into completion", (status) => {
    const result: SkillOutputContract = { schema: "runx.skill_run.v1", status, outcome: "deferred" };
    expect(toVercelAiResponse(result).data.runx.status).toBe(status);
  });
});
