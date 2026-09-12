import crypto from "node:crypto";
import fs from "node:fs";

const REQUIRED_ENGAGEMENT_FIELDS = ["opens_count", "clicks_count", "hard_bounces", "recency_days"];
const HARNESS_CASE_NAMES = [
  "sealed_decay_re_permission",
  "sealed_hard_bounce_suppress",
  "stop_missing_or_stale_evidence",
];

const inputs = readInputs();
const resource = stringValue(inputs.resource) ?? "contacts";
const aggregateId = stringValue(inputs.aggregate_id);
const expectedVersion = numberValue(inputs.expected_version);
const idempotencyKey = stringValue(inputs.idempotency_key);
const engagementHistory = maybeObject(inputs.engagement_history);
const bouncePolicy = maybeObject(inputs.bounce_policy);
const currentConsentState = stringValue(inputs.current_consent_state);
// The whole data_operation_result is passed in, not just .data.projection: a
// successful read of a contact that has no events yet returns an EMPTY
// projection, and that is readable state, not absent state. Distinguishing the
// two is what makes "refuses to decide on unread state" enforceable.
const contactRead = maybeObject(inputs.contact_read) ?? maybeObject(inputs.contact_projection);
const dataSourceRef = stringValue(inputs.data_source_ref);

if (!aggregateId) fail("aggregate_id is required");
if (expectedVersion === undefined) fail("expected_version is required");
if (!idempotencyKey) fail("idempotency_key is required");

const projection = summarizeProjection(contactRead);

// ---------------------------------------------------------------------------
// Evidence gate. Every metric this skill judges on must be READ, never assumed:
// the contact projection must have been read back from data-store, and the
// engagement counters must be present as finite numbers. A missing counter is
// not a zero.
// ---------------------------------------------------------------------------
const stopReasons = [];
const missingEngagementFields = REQUIRED_ENGAGEMENT_FIELDS.filter(
  (field) => numberValue(engagementHistory?.[field]) === undefined,
);

if (!engagementHistory) {
  stopReasons.push("engagement_history is missing; refusing to judge list hygiene without read engagement evidence");
} else if (missingEngagementFields.length > 0) {
  stopReasons.push(
    `engagement_history is unreadable: missing numeric ${missingEngagementFields.join(", ")}; a missing counter is not a zero`,
  );
}

if (!projection.readable) {
  stopReasons.push(
    `contact projection for ${aggregateId} was not readable from data-store; refusing to decide a consent transition on unread state`,
  );
}

if (!bouncePolicy) {
  stopReasons.push("bounce_policy is missing; decay_threshold_days and hard_bounce_action must be declared by the caller");
}

const decayThresholdDays = numberValue(bouncePolicy?.decay_threshold_days);
const hardBounceAction = stringValue(bouncePolicy?.hard_bounce_action);
if (bouncePolicy && decayThresholdDays === undefined) {
  stopReasons.push("bounce_policy.decay_threshold_days is missing or not a number");
}
if (bouncePolicy && !hardBounceAction) {
  stopReasons.push("bounce_policy.hard_bounce_action is missing");
}

// The compare-and-set precondition is evaluated after the decision, because
// "the stream moved" and "the stream moved BECAUSE OF THIS EXACT TRANSITION"
// are different facts and only the first one is staleness. See the version gate
// below.

const hardBounces = numberValue(engagementHistory?.hard_bounces);
const recencyDays = numberValue(engagementHistory?.recency_days);
const opensCount = numberValue(engagementHistory?.opens_count);
const clicksCount = numberValue(engagementHistory?.clicks_count);

// An unsubscribe marker is authoritative and is read from the store projection
// first; the caller-supplied consent state is only a corroborating signal.
const unsubscribeMarkerActive =
  projection.unsubscribed === true || currentConsentState === "unsubscribed";

// Ambiguous bounce recovery: the contact hard-bounced AND has since engaged.
// Suppressing would drop a live human; re-permissioning would ignore a real
// delivery failure. Neither is decidable from the evidence, so neither is taken.
const engagedSinceBounce =
  hardBounces !== undefined &&
  hardBounces > 0 &&
  recencyDays !== undefined &&
  decayThresholdDays !== undefined &&
  recencyDays <= decayThresholdDays &&
  ((opensCount ?? 0) > 0 || (clicksCount ?? 0) > 0);

let decisionState = "no_change";
let decisionReason = "";
let escalate = false;

