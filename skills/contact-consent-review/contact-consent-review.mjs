const WRITABLE_STATES = new Set(["re_permission", "suppress"]);

export function decideListHygiene(inputs) {
  const projectionResult = object(inputs.contact_readback);
  const projection = object(projectionResult.projection);
  const base = {
    aggregate_id: inputs.aggregate_id,
    data_source_uri: inputs.data_source_uri,
    resource: inputs.resource,
    expected_version: inputs.expected_version,
    idempotency_key: inputs.idempotency_key,
    observed_state: inputs.consent_evidence.state,
    projection_version: integer(projection.version),
    projection_digest: string(projectionResult.projection_digest),
  };

  if (!sameSource(projectionResult, base) || projectionResult.status !== "read") {
    return noWrite(base, "human_review", "ledger readback did not match the requested contact", "source_mismatch");
  }
  if (base.projection_version !== base.expected_version) {
    return noWrite(
      base,
      "human_review",
      `ledger version ${base.projection_version} does not match expected version ${base.expected_version}`,
      "stale_version",
    );
  }

  const consent = inputs.consent_evidence;
  if (consent.evidence_status !== "read") {
    return noWrite(
      base,
      "human_review",
      `consent evidence is ${consent.evidence_status}; fresh readable evidence is required`,
      consent.evidence_status === "ambiguous" ? "ambiguous_evidence" : "unusable_evidence",
    );
  }
  if (consent.evidence_version !== base.expected_version) {
    return noWrite(
      base,
      "human_review",
      `evidence version ${consent.evidence_version} does not match expected version ${base.expected_version}`,
      "stale_evidence",
    );
  }
  if (consent.active_unsubscribe_marker || consent.state === "unsubscribed") {
    return noWrite(base, "human_review", "an unsubscribe marker forbids automated re-permission", "active_unsubscribe");
  }
  if (consent.state === "suppress") {
    return noWrite(base, "suppress", "existing suppression remains in force", "already_suppressed");
  }

  const engagement = inputs.engagement_evidence;
  const policy = inputs.hygiene_policy;
  if (engagement.hard_bounces > 0) {
    if (policy.hard_bounce_action !== "suppress") {
      return noWrite(base, "human_review", "hard-bounce policy requires human review", "hard_bounce_review");
    }
    return write(
      base,
      consent.state,
      "suppress",
      "verified hard-bounce evidence requires suppression",
      { hard_bounces: engagement.hard_bounces, hard_bounce_action: policy.hard_bounce_action },
    );
  }
  if (consent.state === "re_permission") {
    return noWrite(base, "re_permission", "re-permission remains required", "already_re_permission");
  }
  if (engagement.recency_days > policy.decay_threshold_days) {
    return write(
      base,
      consent.state,
      "re_permission",
      `recency ${engagement.recency_days} days exceeds the ${policy.decay_threshold_days}-day threshold`,
      {
        recency_days: engagement.recency_days,
        decay_threshold_days: policy.decay_threshold_days,
        hard_bounces: engagement.hard_bounces,
      },
    );
  }
  return noWrite(base, "verify", "current evidence requires no consent transition", "current_evidence_clean");
}

export function finalizeListHygiene(inputs) {
  const plan = object(inputs.decision_plan);
  const readback = object(inputs.recorded_readback);
  const projection = object(readback.projection);
  if (readback.status !== "read" || !sameSource(readback, plan)) {
    throw new Error("ledger readback did not match the planned contact source");
  }

  const appended = plan.append_allowed === true;
  const writeResult = object(inputs.write_result);
  if (appended) {
    if (!WRITABLE_STATES.has(plan.decision.state)) {
      throw new Error("only re_permission or suppress may be recorded automatically");
    }
    if (!["committed", "idempotent_replay"].includes(writeResult.status)) {
      throw new Error("ledger append did not return a committed transition");
    }
    if (!sameSource(writeResult, plan)) {
      throw new Error("ledger append identity did not match the planned transition");
    }
    if (writeResult.before_version !== plan.expected_version || writeResult.after_version !== plan.expected_version + 1) {
      throw new Error("ledger append version did not match the planned transition");
    }
    if (
      projection.version !== writeResult.after_version ||
      projection.last_event_type !== plan.event.type ||
      projection.last_event_id !== writeResult.event_id ||
      projection.last_event_digest !== writeResult.event_digest ||
      readback.projection_digest !== writeResult.projection_digest
    ) {
      throw new Error("ledger readback did not prove the exact appended transition");
    }
  } else if (
    projection.version !== plan.projection_version ||
    readback.projection_digest !== plan.projection_digest
  ) {
    throw new Error("ledger changed while evaluating a no-write decision");
  }

  return {
    list_hygiene_result: {
      decision: plan.decision,
      observed_state: plan.observed_state,
      recorded_transition: {
        recorded: appended,
        state: appended ? plan.decision.state : null,
        aggregate_id: plan.aggregate_id,
        data_source_uri: plan.data_source_uri,
        resource: plan.resource,
        idempotency_key: appended ? plan.idempotency_key : null,
        write_status: appended ? writeResult.status : "not_attempted",
        before_version: plan.projection_version,
        after_version: projection.version,
        event_id: appended ? writeResult.event_id : null,
        event_digest: appended ? writeResult.event_digest : null,
        projection_digest: readback.projection_digest,
      },
      escalation: plan.decision.state === "human_review"
        ? { lane: "human:list-hygiene-reviewer", reason_code: plan.reason_code, status: "required_before_any_write" }
        : null,
      downstream_send: {
        skill: "message-send",
        status: "not_run",
        requirement: "read current consent evidence again and enforce it at delivery time",
      },
    },
  };
}

function write(base, fromState, state, reason, evidence) {
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
      },
    },
  };
}

function noWrite(base, state, reason, reasonCode) {
  return {
    decision_plan: {
      ...base,
      decision: { state, reason },
      reason_code: reasonCode,
      append_allowed: false,
      event: null,
    },
  };
}

function sameSource(result, expected) {
  return result.data_source_uri === expected.data_source_uri &&
    result.resource === expected.resource &&
    result.aggregate_id === expected.aggregate_id;
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function integer(value) {
  return Number.isInteger(value) && value >= 0 ? value : -1;
}

function string(value) {
  return typeof value === "string" ? value : "";
}
