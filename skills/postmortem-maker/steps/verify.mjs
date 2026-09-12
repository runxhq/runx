import {
  PACKAGE_NAME,
  PACKAGE_VERSION,
  field,
  isMainModule,
  readOutboxState,
  requireObject,
  runCli,
} from "./common.mjs";

function sourceSummary(incident) {
  return {
    kind: incident.source?.kind ?? "incident_thread",
    ref: incident.source?.ref ?? "unknown",
    read_mode: incident.source?.read_mode ?? "unknown",
    events_read: incident.source?.events_read ?? (Array.isArray(incident.events) ? incident.events.length : 0),
    source_digest: incident.source?.source_digest ?? null,
  };
}

async function handler(envelope) {
  const incident = requireObject(field(envelope, "incident"), "incident");
  const postmortem = requireObject(field(envelope, "postmortem"), "postmortem");
  const target = requireObject(field(envelope, "publish_target") ?? {}, "publish_target");
  const sendPlan = requireObject(field(envelope, "send_plan"), "send_plan");
  const publishable = field(envelope, "publishable") === true;
  const contentDigest = field(envelope, "content_digest");
  const postmortemDigest = field(envelope, "postmortem_digest");
  const idempotencyKey = field(envelope, "idempotency_key");
  const expectedVersion = field(envelope, "expected_version");
  const deliveryResult = field(envelope, "delivery_result") ?? null;
  const state = readOutboxState(target);
  const message = typeof idempotencyKey === "string"
    ? state.messages.find((item) => item.idempotency_key === idempotencyKey) ?? null
    : null;

  let readback;
  let publishResult = deliveryResult;
  if (publishable) {
    if (!message) throw new Error("authorized publication has no outbox message to read back");
    const digestMatch = message.content_digest === contentDigest && message.postmortem_digest === postmortemDigest;
    if (!digestMatch) throw new Error("outbox readback digest does not match the authorized postmortem");
    readback = {
      delivered: true,
      digest_match: true,
      delivery_exists: true,
      provider_act_performed: true,
      message_id: message.message_id,
      message_ref: message.message_ref,
      outbox_version_after: state.version,
      outbox_unchanged: deliveryResult?.replayed === true,
    };
    if (!publishResult) {
      publishResult = {
        schema: "runx.postmortem-maker.publish-result.v1",
        status: "executed",
        delivery_status: "delivered",
        provider: "local-outbox",
        operation: "send",
        message_id: message.message_id,
        message_ref: message.message_ref,
        content_digest: message.content_digest,
        postmortem_digest: message.postmortem_digest,
        before_version: expectedVersion,
        after_version: state.version,
        replayed: true,
        idempotency_key: idempotencyKey,
      };
    }
  } else {
    const unchanged = Number.isInteger(expectedVersion) && state.version === expectedVersion;
    if (message || !unchanged) {
      throw new Error("withheld publication could not prove absence without changing the outbox");
    }
    readback = {
      delivered: false,
      digest_match: false,
      delivery_exists: false,
      provider_act_performed: false,
      send_plan_created: false,
      outbox_unchanged: true,
      outbox_version_after: state.version,
    };
    publishResult = null;
  }

  const evidence = {
    source_read: sourceSummary(incident),
    timeline_count: Array.isArray(postmortem.timeline) ? postmortem.timeline.length : 0,
    impact: postmortem.impact,
    root_cause_status: postmortem.root_cause?.status ?? "unknown",
    unknowns: field(envelope, "unknowns") ?? [],
    action_items: field(envelope, "action_items") ?? [],
    executed_publish_result: publishResult,
    send_plan_status: sendPlan.status,
    receipt_id: null,
  };
  const verification = {
    schema: "runx.postmortem-maker.verification.v1",
    status: "passed",
    checked_at: new Date().toISOString(),
    readback,
  };
  return {
    schema: "runx.postmortem-maker.result.v1",
    skill: PACKAGE_NAME,
    version: PACKAGE_VERSION,
    source: sourceSummary(incident),
    postmortem,
    unknowns: field(envelope, "unknowns") ?? [],
    action_items: field(envelope, "action_items") ?? [],
    publishable,
    root_cause_status: postmortem.root_cause?.status ?? "unknown",
    timeline_count: Array.isArray(postmortem.timeline) ? postmortem.timeline.length : 0,
    content_digest: contentDigest,
    idempotency_key: idempotencyKey,
    expected_version: expectedVersion,
    send_plan: sendPlan,
    publish_result: publishResult,
    readback,
    verification,
    evidence,
  };
}

if (isMainModule(import.meta.url)) {
  runCli(handler);
}

export { handler as verifyPostmortem };
