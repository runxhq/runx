import fs from "node:fs";

const WRITABLE_STATES = new Set(["re_permission", "suppress"]);

const operation = process.argv[2];
const raw = process.env.RUNX_INPUTS_PATH
  ? fs.readFileSync(process.env.RUNX_INPUTS_PATH, "utf8")
  : process.env.RUNX_INPUTS_JSON || "{}";
const inputs = JSON.parse(raw);

let result;
if (operation === "decide") result = decideListHygiene(inputs);
else if (operation === "finalize") result = finalizeListHygiene(inputs);
else throw new Error("expected decide or finalize operation");

process.stdout.write(`${JSON.stringify(result)}\n`);

function decideListHygiene(inputs) {
  const aggregateId = nonempty(inputs.aggregate_id, "aggregate_id");
  const expectedVersion = nonnegativeInteger(inputs.expected_version, "expected_version");
  const idempotencyKey = nonempty(inputs.idempotency_key, "idempotency_key");
  const dataSourceRef = nonempty(inputs.data_source_ref, "data_source_ref");
  const resource = nonempty(inputs.resource, "resource");
  const engagement = requiredObject(inputs.engagement_history, "engagement_history");
  const policy = requiredObject(inputs.bounce_policy, "bounce_policy");
  const consent = requiredObject(inputs.current_consent_state, "current_consent_state");
  nonnegativeInteger(engagement.opens_count, "engagement_history.opens_count");
  nonnegativeInteger(engagement.clicks_count, "engagement_history.clicks_count");
  nonnegativeInteger(engagement.hard_bounces, "engagement_history.hard_bounces");
  nonnegativeInteger(engagement.recency_days, "engagement_history.recency_days");
  if (!["suppress", "human_review"].includes(policy.hard_bounce_action)) {
    throw new Error("bounce_policy.hard_bounce_action must be suppress or human_review");
  }
  const decayThreshold = nonnegativeInteger(
    policy.decay_threshold_days,
    "bounce_policy.decay_threshold_days",
  );
  if (decayThreshold < 1 || decayThreshold > 3650) {
    throw new Error("bounce_policy.decay_threshold_days must be between 1 and 3650");
  }
  if (!["subscribed", "re_permission", "suppress", "unsubscribed"].includes(consent.state)) {
    throw new Error("current_consent_state.state is unsupported");
  }
  if (typeof consent.active_unsubscribe_marker !== "boolean") {
    throw new Error("current_consent_state.active_unsubscribe_marker must be boolean");
  }
  if (!["read", "missing", "unreadable", "stale", "ambiguous"].includes(consent.evidence_status)) {
    throw new Error("current_consent_state.evidence_status is unsupported");
  }
  nonnegativeInteger(consent.evidence_version, "current_consent_state.evidence_version");
  const contactReadback = object(inputs.contact_readback);
  const projection = object(contactReadback.projection);
  const projectionVersion = integer(projection.version, 0);
  const base = {
    aggregate_id: aggregateId,
    data_source_ref: dataSourceRef,
    resource,
    expected_version: expectedVersion,
    idempotency_key: idempotencyKey,
    projection_version: projectionVersion,
    projection_digest: text(contactReadback.projection_digest),
  };

  if (projectionVersion !== expectedVersion) {
    return planStop(
      base,
      `contact projection version ${projectionVersion} does not match expected_version ${expectedVersion}`,
      "stale_version",
    );
  }
  if (consent.evidence_status !== "read") {
    return planStop(
      base,
      `engagement evidence is ${consent.evidence_status}; fresh readable evidence is required`,
      consent.evidence_status === "ambiguous" ? "ambiguous_bounce_recovery" : "missing_or_unreadable_evidence",
    );
  }
  if (consent.evidence_version !== expectedVersion) {
    return planStop(
      base,
      `evidence_version ${consent.evidence_version} does not match expected_version ${expectedVersion}`,
      "stale_evidence",
    );
  }
  if (consent.active_unsubscribe_marker || consent.state === "unsubscribed") {
    return planStop(
      base,
      "an active unsubscribe marker forbids automated re-permission",
      "active_unsubscribe",
    );
  }
  if (engagement.hard_bounces > 0) {
    if (policy.hard_bounce_action !== "suppress") {
      return planStop(
        base,
        "hard-bounce recovery is ambiguous under the supplied policy",
        "ambiguous_bounce_recovery",
      );
    }
    return planWrite(base, consent.state, "suppress", "verified hard-bounce evidence requires suppression", {
      hard_bounces: engagement.hard_bounces,
      hard_bounce_action: policy.hard_bounce_action,
    });
  }
  if (engagement.recency_days > policy.decay_threshold_days) {
    return planWrite(
      base,
      consent.state,
      "re_permission",
      `recency_days ${engagement.recency_days} exceeds decay_threshold_days ${policy.decay_threshold_days}`,
      {
        recency_days: engagement.recency_days,
        decay_threshold_days: policy.decay_threshold_days,
        hard_bounces: engagement.hard_bounces,
      },
    );
  }
  return planStop(base, "no safe automated consent transition is required", "no_transition_required");
}

