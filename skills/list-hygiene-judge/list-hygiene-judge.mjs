const HARD_BOUNCE_EVENT = "contact.hard_bounce_observed";
const CONSENT_EVENT = "contact.consent_state_recorded";

export function decideTransition(inputs) {
  const readback = record(inputs.contact_readback);
  const projection = record(readback.projection);
  const engagement = record(inputs.engagement_history);
  const policy = record(inputs.bounce_policy);
  const consent = record(inputs.current_consent_state);
  const expectedVersion = integer(inputs.expected_version);
  const evidenceVersion = integer(consent.evidence_version);
  const actualVersion = integer(projection.version);
  const findings = [];

  if (readback.operation !== "read_projection" || readback.status !== "read") {
    findings.push(finding("contact_readback.unreadable", "The canonical contact projection was not read successfully."));
  }
  if (projection.aggregate_id !== inputs.aggregate_id) {
    findings.push(finding("contact_readback.wrong_contact", "The projection does not belong to the requested contact."));
  }
  if (actualVersion === null || expectedVersion === null || actualVersion !== expectedVersion) {
    findings.push(finding("contact_readback.version_mismatch", `Expected durable version ${show(expectedVersion)} but read ${show(actualVersion)}.`));
  }
  if (evidenceVersion === null || actualVersion === null || evidenceVersion !== actualVersion) {
    findings.push(finding("evidence.version_mismatch", "Consent evidence is not bound to the durable contact version."));
  }
  if (consent.evidence_status !== "read") {
    findings.push(finding("evidence.not_fresh", `Evidence status is ${show(consent.evidence_status)}; a fresh durable read is required.`));
  }
  if (consent.active_unsubscribe_marker === true || consent.state === "unsubscribed") {
    findings.push(finding("consent.unsubscribe_active", "An active unsubscribe marker requires human review and forbids re-permission."));
  }

  const hardBounces = nonNegativeInteger(engagement.hard_bounces);
  const recencyDays = nonNegativeInteger(engagement.recency_days);
  const opensCount = nonNegativeInteger(engagement.opens_count);
  const clicksCount = nonNegativeInteger(engagement.clicks_count);
  const threshold = positiveInteger(policy.decay_threshold_days);
  if ([hardBounces, recencyDays, opensCount, clicksCount].some((value) => value === null)) {
    findings.push(finding("engagement.invalid", "Engagement and bounce metrics must be non-negative integers."));
  }
  if (threshold === null || policy.hard_bounce_action !== "suppress") {
    findings.push(finding("bounce_policy.invalid", "Bounce policy must define a positive decay threshold and suppress hard bounces."));
  }
  if (hardBounces !== null && hardBounces > 0 && projection.last_event_type !== HARD_BOUNCE_EVENT) {
    findings.push(finding("hard_bounce.store_evidence_missing", "A positive hard-bounce count is not backed by the durable contact projection."));
  }
  if (hardBounces === 0 && consent.state === "suppressed") {
    findings.push(finding("bounce_recovery.ambiguous", "A suppressed contact without current hard-bounce evidence requires human recovery review."));
  }
  if (integer(projection.event_count) === 0) {
    findings.push(finding("contact_readback.empty", "The contact stream has no durable engagement or consent evidence."));
  }

  const sourceRead = {
    data_source_ref: text(inputs.data_source_ref),
    resource: text(inputs.resource),
    aggregate_id: text(inputs.aggregate_id),
    version: actualVersion ?? 0,
    event_count: integer(projection.event_count) ?? 0,
    last_event_type: nullableText(projection.last_event_type),
    projection_digest: nullableText(projection.projection_digest),
  };

  if (findings.length > 0) {
    return {
      decision_plan: {
        path: "stop",
        decision: { state: "human_review", reason: findings.map((item) => item.message).join(" ") },
        source_read: sourceRead,
        event: null,
        downstream_send: downstreamSend(),
        stop: { required: true, lane: "human:list-hygiene-reviewer", findings },
      },
    };
  }

  const state = hardBounces > 0
    ? "suppress"
    : recencyDays > threshold
      ? "re_permission"
      : "verify";
  const reason = state === "suppress"
    ? `Durable hard-bounce evidence supports suppression (${hardBounces} hard bounce(s)).`
    : state === "re_permission"
      ? `Engagement recency ${recencyDays} days exceeds the ${threshold}-day decay threshold.`
      : `Engagement recency ${recencyDays} days remains within the ${threshold}-day verification window.`;
  const idempotencyKey = text(inputs.idempotency_key);

  return {
    decision_plan: {
      path: "append",
      decision: { state, reason },
      source_read: sourceRead,
      event: {
        type: CONSENT_EVENT,
        new_state: state,
        reason,
        aggregate_id: text(inputs.aggregate_id),
        idempotency_key: idempotencyKey,
        based_on_version: actualVersion,
        source_projection_digest: nullableText(projection.projection_digest),
        hard_bounces: hardBounces,
        recency_days: recencyDays,
      },
      downstream_send: downstreamSend(),
      stop: null,
    },
  };
}

