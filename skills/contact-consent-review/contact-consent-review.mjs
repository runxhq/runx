const WRITABLE_STATES = new Set(["re_permission", "suppress"]);

function decide(inputs) {
  const readback = object(inputs.contact_readback.result);
  const base = {
    key: inputs.key,
    expected_version: inputs.expected_version,
    observed_state: inputs.consent_evidence.state,
    version: integer(readback.version),
    observed_state_source: "supplied_evidence",
  };

  if (!sameKey(readback, base) || readback.operation !== "events.read") {
    return noWrite(base, "human_review", "ledger readback did not match the requested contact", "source_mismatch");
  }
  if (base.version !== base.expected_version) {
    return noWrite(
      base,
      "human_review",
      `ledger version ${base.version} does not match expected version ${base.expected_version}`,
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
  if (base.version > 0) {
    const latest = readback.events?.[0];
    const event = latest?.data;
    if (latest?.version !== base.version ||
        latest.type !== "list_hygiene.consent_transitioned" ||
        event?.type !== "list_hygiene.consent_transitioned" ||
        !WRITABLE_STATES.has(event.new_state)) {
      return noWrite(base, "human_review", "the ledger head is not an exact recognized consent transition", "unrecognized_ledger_state");
    }
    base.observed_state = event.new_state;
    base.observed_state_source = "native_record";
    if (consent.state !== event.new_state) {
      return noWrite(base, "human_review", "supplied consent state conflicts with the recorded ledger state", "conflicting_consent_state");
    }
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
      "supplied hard-bounce evidence requires suppression",
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

export function decideListHygiene(inputs) {
  const { decision_plan: plan } = decide(inputs);
  return {
    decision_plan: plan,
    list_hygiene_result: {
      schema: "runx/contact-consent-review.decision.v1",
      decision: plan.decision,
      observed_state: plan.observed_state,
      observed_state_source: plan.observed_state_source,
      observed_contact: inputs.contact_readback,
      escalation: plan.decision.state === "human_review"
        ? { lane: "human:list-hygiene-reviewer", reason_code: plan.reason_code, status: "required_before_any_write" }
        : null,
      downstream_send: {
        skill: null,
        status: "not_run",
        requirement: "The operator-selected mail capability must reread the current ledger decision and provider unsubscribe/bounce state at delivery; this review grants no send authority.",
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

// A store answer about this contact: its key in this skill's own consent
// stream, which the runtime places under whichever package runs it.
function sameKey(result, expected) {
  return typeof result.stream === "string" &&
    result.stream.endsWith("/consent") &&
    result.key === expected.key;
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function integer(value) {
  return Number.isInteger(value) && value >= 0 ? value : -1;
}
