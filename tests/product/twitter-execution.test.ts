import { describe, expect, it } from "vitest";

import { bindExecution, prepareExecution } from "../../skills/twitter/twitter-execution.mjs";

const DIGEST = `sha256:${"a".repeat(64)}`;

describe("twitter execution plans", () => {
  it("refuses post text that is not a non-empty string within the connector's bound", () => {
    for (const params of [{ text: 42 }, { text: "   " }, { text: "x".repeat(4001) }]) {
      expect(prepare([{ act_id: "act-1", kind: "post", params }]).blockers).toEqual([
        "act act-1 text must be a non-empty string of at most 4000 characters",
      ]);
    }
    expect(prepare([{ act_id: "act-1", kind: "post", params: { text: "x".repeat(4000) } }]).decision).toBe("ready");
  });

  it("refuses thread texts that are not 1 to 25 non-empty strings", () => {
    for (const texts of ["hello", [], ["one", 2], ["one", ""], Array.from({ length: 26 }, () => "x")]) {
      expect(prepare([{ act_id: "act-1", kind: "thread", params: { texts } }]).blockers).toContain(
        "thread act-1 texts must be 1 to 25 non-empty strings of at most 4000 characters",
      );
    }
    const plan = prepare([{ act_id: "act-1", kind: "thread", params: { texts: ["one", "two"] } }]);
    expect(plan.batch?.input).toEqual({ text: "one" });
  });

  it("refuses ids that are not X id strings", () => {
    expect(prepare([{ act_id: "act-1", kind: "like", params: { post_id: 5001 } }]).blockers).toEqual([
      "act act-1 has ids that are not X ids: post_id",
    ]);
  });

  it("refuses a principal that does not name an account handle", () => {
    expect(prepare([{ act_id: "act-1", kind: "post", params: { text: "hi" } }], "the example account").blockers).toEqual([
      "plan_json.principal must name the account as account:@handle",
    ]);
  });

  it("binds only the account the plan names and shows it to the approver", () => {
    const plan = prepare([{ act_id: "act-1", kind: "post", params: { text: "hi" } }]);
    const call = bindExecution({ execution_plan: plan, account: { id: "42", username: "Example" } }).twitter_call;
    expect(call.approval.reason).toContain("on X as @Example (account 42)");
    expect(call.account_id).toBe("42");
    expect(() => bindExecution({ execution_plan: plan, account: { id: "43", username: "alice" } })).toThrow(
      "the bound X account @alice is not the plan's principal account:@example",
    );
    expect(() => bindExecution({ execution_plan: plan, account: { username: "example" } })).toThrow(
      "the account snapshot did not name the bound account",
    );
  });
});

function prepare(acts: Record<string, unknown>[], principal = "account:@example") {
  return prepareExecution({
    plan_json: {
      decision: "ready",
      principal,
      acts: acts.map((act) => ({ consequence: "public_send", rationale: "fixture", ...act })),
      gates: { human_approval_required: true },
    },
    plan_digest: DIGEST,
    digest_result: { digest: DIGEST },
    execution_ledger: { after_version: 0, events: [] },
  }).twitter_execution_plan;
}
