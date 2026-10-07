import { actKind, reachedState } from "./twitter-execution-batch.mjs";
import {
  activeThread,
  array,
  nonNegativeInteger,
  record,
  text,
} from "./twitter-execution-values.mjs";

// A refusal of the caller rather than the act leaves the act to a later
// batch: the kernel's halts (a rate limit, a refused credential, a billing
// hold) and X's problems with the app or its access, which it answers per
// request but which no act can clear.
const HALTING = new Set(["rate_limited", "unauthorized", "billing_required"]);
const ACCOUNT_PROBLEMS = new Set([
  "https://api.twitter.com/2/problems/oauth1-permissions",
  "https://api.twitter.com/2/problems/client-forbidden",
  "https://api.twitter.com/2/problems/unsupported-authentication",
]);

function halts(outcome) {
  return HALTING.has(outcome.code) || ACCOUNT_PROBLEMS.has(text(record(outcome.answer).type));
}

export function finalizeExecution(inputs) {
  const plan = record(inputs.execution_plan);
  if (plan.decision !== "ready") {
    return { twitter_execution: executionPacket(plan, plan.decision) };
  }
  const batch = record(plan.batch);
  const operation = record(inputs.provider_operation);
  const state = batch.segment
    ? publishedSegment(plan, batch, operation)
    : actKind(batch.kind)?.publish
      ? published(plan, batch, operation)
      : repeated(plan, batch, record(operation.result));
  const decision = state.nextActIndex === state.totalActCount
    ? "executed"
    : state.rateLimited
      ? "stopped"
      : "partial";
  const remainingCount = Math.max(0, state.totalActCount - state.nextActIndex);
  const completed = state.nextActIndex - state.startActIndex;
  return {
    twitter_execution: executionPacket(plan, decision, {
      results: state.results,
      next_act_index: state.nextActIndex,
      remaining_count: remainingCount,
      active_thread: state.activeThread,
      ledger_delta: progressEvent(plan, decision, state, array(batch.acts).length, text(inputs.account_id)),
      rate: state.rate,
      success_checkpoint: {
        milestone: decision === "executed" ? "plan_fully_executed" : "plan_partially_executed",
        description: `${completed} acts completed in this batch, ${remainingCount} remaining`,
      },
    }),
  };
}

function baseState(plan) {
  const startActIndex = nonNegativeInteger(plan.start_act_index) ?? 0;
  return {
    startActIndex,
    nextActIndex: startActIndex,
    totalActCount: nonNegativeInteger(plan.total_act_count) ?? 0,
    activeThread: null,
    results: [],
    failed: false,
    rateLimited: false,
    rate: rate(),
  };
}

// One published post: the operation settled, so the act is done.
function published(plan, batch, operation) {
  const state = baseState(plan);
  const act = record(batch.acts[0]);
  state.results.push(result(act, "done", postId(operation), null));
  state.nextActIndex += 1;
  return state;
}

// One thread segment: the thread stays active until its last segment.
function publishedSegment(plan, batch, operation) {
  const state = baseState(plan);
  const act = record(batch.acts[0]);
  const segment = record(batch.segment);
  const posted = nonNegativeInteger(segment.index) ?? 0;
  const count = nonNegativeInteger(segment.count) ?? 0;
  const id = postId(operation);
  if (!id) {
    // The next segment replies to this one, so without its id the thread
    // cannot go on; it stops here for the operator to look.
    state.failed = true;
    state.results.push(result(act, "failed", null, `segment ${posted + 1} of ${count} was confirmed without a post id`));
    return state;
  }
  state.results.push(result(act, "done", id, `segment ${posted + 1} of ${count} posted`));
  if (posted + 1 < count) {
    state.activeThread = {
      act_index: nonNegativeInteger(act.act_index) ?? state.startActIndex,
      next_segment_index: posted + 1,
      in_reply_to: id,
    };
  } else {
    state.nextActIndex += 1;
  }
  return state;
}

