import { readFileSync } from "node:fs";

// list-hygiene-judge: deterministic consent-state judgment over a sealed data-store read.
// The judgment decides verify / suppress / re_permission / stop; a durable transition is
// recorded only by a compare-and-set append_event on the contact stream. This skill never
// sends: the recorded consent state is honored downstream by send-as, a separate governed run.

const SCHEMA = "runx.list_hygiene_result.v1";
const HUMAN_LANE = "human_list_hygiene_review";

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function toNumber(value, fallback) {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function storeVersionOf(readResult) {
  const r = record(readResult);
  for (const key of ["after_version", "before_version", "version"]) {
    if (typeof r[key] === "number" && Number.isFinite(r[key])) return r[key];
  }
  return null;
}

function hasUnsubscribeMarker(currentConsentState, readResult) {
  if (typeof currentConsentState === "string" &&
      currentConsentState.toLowerCase() === "unsubscribed") return true;
  const events = Array.isArray(record(readResult).events) ? readResult.events : [];
  return events.some((e) => {
    const t = String(record(e).event_type ?? record(e).type ?? "");
    return t.toLowerCase().includes("unsubscribe");
  });
}

function describeEvidence(readResult, engagement) {
  const r = record(readResult);
  const e = record(engagement);
  return {
    store_read_status: Object.keys(r).length ? String(r.status ?? "read") : "missing",
    store_version: storeVersionOf(readResult),
    projection_digest: typeof r.projection_digest === "string" ? r.projection_digest : null,
    result_digest: typeof r.result_digest === "string" ? r.result_digest : null,
    input_metrics: {
      opens_count: toNumber(e.opens_count, null),
      clicks_count: toNumber(e.clicks_count, null),
      hard_bounces: toNumber(e.hard_bounces, null),
      recency_days: toNumber(e.recency_days, null),
    },
  };
}

export function judgeContact(inputs) {
  const engagement = record(inputs.engagement_history);
  const policy = record(inputs.bounce_policy);
  const consentState = inputs.current_consent_state;
  const aggregateId = inputs.aggregate_id;
  const idempotencyKey = inputs.idempotency_key;
  const expectedVersion = toNumber(inputs.expected_version, null);
  const readResult = inputs.store_read;

  const stop = (reason) => ({
    judgment: {
      decision: { state: "stop", reason },
      escalation: { required: true, lane: HUMAN_LANE, reason },
      write: { should_append: false },
      event: null,
      evidence_basis: describeEvidence(readResult, engagement),
    },
  });

  // 1. The sealed store read is the evidence floor: no readable read, no judgment.
  if (!readResult || typeof readResult !== "object" || readResult.status === "error") {
    return stop("engagement evidence from the data-store read is missing or unreadable; no append was emitted");
  }

  // 2. Typed engagement evidence must be present and well-formed; metrics are never invented.
  if (!Number.isFinite(engagement.hard_bounces) || !Number.isFinite(engagement.recency_days)) {
    return stop("engagement_history evidence is missing or malformed; no append was emitted");
  }
  const opens = toNumber(engagement.opens_count, 0);
  const clicks = toNumber(engagement.clicks_count, 0);
  const hardBounces = engagement.hard_bounces;
  const recencyDays = engagement.recency_days;
  const decayThreshold = toNumber(policy.decay_threshold_days, null);
  if (decayThreshold === null || typeof policy.hard_bounce_action !== "string") {
    return stop("bounce_policy is missing hard_bounce_action or decay_threshold_days; no append was emitted");
  }

  // 3. Optimistic-concurrency check happens BEFORE any decision to write:
  //    a stale expected_version means the caller judged from an old projection.
  const storeVersion = storeVersionOf(readResult);
  if (expectedVersion === null || (storeVersion !== null && storeVersion !== expectedVersion)) {
    return stop(
      `stale expected_version: caller expected ${expectedVersion} but the contact stream reads version ${storeVersion}; no append was emitted`
    );
  }

  // 4. An active unsubscribe marker is never overridden by this judgment.
  const unsubscribed = hasUnsubscribeMarker(consentState, readResult);

  // 5. Hard-bounce path: suppression is decided only on hard-bounce evidence, and
  //    ambiguous bounce recovery escalates to the human lane instead of writing.
  if (hardBounces > 0) {
    const recentActivity = (opens + clicks) > 0 && recencyDays <= decayThreshold;
    if (recentActivity) {
      return stop(
        "ambiguous bounce recovery: hard bounces recorded but recent opens/clicks contradict a dead address; escalated, no append was emitted"
      );
    }
    if (policy.hard_bounce_action === "suppress") {
      return {
        judgment: {
          decision: {
            state: "suppress",
            reason: `hard_bounces=${hardBounces} with bounce_policy.hard_bounce_action=suppress and no contradicting recovery event in the sealed stream read`,
          },
          escalation: { required: false, lane: "none", reason: "hard-bounce evidence and policy authorize suppression" },
          write: { should_append: true },
          event: {
            type: "consent.suppressed",
            effect_family: "consent",
            operation: "suppress",
            payload: {
              aggregate_id: aggregateId,
              from_state: consentState ?? null,
              to_state: "suppressed",
              hard_bounces: hardBounces,
              decided_by: "list-hygiene-judge",
              idempotency_key: idempotencyKey ?? null,
            },
          },
          evidence_basis: describeEvidence(readResult, engagement),
        },
      };
    }
    return stop(
      `hard bounces present but bounce_policy.hard_bounce_action=${policy.hard_bounce_action} does not authorize suppression; escalated, no append was emitted`
    );
  }

  // 6. Decay path: re-permission only without an unsubscribe marker.
  if (recencyDays > decayThreshold) {
    if (unsubscribed) {
      return stop(
        "contact carries an active unsubscribe marker; re-permission is refused and the case escalates to the human approval lane; no append was emitted"
      );
    }
    return {
      judgment: {
        decision: {
          state: "re_permission",
          reason: `recency_days=${recencyDays} exceeds decay_threshold_days=${decayThreshold} with no unsubscribe marker in the input state or the sealed stream read`,
        },
        escalation: { required: false, lane: "none", reason: "decay threshold exceeded with clean consent evidence" },
        write: { should_append: true },
        event: {
          type: "consent.re_permission_requested",
          effect_family: "consent",
          operation: "re_permission",
          payload: {
            aggregate_id: aggregateId,
            from_state: consentState ?? null,
            to_state: "re_permission_pending",
            recency_days: recencyDays,
            decided_by: "list-hygiene-judge",
            idempotency_key: idempotencyKey ?? null,
          },
        },
        evidence_basis: describeEvidence(readResult, engagement),
      },
    };
  }

  // 7. Engaged and clean: verify only, no durable consent-state transition, no append.
  return {
    judgment: {
      decision: {
        state: "verify",
        reason: `contact is inside decay_threshold_days=${decayThreshold} with no hard bounces; no durable consent-state transition is warranted`,
      },
      escalation: { required: false, lane: "none", reason: "no transition to record" },
      write: { should_append: false },
      event: null,
      evidence_basis: describeEvidence(readResult, engagement),
    },
  };
}

function baseResult(inputs, judgment) {
  return {
    schema: SCHEMA,
    decision: judgment.decision,
    escalation: judgment.escalation,
    contact: {
      aggregate_id: String(inputs.aggregate_id ?? ""),
      resource: String(inputs.resource ?? ""),
      data_source_ref: String(inputs.data_source_ref ?? ""),
      current_consent_state: inputs.current_consent_state ?? null,
    },
    evidence: {
      basis: judgment.evidence_basis,
      evidence_digest: typeof inputs.evidence_digest === "string" ? inputs.evidence_digest : null,
    },
    downstream_send_performed: false,
  };
}

export function finalizeCommitted(inputs) {
  const judgment = record(record(inputs.judgment).judgment ?? inputs.judgment);
  const appended = record(inputs.append_result);
  const readback = record(inputs.readback);
  if (appended.operation !== "append_event" || typeof appended.after_version !== "number") {
    throw new Error("durable consent-transition append evidence is missing");
  }
  const result = baseResult(inputs, judgment);
  result.persistence = {
    append_status: "committed",
    aggregate_id: String(appended.aggregate_id ?? inputs.aggregate_id ?? ""),
    idempotency_key: String(inputs.idempotency_key ?? ""),
    expected_version: toNumber(inputs.expected_version, null),
    after_version: appended.after_version,
    event_ref: typeof appended.event_ref === "string" ? appended.event_ref : null,
  };
  result.recorded_transition = {
    read_back: true,
    store_version: storeVersionOf(readback),
    projection_digest: typeof readback.projection_digest === "string" ? readback.projection_digest : null,
    latest_events: Array.isArray(readback.events) ? readback.events.slice(-3) : [],
  };
  return { list_hygiene_result: result };
}

export function finalizeSkipped(inputs) {
  const judgment = record(record(inputs.judgment).judgment ?? inputs.judgment);
  const result = baseResult(inputs, judgment);
  result.persistence = {
    append_status: "skipped",
    aggregate_id: String(inputs.aggregate_id ?? ""),
    idempotency_key: String(inputs.idempotency_key ?? ""),
    expected_version: toNumber(inputs.expected_version, null),
    after_version: null,
    event_ref: null,
  };
  result.recorded_transition = null;
  return { list_hygiene_result: result };
}

function readCliInputs() {
  if (process.env.RUNX_INPUTS_PATH) {
    return JSON.parse(readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"));
  }
  return JSON.parse(process.env.RUNX_INPUTS_JSON || "{}");
}

function runCli() {
  const operation = process.argv[2];
  const inputs = readCliInputs();
  if (operation === "judge") return judgeContact(inputs);
  if (operation === "finalize-committed") return finalizeCommitted(inputs);
  if (operation === "finalize-skipped") return finalizeSkipped(inputs);
  throw new Error("operation must be judge, finalize-committed, or finalize-skipped");
}

if (process.argv[1]?.endsWith("list-hygiene-judge.mjs")) {
  try {
    process.stdout.write(`${JSON.stringify(runCli())}
`);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`list-hygiene-judge failed: ${message}
`);
    process.exitCode = 1;
  }
}
