export function decide(inputs) {
  const projectionResult = object(inputs.projection_result);
  const projection = object(projectionResult.projection);
  const suppliedHistory = object(inputs.engagement_history);
  const suppliedPolicy = object(inputs.bounce_policy);
  const current = object(inputs.current_consent_state);
  const errors = [];

  const storedHistory = object(projection.engagement_history);
  const storedPolicy = object(projection.bounce_policy);
  const storedConsent = object(projection.current_consent_state);
  if (projectionResult.operation !== "read_projection") errors.push("data-store did not return a projection");
  if (!projection.aggregate_id) errors.push("contact projection is missing aggregate_id");
  if (!storedHistory.opens_count && storedHistory.opens_count !== 0) errors.push("opens_count is unreadable");
  if (!storedHistory.clicks_count && storedHistory.clicks_count !== 0) errors.push("clicks_count is unreadable");
  if (!storedHistory.hard_bounces && storedHistory.hard_bounces !== 0) errors.push("hard_bounces is unreadable");
  if (!Number.isFinite(storedHistory.recency_days)) errors.push("recency_days is unreadable");
  if (!Number.isFinite(storedPolicy.decay_threshold_days)) errors.push("decay_threshold_days is unreadable");
  if (!storedPolicy.hard_bounce_action) errors.push("hard_bounce_action is unreadable");
  if (!storedConsent.state) errors.push("current consent state is unreadable");
  if (projection.aggregate_id !== inputs.aggregate_id) errors.push("projection aggregate_id does not match input");
  if (!sameObject(suppliedHistory, storedHistory, ["opens_count", "clicks_count", "hard_bounces", "recency_days"])) errors.push("supplied engagement evidence differs from data-store projection");
  if (!sameObject(suppliedPolicy, storedPolicy, ["hard_bounce_action", "decay_threshold_days"])) errors.push("supplied bounce policy differs from data-store projection");
  if (current.state && storedConsent.state !== current.state) errors.push("supplied consent state differs from data-store projection");

  const storedVersion = Number(projection.version);
  if (Number.isFinite(storedVersion) && storedVersion !== Number(inputs.expected_version)) errors.push("stale expected_version; no write permitted");

  let state = "unchanged";
  let reason = "Consent state remains unchanged.";
  let append = false;
  if (errors.length === 0) {
    const unsubscribe = storedConsent.unsubscribe_active === true || storedConsent.state === "unsubscribed";
    const hardBounce = Number(storedHistory.hard_bounces) > 0;
    if (hardBounce) {
      state = "suppress";
      reason = "Hard-bounce evidence requires suppression.";
      append = true;
    } else if (unsubscribe) {
      state = "human_review";
      reason = "Active unsubscribe marker blocks re-permission and requires human review.";
    } else if (Number(storedHistory.recency_days) > Number(storedPolicy.decay_threshold_days)) {
      state = "re_permission";
      reason = "Engagement is older than the configured decay threshold and no unsubscribe marker is active.";
      append = true;
    }
  } else {
    state = "human_review";
    reason = errors.join("; ");
  }

  const decision = { state, reason };
  const event = append ? {
    type: "contact.consent_state_changed",
    payload: {
      aggregate_id: inputs.aggregate_id,
      state,
      reason,
      idempotency_key: inputs.idempotency_key,
      source: "list-hygiene-judge",
    },
  } : null;
  return { decision, append, event, evidence: { projection, errors } };
}

export function finalize(inputs) {
  const decision = object(inputs.decision);
  const append = object(inputs.append_repermission ?? inputs.append_suppress);
  const projection = object(inputs.projection_result);
  const recorded = append.operation === "append_event" ? {
    status: append.status ?? "committed",
    aggregate_id: append.aggregate_id,
    after_version: append.after_version,
    idempotency_key: append.idempotency_key ?? inputs.idempotency_key,
  } : null;
  return {
    judge_result: {
      schema: "runx.list_hygiene_judge.v1",
      decision,
      transition: recorded,
      recorded_projection: projection.projection ?? null,
      append_emitted: Boolean(recorded),
      idempotency_key: inputs.idempotency_key,
    },
  };
}

function object(value) { return value && typeof value === "object" && !Array.isArray(value) ? value : {}; }
function sameObject(a, b, keys) { return keys.every((key) => a[key] === b[key]); }
