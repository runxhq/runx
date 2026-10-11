import { describe, expect, it } from "vitest";

import { finalizePolicy, preparePolicy } from "../skills/policy-draft/policy-draft.mjs";

describe("policy-draft domain validation", () => {
  it("rejects authority widening before native lint", () => {
    const existing = policy(["issue-intake"]);
    const proposed = policy(["issue-intake", "issue-to-pr"]);
    const prepared = preparePolicy({
      existing_policy: existing,
      policy_proposal: {
        decision: "ready",
        policy: proposed,
        rationale: "Add issue-to-pr.",
        blockers: [],
        needs_input: [],
        success_checkpoint: {},
      },
    }).policy_context;
    const proposal = finalizePolicy({ policy_context: prepared }).policy_proposal;

    expect(prepared.path).toBe("stop");
    expect(proposal.decision).toBe("reject");
    expect(proposal.validation.status).toBe("fail");
    expect(proposal.validation.findings).toContainEqual({
      code: "policy.attenuation.widened",
      path: "source_id.github-issues.allowed_actions.issue-to-pr",
      message: "The tightening lane cannot add or widen this authority.",
    });
  });

  it("fails closed when native lint rejects the exact candidate", () => {
    const prepared = preparePolicy({
      policy_proposal: {
        decision: "ready",
        policy: policy(["issue-intake"]),
        rationale: "Exercise the native rejection path.",
        blockers: [],
        needs_input: [],
        success_checkpoint: {},
      },
    }).policy_context;
    const proposal = finalizePolicy({
      policy_context: prepared,
      policy_lint: {
        status: "fail",
        findings: [{
          code: "policy.native_lint.invalid",
          path: "$",
          message: "The proposal could not be parsed or validated by the native policy engine.",
        }],
        readback: null,
      },
    }).policy_proposal;

    expect(proposal.decision).toBe("reject");
    expect(proposal.validation).toMatchObject({
      status: "fail",
      engine: "runx policy",
      readback: null,
    });
    expect(proposal.validation.findings).toContainEqual({
      code: "policy.native_lint.invalid",
      path: "$",
      message: "The proposal could not be parsed or validated by the native policy engine.",
    });
  });
});

function policy(actions: string[]) {
  return {
    sources: [{ source_id: "github-issues", allowed_locators: ["github://acme/acme/issues"], allowed_actions: actions }],
    runners: [{ runner_id: "local-review", allowed_actions: actions, target_repos: ["acme/acme"] }],
    targets: [{ repo: "acme/acme", allowed_actions: actions, runner_ids: ["local-review"] }],
    permissions: { auto_merge: false, mutate_target_repo: true, require_human_merge_gate: true },
  };
}
