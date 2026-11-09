import { array, record, text } from "./twitter-execution-values.mjs";

// Immutable plan memory lives in this skill's own plans stream, and execution
// progress in its execution stream. The runtime owns storage, integrity and
// who may write them; these functions interpret X work.
export function resolvePlan(inputs) {
  const stored = record(inputs.saved_plan);
  const rows = array(stored.events);
  const saved = record(record(rows[0]).data);
  const supplied = record(inputs.plan_json);
  const present = Object.keys(supplied).length > 0;
  if (stored.version > 0 && (stored.version !== 1 || rows.length !== 1 || record(rows[0]).type !== "twitter.plan" || saved.type !== "twitter.plan" || saved.plan_digest !== inputs.plan_digest)) {
    throw new Error("saved X plan is inconsistent; inspect the native store result");
  }
  return {
    resolved_plan: {
      plan: present ? supplied : record(saved.plan),
      remember: stored.version === 0 && present,
    },
  };
}

export function executionStatus(inputs) {
  const plan = record(inputs.execution_plan);
  const original = record(inputs.plan);
  const ledger = record(inputs.execution_ledger);
  const event = record(record(array(ledger.events)[0]).data);
  const hasPlan = Object.keys(original).length > 0;
  const stop = plan.stop ?? null;
  const decision = !hasPlan ? "needs_plan"
    : plan.decision === "noop" ? (plan.counts.failed > 0 ? "complete_with_failures" : "complete")
    : plan.decision !== "ready" ? "refused"
    : stop ? (stop.action === "wait" ? "waiting" : "stopped") : "ready";
  return {
    twitter_status: {
      decision,
      plan_digest: inputs.plan_digest,
      objective: text(original.objective) || null,
      principal: text(original.principal) || null,
      next_act_index: hasPlan ? plan.next_act_index : null,
      total_act_count: hasPlan ? plan.total_act_count : null,
      remaining_count: hasPlan ? plan.remaining_count : null,
      thread_reply_to: plan.active_thread?.in_reply_to ?? null,
      counts: plan.counts,
      last_failure: plan.last_failure,
      last_batch: event.batch ?? null,
      stop,
      retry_at: stop?.retry_at ?? null,
      next_batch: plan.batch ? {
        kind: plan.batch.kind,
        act_ids: array(plan.batch.acts).map((act) => act.act_id),
        thread_segment_index: plan.batch.segment?.index ?? null,
        thread_segment_count: plan.batch.segment?.count ?? null,
      } : null,
      next_input: decision === "ready" || decision === "waiting" ? {
        plan_digest: inputs.plan_digest,
        max_acts: inputs.max_acts,
      } : null,
      blockers: hasPlan ? array(plan.blockers) : ["Supply the exact plan and native digest to execute once; no saved plan is available in this store."],
    },
  };
}

export function recentExecutions(inputs) {
  const result = record(inputs.heads);
  return {
    twitter_recent: {
      items: array(result.keys).map((listed) => {
        const event = record(record(listed.last_event).data);
        return {
          plan_digest: listed.key,
          account_id: event.account_id ?? null,
          decision: event.decision,
          next_act_index: event.next_act_index,
          total_act_count: event.total_act_count,
          counts: event.counts,
          last_failure: event.last_failure,
          stop: event.stop,
          retry_at: event.stop?.retry_at ?? null,
        };
      }),
      next_cursor: result.next_cursor ?? null,
    },
  };
}
