import { describe, expect, it } from "vitest";
import { toOpenAiResponse } from "@runxhq/client/adapters";
import type { SkillOutputContract } from "@runxhq/contracts";

describe("openai native result formatting", () => {
  it.each(["completed", "failed", "blocked", "deferred"] as const)("preserves a sealed %s outcome", (outcome) => {
    const result: SkillOutputContract = { schema: "runx.skill_run.v1", status: "sealed", outcome, result: { message: "evidence" } };
    expect(toOpenAiResponse(result).structuredContent.runx).toBe(result);
  });
  it.each(["needs_agent", "needs_approval", "payment_required", "policy_denied", "failure"] as const)("preserves %s without turning it into completion", (status) => {
    const result: SkillOutputContract = { schema: "runx.skill_run.v1", status, outcome: "deferred" };
    expect(toOpenAiResponse(result).structuredContent.runx.status).toBe(status);
  });
});