if (stopReasons.length > 0) {
  decisionState = "stop";
  decisionReason = stopReasons.join("; ");
  escalate = stopReasons.some((reason) => reason.startsWith("stale expected_version"));
} else if (engagedSinceBounce) {
  decisionState = "stop";
  escalate = true;
  decisionReason =
    `ambiguous bounce recovery: hard_bounces=${hardBounces} but the contact engaged ${recencyDays} days ago ` +
    `(opens=${opensCount}, clicks=${clicksCount}) inside the ${decayThresholdDays}-day decay window; escalating to human approval instead of writing`;
} else if (hardBounces !== undefined && hardBounces > 0) {
  decisionState = "suppress";
  decisionReason =
    `hard_bounces=${hardBounces} read from ${resource}/${aggregateId}; bounce_policy.hard_bounce_action=${hardBounceAction} ` +
    `so the contact transitions to suppress. send-as reads this recorded state at send time and will gate delivery.`;
} else if (unsubscribeMarkerActive) {
  decisionState = "stop";
  escalate = true;
  decisionReason =
    `contact carries an active unsubscribe marker (projection.unsubscribed=${projection.unsubscribed}, ` +
    `current_consent_state=${currentConsentState ?? "null"}); refusing to re-permission and escalating to human approval`;
} else if (recencyDays !== undefined && recencyDays > decayThresholdDays) {
  decisionState = "re_permission";
  decisionReason =
    `recency_days=${recencyDays} exceeds bounce_policy.decay_threshold_days=${decayThresholdDays} with no active ` +
    `unsubscribe marker and hard_bounces=${hardBounces}; the contact transitions to re_permission before any further send.`;
} else {
  decisionState = "no_change";
  decisionReason =
    `recency_days=${recencyDays} is within the ${decayThresholdDays}-day decay threshold and hard_bounces=${hardBounces}; ` +
    `no consent-state transition is warranted, so no event is appended.`;
}

// ---------------------------------------------------------------------------
// Version gate. A stream that sits exactly one event ahead of what the caller
// read, whose newest event is the very transition this judgment would record,
// is not stale state -- it is a RETRY of our own write. The store's append is
// keyed by idempotency_key and returns the recorded version instead of
// double-applying, so the honest thing is to stand by the same decision and let
// the append replay. Any other version drift is genuine staleness: our read is
// behind the stream and an append would apply against state we never saw.
// ---------------------------------------------------------------------------
let writes = decisionState === "suppress" || decisionState === "re_permission";
const versionDelta = projection.readable ? projection.version - expectedVersion : 0;
const idempotentReplay =
  writes && versionDelta === 1 && projection.last_event_type === `contact.consent_${decisionState}`;
const staleVersion = projection.readable && versionDelta !== 0 && !idempotentReplay;

if (staleVersion) {
  decisionState = "stop";
  writes = false;
  escalate = true;
  decisionReason =
    `stale expected_version: caller passed ${expectedVersion} but the contact stream is at version ${projection.version} ` +
    `(last event ${projection.last_event_type ?? "none"}); refusing to write against state we did not read`;
}

const newVersion = writes ? (idempotentReplay ? projection.version : expectedVersion + 1) : projection.version;
const transitionId = writes
  ? `transition_${sha256(`${aggregateId}:${idempotencyKey}:${decisionState}`).slice(0, 16)}`
  : null;

const decision = {
  state: decisionState,
  reason: decisionReason,
  writes,
};

const recordedTransition = writes
  ? {
      transition_id: transitionId,
      aggregate_id: aggregateId,
      resource,
      from_state: currentConsentState ?? projection.consent_state ?? "unknown",
      to_state: decisionState,
      expected_version: expectedVersion,
      new_version: newVersion,
      idempotency_key: idempotencyKey,
    }
  : null;

const observations = {
  consent_state_verdict: decisionState,
  consent_state_reason: decisionReason,
  new_state: writes
    ? {
        state: decisionState,
        aggregate_id: aggregateId,
        idempotency_key: idempotencyKey,
        expected_version: expectedVersion,
        new_version: newVersion,
        transition_id: transitionId,
      }
    : null,
  suppress_reason:
    decisionState === "suppress"
      ? {
          reason: decisionReason,
          hard_bounces: hardBounces,
          hard_bounce_action: hardBounceAction,
          data_store_resource_ref: `${dataSourceRef ?? "data-store"}#${resource}/${aggregateId}`,
        }
      : null,
  stop_reason: writes ? null : decisionReason,
  no_append_emitted: !writes,
  escalated_to_human_approval: escalate,
  engagement_evidence_read: {
    opens_count: opensCount ?? null,
    clicks_count: clicksCount ?? null,
    hard_bounces: hardBounces ?? null,
    recency_days: recencyDays ?? null,
    missing_fields: missingEngagementFields,
    source: `${dataSourceRef ?? "data-store"}#${resource}/${aggregateId}`,
  },
  bounce_policy_read: {
    hard_bounce_action: hardBounceAction,
    decay_threshold_days: decayThresholdDays ?? null,
  },
  unsubscribe_marker_active: unsubscribeMarkerActive,
  ambiguous_bounce_recovery: engagedSinceBounce,
  contact_projection_read: projection,
  expected_version: expectedVersion,
  idempotency_key: idempotencyKey,
  aggregate_id: aggregateId,
  cas_precondition_met: projection.readable && (versionDelta === 0 || idempotentReplay),
  idempotent_replay_of_recorded_transition: idempotentReplay,
  stale_expected_version: staleVersion,
  projection_version_delta: versionDelta,
  downstream_enforcer: "send-as (separate governed run, dispatched by naming; reads the recorded state and gates delivery)",
  send_effect: "none",
  harness_case_names: HARNESS_CASE_NAMES,
  receipt_id: "assigned by runx receipt after execution",
};