function finalizeListHygiene(inputs) {
  const plan = object(inputs.decision_plan);
  const readback = object(inputs.recorded_readback);
  const projection = object(readback.projection);
  const projectionDigest = text(readback.projection_digest);
  const projectionVersion = integer(projection.version, 0);
  const appendAllowed = plan.append_allowed === true;
  const expectedProjectionVersion = appendAllowed
    ? plan.expected_version + 1
    : plan.projection_version;

  if (projectionVersion !== expectedProjectionVersion) {
    throw new Error(
      `readback projection version ${projectionVersion} does not match expected ${expectedProjectionVersion}`,
    );
  }
  if (appendAllowed && projection.last_event_type !== plan.event.type) {
    throw new Error("readback did not observe the planned consent transition event");
  }
  if (appendAllowed && !WRITABLE_STATES.has(plan.decision.state)) {
    throw new Error("only re_permission or suppress may be recorded automatically");
  }

  return {
    list_hygiene_result: {
      decision: plan.decision,
      recorded_transition: {
        recorded: appendAllowed,
        state: appendAllowed ? plan.decision.state : "human_review",
        aggregate_id: plan.aggregate_id,
        data_source_ref: plan.data_source_ref,
        resource: plan.resource,
        idempotency_key: plan.idempotency_key,
        before_version: plan.projection_version,
        after_version: projectionVersion,
        event_type: appendAllowed ? plan.event.type : null,
        projection_digest: projectionDigest,
      },
      escalation: appendAllowed
        ? null
        : {
            lane: "human:list-hygiene-reviewer",
            reason_code: plan.reason_code,
            status: "required_before_any_write",
          },
      downstream_send: {
        skill: "send-as",
        status: "not_run",
        requirement: "read the recorded consent state at send time and refuse suppressed contacts",
      },
    },
  };
}

function planWrite(base, fromState, state, reason, evidence) {
  return {
    decision_plan: {
      ...base,
      decision: { state, reason },
      reason_code: state === "suppress" ? "hard_bounce" : "engagement_decay",
      append_allowed: true,
      event: {
        type: "list_hygiene.consent_transitioned",
        aggregate_id: base.aggregate_id,
        from_state: fromState,
        new_state: state,
        reason,
        evidence,
        evidence_version: base.expected_version,
        idempotency_key: base.idempotency_key,
      },
    },
  };
}

function planStop(base, reason, reasonCode) {
  return {
    decision_plan: {
      ...base,
      decision: { state: "human_review", reason },
      reason_code: reasonCode,
      append_allowed: false,
      event: null,
    },
  };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function integer(value, fallback) {
  return Number.isInteger(value) && value >= 0 ? value : fallback;
}

function text(value) {
  return typeof value === "string" ? value : "";
}

function requiredObject(value, name) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${name} must be an object`);
  }
  return value;
}

function nonempty(value, name) {
  if (typeof value !== "string" || value.trim().length === 0) {
    throw new Error(`${name} must be a non-empty string`);
  }
  return value;
}

function nonnegativeInteger(value, name) {
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(`${name} must be a non-negative integer`);
  }
  return value;
}
