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
  const processed = state.nextActIndex - state.startActIndex;
  const succeeded = state.results.slice(0, processed).filter((item) => item.status === "done").length;
  const failed = processed - succeeded;
  const counts = {
    processed: plan.counts.processed + processed,
    succeeded: plan.counts.succeeded + succeeded,
    failed: plan.counts.failed + failed,
  };
  const failure = state.results.slice(0, processed).findLast((item) => item.status === "failed");
  const lastFailure = failure ? {
    act_id: failure.act_id,
    reason: failure.detail,
    evidence_uri: text(operation.readback_uri),
  } : plan.last_failure;
  const decision = state.nextActIndex === state.totalActCount
    ? counts.failed > 0 ? "completed_with_failures" : "executed"
    : state.stop
      ? "stopped"
      : "partial";
  const remainingCount = Math.max(0, state.totalActCount - state.nextActIndex);
  return {
    twitter_execution: executionPacket(plan, decision, {
      results: state.results,
      next_act_index: state.nextActIndex,
      remaining_count: remainingCount,
      active_thread: state.activeThread,
      counts,
      last_failure: lastFailure,
      stop: state.stop,
      ledger_delta: progressEvent(plan, decision, state, array(batch.acts).length, text(inputs.account_id), counts, lastFailure, { processed, succeeded, failed }),
      rate: state.rate,
      success_checkpoint: {
        milestone: decision === "executed" ? "plan_fully_executed"
          : decision === "completed_with_failures" ? "plan_completed_with_failures" : "plan_partially_executed",
        description: `${processed} acts processed in this batch: ${succeeded} succeeded, ${failed} failed; ${remainingCount} remaining`,
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
    activeThread: activeThread(plan.active_thread),
    results: [],
    rateLimited: false,
    rate: rate(),
    stop: null,
  };
}

// One published post: the operation settled, so the act is done.
function published(plan, batch, operation) {
  const state = baseState(plan);
  const act = record(batch.acts[0]);
  const id = postId(operation);
  if (!id) {
    state.stop = stop("missing_post_id", "The published post has no confirmed X id; reconcile the native invocation before continuing.", "reconcile");
    state.results.push(result(act, "failed", null, state.stop.reason));
    return state;
  }
  state.results.push(result(act, "done", id, null));
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
    state.stop = stop("missing_post_id", "The thread segment has no confirmed X id; reconcile the native invocation before continuing.", "reconcile");
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
    state.activeThread = null;
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
        state.stop = stop("unknown_resulting_state", "The provider's answer did not state the resulting state; review the native evidence before retrying.", "review");
        state.results.push(result(act, "failed", act.target, "the provider's answer did not state the resulting state"));
        break;
      }
      if (reached === false) {
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
        state.results.push(result(act, "failed", act.target, detail));
      }
      state.nextActIndex += 1;
      continue;
    }
    if (outcome.status === "refused") {
      state.rateLimited = outcome.code === "rate_limited";
      state.rate = rate(outcome);
      state.stop = stop(text(outcome.code) || "provider_refused", providerDetail(outcome), state.rateLimited && state.rate.reset_at ? "wait" : "review", state.rate.reset_at);
      state.results.push(result(act, "failed", act.target, providerDetail(outcome)));
    } else {
      state.stop = stop("missing_outcome", "The batch has no settled outcome for the next act; reconcile the native invocation.", "reconcile");
      state.results.push(result(act, "failed", act.target, state.stop.reason));
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
  const id = text(record(record(operation.result).post).id);
  return /^[0-9]{1,19}$/.test(id) ? id : null;
}

function stop(code, reason, action, retryAt = null) {
  return { code, reason, action, retry_at: retryAt };
}

function progressEvent(plan, decision, state, selectedCount, accountId, counts, lastFailure, batchCounts) {
  return {
    schema: "runx/twitter.execution_progress.v1",
    type: "twitter.execution.progress",
    plan_digest: text(plan.plan_digest),
    account_id: accountId,
    decision,
    next_act_index: state.nextActIndex,
    total_act_count: state.totalActCount,
    active_thread: state.activeThread,
    counts,
    last_failure: lastFailure,
    stop: state.stop,
    batch: {
      start_act_index: state.startActIndex,
      selected_act_count: selectedCount,
      processed_act_count: batchCounts.processed,
      succeeded_act_count: batchCounts.succeeded,
      failed_act_count: batchCounts.failed,
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
    counts: plan.counts,
    last_failure: plan.last_failure,
    stop: plan.stop,
    results: [],
    ledger_delta: null,
    rate: rate(),
    blockers: array(plan.blockers),
    success_checkpoint: {
      milestone: decision === "noop" ? (plan.counts.failed > 0 ? "plan_completed_with_failures" : "plan_fully_executed") : "plan_refused",
      description: decision === "noop"
        ? "The durable cursor is already at the end of the plan."
        : "The execution contract was refused before provider work.",
    },
    ...overrides,
  };
}

function result(act, status, externalId, detail) {
  return {
    act_id: String(act.act_id),
    kind: act.kind,
    consequence: act.consequence,
    status,
    external_id: externalId || null,
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