// Every declared runner output is always a well-formed object; the branch is
// expressed by the `recorded` / `status` field inside each one, never by an
// absent or null key. On a stop path contact_event is deliberately inert, so
// that even if the append guard were ever removed there is no transition in it
// to apply.
const result = {
  decision,
  observations,
  recorded_transition: {
    recorded: false,
    aggregate_id: aggregateId,
    resource,
    expected_version: expectedVersion,
    idempotency_key: idempotencyKey,
  },
  contact_event: {
    type: "contact.consent_no_transition",
    payload: {
      packet: "runx.list.hygiene_judge.v1",
      aggregate_id: aggregateId,
      resource,
      records_transition: false,
    },
  },
  stop_state: {
    status: "no_stop",
    reason: null,
    no_append_emitted: false,
    no_consent_transition_recorded: false,
  },
};

if (writes) {
  result.recorded_transition = { recorded: true, ...recordedTransition };
  result.contact_event = {
    type: `contact.consent_${decisionState}`,
    payload: {
      packet: "runx.list.hygiene_judge.v1",
      aggregate_id: aggregateId,
      resource,
      decision,
      transition: recordedTransition,
      evidence: {
        opens_count: opensCount,
        clicks_count: clicksCount,
        hard_bounces: hardBounces,
        recency_days: recencyDays,
        decay_threshold_days: decayThresholdDays,
        hard_bounce_action: hardBounceAction,
      },
      // Deliberately NOT the prior projection snapshot: the appended event must
      // be byte-identical on a retry, and the projection version moves. The
      // event describes the transition and the evidence it rested on, nothing
      // that mutates between attempts.
      data_store: {
        aggregate_id: aggregateId,
        expected_version: expectedVersion,
        idempotency_key: idempotencyKey,
      },
    },
  };
} else {
  result.stop_state = {
    status: escalate ? "needs_human" : stopReasons.length > 0 ? "needs_input" : "no_change",
    reason: decisionReason,
    no_append_emitted: true,
    no_consent_transition_recorded: true,
  };
}

process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);

function summarizeProjection(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { readable: false, seen: false, version: 0, event_count: 0, consent_state: null, unsubscribed: null };
  }
  // Accept either the raw data_operation_result ({data:{projection}}), the data
  // envelope ({projection}), or a bare projection object.
  const projectionBody =
    maybeObject(maybeObject(value.data)?.projection) ?? maybeObject(value.projection) ?? value;
  return {
    readable: true,
    seen: true,
    version: numberValue(projectionBody.version) ?? 0,
    event_count: numberValue(projectionBody.event_count) ?? 0,
    consent_state: stringValue(projectionBody.consent_state),
    unsubscribed: typeof projectionBody.unsubscribed === "boolean" ? projectionBody.unsubscribed : null,
    last_event_type: stringValue(projectionBody.last_event_type),
  };
}

function readInputs() {
  if (process.env.RUNX_INPUTS_PATH) {
    return JSON.parse(fs.readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"));
  }
  if (process.env.RUNX_INPUTS_JSON) {
    return JSON.parse(process.env.RUNX_INPUTS_JSON);
  }
  return {
    data_source_ref: parseInputValue(process.env.RUNX_INPUT_DATA_SOURCE_REF),
    resource: parseInputValue(process.env.RUNX_INPUT_RESOURCE),
    aggregate_id: parseInputValue(process.env.RUNX_INPUT_AGGREGATE_ID),
    expected_version: parseInputValue(process.env.RUNX_INPUT_EXPECTED_VERSION),
    idempotency_key: parseInputValue(process.env.RUNX_INPUT_IDEMPOTENCY_KEY),
    engagement_history: parseInputValue(process.env.RUNX_INPUT_ENGAGEMENT_HISTORY),
    bounce_policy: parseInputValue(process.env.RUNX_INPUT_BOUNCE_POLICY),
    current_consent_state: parseInputValue(process.env.RUNX_INPUT_CURRENT_CONSENT_STATE),
    contact_read: parseInputValue(process.env.RUNX_INPUT_CONTACT_READ),
  };
}

function parseInputValue(raw) {
  if (raw === undefined || raw === "") return undefined;
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

function maybeObject(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : null;
}

function numberValue(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

function stringValue(value) {
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
}

function sha256(value) {
  return crypto.createHash("sha256").update(value, "utf8").digest("hex");
}

function fail(message) {
  process.stderr.write(`${message}\n`);
  process.exit(64);
}