// A repeated act: one outcome per act, in order. The cursor passes every
// act the provider settled, applied or refused, and stops at the first it
// halted on, never received, or applied without stating the resulting state.
// An act stopped on is sent again by a later batch; it sets a state, so a
// second send changes nothing.
function repeated(plan, batch, operationResult) {
  const state = baseState(plan);
  const outcomes = array(operationResult.outcomes).map(record);
  for (const [index, act] of array(batch.acts).map(record).entries()) {
    const outcome = outcomes[index] ?? {};
    if (outcome.status === "applied") {
      const reached = reachedState(act.kind, outcome.acknowledgement);
      if (reached === null) {
        state.failed = true;
        state.results.push(result(act, "failed", act.target, "the provider's answer did not state the resulting state"));
        break;
      }
      if (reached === false) {
        state.failed = true;
        state.results.push(result(act, "failed", act.target, "the provider reported that the requested state was not reached"));
      } else {
        state.results.push(result(act, "done", act.target, null));
      }
      state.nextActIndex += 1;
      continue;
    }
    if (outcome.status === "refused" && !halts(outcome)) {
      const detail = providerDetail(outcome);
      if (alreadySatisfied(act.kind, detail)) {
        state.results.push(result(act, "done", act.target, "Target account is inactive; the requested not-following state is already satisfied."));
      } else {
        state.failed = true;
        state.results.push(result(act, "failed", act.target, detail));
      }
      state.nextActIndex += 1;
      continue;
    }
    if (outcome.status === "refused") {
      state.failed = true;
      state.rateLimited = outcome.code === "rate_limited";
      state.rate = rate(outcome);
      state.results.push(result(act, "failed", act.target, providerDetail(outcome)));
    }
    break;
  }
  return state;
}

// X answers an unfollow of a deactivated account with this refusal; the
// account is not followed, which is what the act asked for.
function alreadySatisfied(kind, detail) {
  return kind === "unfollow" && detail === "You cannot unfollow an account that is not active.";
}

function providerDetail(outcome) {
  const answer = record(outcome.answer);
  const first = array(answer.errors).map(record)[0] ?? {};
  return text(first.detail)
    || text(first.message)
    || text(answer.detail)
    || text(answer.title)
    || text(outcome.reason)
    || "the provider refused the act";
}

function postId(operation) {
  return text(record(record(operation.result).post).id) || text(operation.operation_id) || null;
}

function progressEvent(plan, decision, state, selectedCount, accountId) {
  return {
    schema: "twitter.execution.progress.v1",
    type: "twitter.execution.progress",
    plan_digest: text(plan.plan_digest),
    account_id: accountId,
    decision,
    next_act_index: state.nextActIndex,
    total_act_count: state.totalActCount,
    active_thread: state.activeThread,
    batch: {
      start_act_index: state.startActIndex,
      selected_act_count: selectedCount,
      completed_act_count: state.nextActIndex - state.startActIndex,
      failed: state.failed,
      rate_limited: state.rateLimited,
    },
  };
}

function executionPacket(plan, decision, overrides = {}) {
  return {
    decision,
    plan_digest: text(plan.plan_digest),
    principal: plan.principal ?? null,
    next_act_index: nonNegativeInteger(plan.next_act_index) ?? 0,
    total_act_count: nonNegativeInteger(plan.total_act_count) ?? 0,
    remaining_count: nonNegativeInteger(plan.remaining_count) ?? 0,
    active_thread: activeThread(plan.active_thread),
    results: [],
    ledger_delta: null,
    rate: rate(),
    blockers: array(plan.blockers),
    success_checkpoint: {
      milestone: decision === "noop" ? "plan_fully_executed" : "plan_refused",
      description: decision === "noop"
        ? "The durable cursor is already at the end of the plan."
        : "The execution contract was refused before provider work.",
    },
    ...overrides,
  };
}

function result(act, status, providerRef, detail) {
  return {
    act_id: String(act.act_id),
    kind: act.kind,
    consequence: act.consequence,
    status,
    provider_ref: providerRef || null,
    detail,
  };
}

function rate(outcome = {}) {
  const limited = outcome.code === "rate_limited";
  return {
    limited,
    remaining: limited ? 0 : -1,
    reset_at: limited && text(outcome.retry_at) ? text(outcome.retry_at) : null,
  };
}
