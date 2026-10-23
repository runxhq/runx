import { describe, expect, it } from "vitest";

import { finalizeExecution } from "../../skills/twitter/twitter-execution-result.mjs";

describe("twitter execution provider outcomes", () => {
  it("treats following:false as a completed unfollow", () => {
    const execution = finalize("unfollow", [applied({ following: false })]);

    expect(execution).toMatchObject({
      decision: "executed",
      next_act_index: 1,
      remaining_count: 0,
      results: [
        {
          kind: "unfollow",
          status: "done",
          provider_ref: "target-1",
          detail: null,
        },
      ],
    });
  });

  it("records a typed mutation that did not reach its requested state as failed and settled", () => {
    expect(finalize("unfollow", [applied({ following: true })])).toMatchObject({
      decision: "completed_with_failures",
      next_act_index: 1,
      counts: { processed: 1, succeeded: 0, failed: 1 },
      results: [{ status: "failed" }],
      ledger_delta: { batch: { processed_act_count: 1, succeeded_act_count: 0, failed_act_count: 1 } },
    });
    expect(finalize("follow", [applied({ following: true })])).toMatchObject({
      next_act_index: 1,
      results: [{ status: "done" }],
    });
    expect(finalize("delete_post", [applied({ deleted: false })])).toMatchObject({
      results: [{ status: "failed" }],
    });
  });

  it("stops the cursor at an applied act whose answer does not state the resulting state", () => {
    const execution = finalize("unfollow", [applied({}), applied({ following: false })]);
    expect(execution).toMatchObject({
      decision: "stopped",
      next_act_index: 0,
      remaining_count: 2,
      results: [{ status: "failed", detail: "the provider's answer did not state the resulting state" }],
      ledger_delta: { account_id: "42", batch: { processed_act_count: 0, succeeded_act_count: 0, failed_act_count: 0, rate_limited: false } },
    });
    expect(execution.results).toHaveLength(1);
    expect(finalize("delete_post", [applied({ deleted: "yes" })])).toMatchObject({
      next_act_index: 0,
      results: [{ status: "failed" }],
    });
  });

  it("counts an unfollow of an inactive account as done", () => {
    const execution = finalize("unfollow", [
      refused({
        detail: "One or more parameters to your request was invalid.",
        errors: [{ message: "You cannot unfollow an account that is not active." }],
        title: "Invalid Request",
      }),
    ]);

    expect(execution).toMatchObject({
      decision: "executed",
      next_act_index: 1,
      remaining_count: 0,
      results: [
        {
          kind: "unfollow",
          status: "done",
          provider_ref: "target-1",
          detail: "Target account is inactive; the requested not-following state is already satisfied.",
        },
      ],
      ledger_delta: { batch: { processed_act_count: 1, succeeded_act_count: 1, failed_act_count: 0 } },
    });
  });

  it("records other refusals with the provider's reason and moves past them", () => {
    expect(finalize("unfollow", [refused({ errors: [{ message: "Another invalid request." }] })])).toMatchObject({
      decision: "completed_with_failures",
      next_act_index: 1,
      counts: { processed: 1, succeeded: 0, failed: 1 },
      results: [{ status: "failed", detail: "Another invalid request." }],
    });
    expect(finalize("follow", [refused({ errors: [{ message: "You cannot unfollow an account that is not active." }] })])).toMatchObject({
      results: [{ status: "failed" }],
    });
  });

  it("retains the settled failure when a later act halts and then succeeds", () => {
    const plan = executionPlan("unfollow", 2);
    const first = finalizeExecution({
      execution_plan: plan,
      account_id: "42",
      provider_operation: {
        readback_uri: "fixture:first",
        result: { outcomes: [
          refused({ detail: "The first act was refused." }),
          { status: "refused", code: "rate_limited", retry_at: "2027-01-15T08:00:00Z" },
        ] },
      },
    }).twitter_execution;
    expect(first.counts).toEqual({ processed: 1, succeeded: 0, failed: 1 });
    expect(first.last_failure).toEqual({
      act_id: "act-001", reason: "The first act was refused.", evidence_ref: "fixture:first",
    });
    const second = finalizeExecution({
      execution_plan: {
        ...plan, start_act_index: 1, next_act_index: 1,
        counts: first.counts, last_failure: first.last_failure,
        batch: { ...plan.batch, acts: [plan.batch.acts[1]] },
      },
      account_id: "42",
      provider_operation: {
        readback_uri: "fixture:second", result: { outcomes: [applied({ following: false })] },
      },
    }).twitter_execution;
    expect(second.decision).toBe("completed_with_failures");
    expect(second.counts).toEqual({ processed: 2, succeeded: 1, failed: 1 });
    expect(second.last_failure).toEqual(first.last_failure);
  });

  it("stops the cursor where the provider refused the account rather than the act", () => {
    const halted = finalize("unfollow", [
      applied({ following: false }),
      { status: "refused", code: "rate_limited", reason: "the provider rate limited the request", retry_at: "2027-01-15T08:00:00Z" },
      { status: "unsent", code: "rate_limited", reason: "item 2 of 3 halted the batch" },
    ]);
    expect(halted).toMatchObject({
      decision: "stopped",
      next_act_index: 1,
      remaining_count: 2,
      rate: { limited: true, remaining: 0, reset_at: "2027-01-15T08:00:00Z" },
      ledger_delta: { batch: { processed_act_count: 1, succeeded_act_count: 1, failed_act_count: 0, rate_limited: true } },
    });
    expect(halted.results).toHaveLength(2);
    expect(halted.last_failure).toBeNull();
    expect(halted.stop).toMatchObject({ action: "wait", code: "rate_limited", retry_at: "2027-01-15T08:00:00Z" });
    expect(halted.counts).toEqual({ processed: 1, succeeded: 1, failed: 0 });

    const forbidden = finalize("unfollow", [
      refused({
        type: "https://api.twitter.com/2/problems/oauth1-permissions",
        detail: "Your client app is not configured with the appropriate oauth1 app permissions for this endpoint.",
      }),
      refused({ type: "https://api.twitter.com/2/problems/oauth1-permissions" }),
    ]);
    expect(forbidden).toMatchObject({
      decision: "stopped",
      next_act_index: 0,
      results: [{ status: "failed", detail: "Your client app is not configured with the appropriate oauth1 app permissions for this endpoint." }],
    });
    expect(forbidden.stop).toMatchObject({ action: "review", code: "provider_rejected" });
  });
});

function applied(data: Record<string, unknown>) {
  return { status: "applied", acknowledgement: { data } };
}

function refused(answer: Record<string, unknown>) {
  return { status: "refused", code: "provider_rejected", reason: "the provider returned HTTP 400", answer };
}

function finalize(kind: string, outcomes: Record<string, unknown>[]) {
  return finalizeExecution({
    execution_plan: executionPlan(kind, outcomes.length),
    provider_operation: { status: "success", readback_uri: "fixture:native-readback", result: { outcomes } },
    account_id: "42",
  }).twitter_execution;
}

function executionPlan(kind: string, count: number) {
  const acts = Array.from({ length: count }, (_, index) => ({
    act_id: `act-00${index + 1}`,
    act_index: index,
    kind,
    consequence: "live_mutation",
    target: `target-${index + 1}`,
  }));
  return {
    decision: "ready",
    plan_digest: "sha256:fixture",
    principal: "account:@fixture",
    start_act_index: 0,
    next_act_index: 0,
    total_act_count: count,
    remaining_count: count,
    counts: { processed: 0, succeeded: 0, failed: 0 },
    last_failure: null,
    stop: null,
    batch: { kind, operation: "users.unfollow", input: {}, self: true, acts, segment: null },
  };
}
