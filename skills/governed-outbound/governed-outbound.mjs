export default function validatePlan(inputs) {
  const principal = requiredString(inputs.principal, "principal");
  const channel = requiredString(inputs.channel, "channel");
  const source = requiredRecord(inputs.source, "source");
  const redaction = requiredRecord(inputs.redaction, "redaction");
  const sendPlan = requiredRecord(inputs.send_plan, "send_plan");
  if (redaction.decision !== "ready") throw new Error("redaction must be ready");
  if (sendPlan.decision !== "ready") throw new Error("send plan must be ready");
  assertEqual(requiredString(record(sendPlan.principal).ref, "send_plan.principal.ref"), principal, "send plan principal differs from the approved principal");
  assertEqual(requiredString(record(sendPlan.audience).ref, "send_plan.audience.ref"), channel, "send plan audience differs from the approved audience");
  const redactedDigest = requiredDigest(redaction.redacted_digest, "redaction.redacted_digest");
  assertEqual(requiredDigest(record(sendPlan.content).digest, "send_plan.content.digest"), redactedDigest, "send plan digest differs from the scrubbed artifact");
  if (record(sendPlan.success_checkpoint).milestone !== "provider_delivery_required") {
    throw new Error("send plan must leave provider delivery and readback outstanding");
  }
  if (sendPlan.delivery_evidence || sendPlan.provider_receipt || sendPlan.delivery_status === "delivered") {
    throw new Error("plan-only handoff must not claim provider delivery evidence");
  }
  return {
    outbound_plan: {
      decision: "ready",
      completion: "plan_only",
      provider_delivery: "not_executed",
      principal,
      audience: channel,
      source: {
        final_url: requiredString(source.final_url, "source.final_url"),
        digest: requiredDigest(source.content_digest, "source.content_digest"),
        fetched_at: requiredString(record(source.provenance).fetched_at, "source.provenance.fetched_at"),
      },
      redaction: {
        digest: redactedDigest,
        source_digest: requiredDigest(redaction.source_digest, "redaction.source_digest"),
      },
      provider_actions: strings(sendPlan.provider_actions),
      send_plan: sendPlan,
    },
  };
}

function assertEqual(actual, expected, message) { if (actual !== expected) throw new Error(message); }
function requiredDigest(value, field) {
  const parsed = requiredString(value, field);
  if (!/^sha256:[0-9a-f]{64}$/u.test(parsed)) throw new Error(`${field} must be a sha256 digest`);
  return parsed;
}
function requiredString(value, field) {
  const parsed = optionalString(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}
function optionalString(value) { return typeof value === "string" && value.trim() ? value.trim() : null; }
function strings(value) { return Array.isArray(value) ? [...new Set(value.map(optionalString).filter(Boolean))] : []; }
function record(value) { return value && typeof value === "object" && !Array.isArray(value) ? value : {}; }
function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}
