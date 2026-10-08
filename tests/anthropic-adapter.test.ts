import { describe, expect, it } from "vitest";
import { toAnthropicResponse } from "@runxhq/client/adapters";
import type { SkillOutputContract } from "@runxhq/contracts";

describe("anthropic native result formatting", () => {
  it.each(["completed", "failed", "blocked", "deferred"] as const)("preserves a sealed %s outcome", (outcome) => {
    const result: SkillOutputContract = { schema: "runx.skill_run.v1", status: "sealed", outcome, result: { message: "evidence" } };
    expect(toAnthropicResponse(result).metadata.runx).toBe(result);
  });
  it.each(["needs_agent", "needs_approval", "payment_required", "policy_denied", "failure"] as const)("preserves %s without turning it into completion", (status) => {
    const result: SkillOutputContract = { schema: "runx.skill_run.v1", status, outcome: "deferred" };
    expect(toAnthropicResponse(result).metadata.runx.status).toBe(status);
  });
});
