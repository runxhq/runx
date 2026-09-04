const WRITABLE_STATES = new Set(["re_permission", "suppress"]);

export function decide(inputs) {
  const aggregateId = nonempty(inputs.aggregate_id);
  const expectedVersion = nonnegativeInteger(inputs.expected_version);
  const idempotencyKey = nonempty(inputs.idempotency_key);
  const engagement = record(inputs.engagement_history);
  const policy = record(inputs.bounce_policy);
  const consent = record(inputs.current_consent_state);
  const contactReadback = record(inputs.contact_readback);
  const projection = record(contactReadback.projection);
  const projectionVersion = durableVersion(contactReadback);
  const problems = [];

  if (contactReadback.operation !== "read_projection") {
    problems.push("contact_projection_unreadable");
  }
  if (!aggregateId || expectedVersion === null || !idempotencyKey) {
    problems.push("invalid_transition_identity");
  }
  if (projection.aggregate_id && projection.aggregate_id !== aggregateId) {
    problems.push("contact_projection_mismatch");
  }
  if (projectionVersion !== expectedVersion) {
    problems.push("stale_version");
  }

  for (const key of ["opens_count", "clicks_count", "hard_bounces", "recency_days"]) {
    if (nonnegativeInteger(engagement[key]) === null) problems.push(`invalid_${key}`);
  }
  const decayThreshold = nonnegativeInteger(policy.decay_threshold_days);
  if (decayThreshold === null || decayThreshold < 1) problems.push("invalid_decay_policy");
  if (!['suppress', 'human_review'].includes(policy.hard_bounce_action)) {
    problems.push("invalid_bounce_policy");
  }
  if (!['subscribed', 're_permission', 'suppress', 'unsubscribed'].includes(consent.state)) {
    problems.push("invalid_consent_state");
  }
  if (typeof consent.active_unsubscribe_marker !== "boolean") {
    problems.push("invalid_unsubscribe_evidence");
  }
  if (!['read', 'missing', 'unreadable', 'stale', 'ambiguous'].includes(consent.evidence_status)) {
    problems.push("invalid_evidence_status");
  } else if (consent.evidence_status !== "read") {
    problems.push(`${consent.evidence_status}_evidence`);
  }
  if (nonnegativeInteger(consent.evidence_version) !== expectedVersion) {
    problems.push("stale_evidence");
  }
  if (consent.aggregate_id && consent.aggregate_id !== aggregateId) {
    problems.push("consent_contact_mismatch");
  }

  const base = {
    aggregate_id: aggregateId,
    expected_version: expectedVersion,
    projection_version: projectionVersion,
    idempotency_key: idempotencyKey,
  };
  if (problems.length > 0) {
    return stopPlan(base, problems[0], problems);
  }
  if (consent.active_unsubscribe_marker || consent.state === "unsubscribed") {
    return stopPlan(base, "active_unsubscribe", ["active_unsubscribe"]);
  }

  if (engagement.hard_bounces > 0) {
    if (policy.hard_bounce_action !== "suppress") {
      return stopPlan(base, "ambiguous_bounce_recovery", ["ambiguous_bounce_recovery"]);
    }
    return writePlan(base, consent.state, "suppress", "verified_hard_bounce", {
      hard_bounces: engagement.hard_bounces,
      hard_bounce_action: policy.hard_bounce_action,
    });
  }
  if (engagement.recency_days > decayThreshold) {
    return writePlan(base, consent.state, "re_permission", "engagement_decay", {
      recency_days: engagement.recency_days,
      decay_threshold_days: decayThreshold,
      hard_bounces: engagement.hard_bounces,
    });
  }
  return stopPlan(base, "no_transition_required", ["no_transition_required"]);
}

export function finalize(inputs) {
  const plan = record(inputs.transition_plan);
  const appendResult = record(inputs.append_result);
  const readback = record(inputs.recorded_readback);
  const projection = record(readback.projection);
  const projectionVersion = durableVersion(readback);
  const shouldAppend = plan.should_append === true;
  const problems = [];

  if (readback.operation !== "read_projection") problems.push("readback_unreadable");
  if (projection.aggregate_id && projection.aggregate_id !== plan.aggregate_id) {
    problems.push("readback_contact_mismatch");
  }
  if (shouldAppend) {
    if (appendResult.operation !== "append_event") problems.push("append_not_observed");
    if (!['committed', 'idempotent'].includes(appendResult.status)) problems.push("append_not_committed");
    if (appendResult.after_version !== plan.expected_version + 1) problems.push("append_version_mismatch");
    if (projectionVersion !== plan.expected_version + 1) problems.push("readback_version_mismatch");
    if (projection.last_event_type !== record(plan.event).type) problems.push("readback_event_mismatch");
    if (!WRITABLE_STATES.has(record(plan.decision).state)) problems.push("unwritable_transition");
  } else if (projectionVersion !== plan.projection_version) {
    problems.push("evidence_changed_during_judgment");
  }

  if (problems.length > 0) {
    return {
      list_hygiene_result: {
        schema: "runx.list_hygiene_judge.v1",
        decision: {
          state: "stop",
          reason_code: "readback_mismatch",
          reasons: problems,
        },
        planned_decision: plan.decision ?? null,
        persistence: {
          append_status: shouldAppend ? "unverified" : "not_appended",
          before_version: plan.projection_version,
          after_version: projectionVersion,
          idempotency_key: plan.idempotency_key,
        },
        downstream_send: { status: "not_run" },
      },
    };
  }

  return {
    list_hygiene_result: {
      schema: "runx.list_hygiene_judge.v1",
      decision: plan.decision,
      planned_decision: plan.decision,
      persistence: {
        append_status: shouldAppend ? appendResult.status : "not_appended",
        before_version: plan.projection_version,
        after_version: projectionVersion,
        aggregate_id: plan.aggregate_id,
        idempotency_key: plan.idempotency_key,
        event_type: shouldAppend ? plan.event.type : null,
      },
      downstream_send: { status: "not_run" },
    },
  };
}

function writePlan(base, fromState, state, reasonCode, evidence) {
  const event = {
    type: "list_hygiene.consent_transitioned",
    payload: {
      aggregate_id: base.aggregate_id,
      from_state: fromState,
      new_state: state,
      reason_code: reasonCode,
      evidence,
      evidence_version: base.expected_version,
      idempotency_key: base.idempotency_key,
    },
  };
  return {
    transition_plan: {
      ...base,
      decision: { state, reason_code: reasonCode },
      should_append: true,
      event,
    },
  };
}

function stopPlan(base, reasonCode, reasons) {
  return {
    transition_plan: {
      ...base,
      decision: { state: "stop", reason_code: reasonCode, reasons },
      should_append: false,
      event: null,
    },
  };
}

function durableVersion(value) {
  const projection = record(value.projection);
  for (const candidate of [projection.version, value.after_version, value.current_version]) {
    if (Number.isInteger(candidate) && candidate >= 0) return candidate;
  }
  return 0;
}

function nonnegativeInteger(value) {
  return Number.isInteger(value) && value >= 0 ? value : null;
}

function nonempty(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}