export function finalizeResult(inputs) {
  const plan = record(inputs.decision_plan);
  if (plan.path === "stop") {
    return finalizeStop(inputs);
  }

  const readback = record(inputs.recorded_readback);
  const projection = record(readback.projection);
  const expectedAfter = (integer(record(plan.source_read).version) ?? -1) + 1;
  const readbackVersion = integer(projection.version);
  const verified = plan.path === "append"
    && readback.operation === "read_projection"
    && readback.status === "read"
    && readbackVersion === expectedAfter
    && projection.last_event_type === CONSENT_EVENT;

  if (!verified) {
    return resultPacket({
      decision: { state: "human_review", reason: "The consent transition could not be verified by durable readback." },
      source_read: record(plan.source_read),
      recorded_transition: {
        recorded: false,
        status: "unverified_after_append",
        state: text(record(plan.decision).state),
        aggregate_id: text(record(plan.source_read).aggregate_id),
        idempotency_key: text(record(plan.event).idempotency_key),
        after_version: readbackVersion,
        projection_digest: nullableText(projection.projection_digest),
      },
      downstream_send: downstreamSend(),
      escalation: {
        required: true,
        lane: "human:list-hygiene-reviewer",
        reason: "Inspect the append receipt and current contact projection before any send.",
      },
      write_performed: true,
    });
  }

  return resultPacket({
    decision: record(plan.decision),
    source_read: record(plan.source_read),
    recorded_transition: {
      recorded: true,
      status: "committed",
      state: text(record(plan.decision).state),
      aggregate_id: text(record(plan.source_read).aggregate_id),
      idempotency_key: text(record(plan.event).idempotency_key),
      after_version: readbackVersion,
      projection_digest: nullableText(projection.projection_digest),
    },
    downstream_send: downstreamSend(),
    escalation: null,
    write_performed: true,
  });
}

export function finalizeStop(inputs) {
  const plan = record(inputs.decision_plan);
  const stop = record(plan.stop);
  return resultPacket({
    decision: record(plan.decision),
    source_read: record(plan.source_read),
    recorded_transition: null,
    downstream_send: downstreamSend(),
    escalation: {
      required: true,
      lane: text(stop.lane) || "human:list-hygiene-reviewer",
      reason: text(record(plan.decision).reason) || "Human review is required before any write.",
    },
    write_performed: false,
  });
}

function resultPacket(body) {
  return {
    list_hygiene_result: {
      schema: "runx.list_hygiene_result.v1",
      ...body,
      send_performed: false,
    },
  };
}

function downstreamSend() {
  return {
    skill: "send-as",
    dispatch_mode: "by_name",
    status: "not_run",
    must_read_recorded_state: true,
  };
}

function finding(code, message) {
  return { code, message };
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" && value.trim() ? value.trim() : "";
}

function nullableText(value) {
  const valueText = text(value);
  return valueText || null;
}

function integer(value) {
  return Number.isInteger(value) && Number.isSafeInteger(value) ? value : null;
}

function nonNegativeInteger(value) {
  const parsed = integer(value);
  return parsed !== null && parsed >= 0 ? parsed : null;
}

function positiveInteger(value) {
  const parsed = integer(value);
  return parsed !== null && parsed > 0 ? parsed : null;
}

function show(value) {
  return value === null || value === undefined || value === "" ? "unknown" : String(value);
}
