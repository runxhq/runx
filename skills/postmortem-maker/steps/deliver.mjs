import {
  field,
  isMainModule,
  nowIso,
  readOutboxState,
  requireObject,
  requireString,
  runCli,
  sha256,
  writeOutboxState,
} from "./common.mjs";

function aggregateRef(target) {
  return `aggregate://${sha256(`${target.data_source_ref}|${target.aggregate_id}`).slice(7, 31)}`;
}

function messageRef(target, messageId) {
  return `outbox://postmortem-maker/${sha256(`${target.data_source_ref}|${target.aggregate_id}|${messageId}`).slice(7, 31)}`;
}

async function handler(envelope) {
  const target = requireObject(field(envelope, "publish_target"), "publish_target");
  const sendPlan = requireObject(field(envelope, "send_plan"), "send_plan");
  const message = requireObject(field(envelope, "message"), "message");
  const idempotencyKey = requireString(field(envelope, "idempotency_key"), "idempotency_key", { maxLength: 1_000 });
  const expectedVersion = field(envelope, "expected_version");
  if (!Number.isInteger(expectedVersion) || expectedVersion < 0) throw new Error("expected_version must be a non-negative integer");
  if (sendPlan.status !== "authorized") throw new Error("delivery requires an authorized send plan");
  if (sendPlan.content?.digest !== field(envelope, "content_digest") && field(envelope, "content_digest")) {
    throw new Error("send plan content digest does not match the approved content");
  }

  const state = readOutboxState(target);
  const existing = state.messages.find((item) => item.idempotency_key === idempotencyKey);
  if (existing) {
    if (existing.content_digest !== sendPlan.content?.digest) {
      throw new Error("idempotency key is already bound to different content");
    }
    return {
      delivery_result: {
        schema: "runx.postmortem-maker.publish-result.v1",
        status: "executed",
        delivery_status: "delivered",
        provider: "local-outbox",
        operation: "send",
        message_id: existing.message_id,
        message_ref: existing.message_ref,
        content_digest: existing.content_digest,
        postmortem_digest: existing.postmortem_digest,
        before_version: state.version,
        after_version: state.version,
        replayed: true,
        executed_at: nowIso(),
        idempotency_key: idempotencyKey,
      },
    };
  }

  if (state.version !== expectedVersion) {
    throw new Error(`publication outbox changed from version ${expectedVersion} to ${state.version}`);
  }

  const contentDigest = requireString(sendPlan.content?.digest, "send_plan.content.digest", { maxLength: 100 });
  const postmortemDigest = requireString(field(envelope, "postmortem_digest"), "postmortem_digest", { maxLength: 100 });
  const messageId = `msg_${sha256(idempotencyKey).slice(7, 31)}`;
  const storedMessage = {
    message_id: messageId,
    message_ref: messageRef(target, messageId),
    aggregate_ref: aggregateRef(target),
    idempotency_key: idempotencyKey,
    content_digest: contentDigest,
    postmortem_digest: postmortemDigest,
    channel: sendPlan.channel?.ref ?? target.channel,
    principal: sendPlan.principal?.ref ?? target.principal ?? "postmortem-maker",
    audience: sendPlan.audience?.ref ?? target.audience ?? target.channel,
    content: message,
  };
  const beforeVersion = state.version;
  const nextState = {
    ...state,
    version: beforeVersion + 1,
    messages: [...state.messages, storedMessage],
  };
  writeOutboxState(target, nextState);

  return {
    delivery_result: {
      schema: "runx.postmortem-maker.publish-result.v1",
      status: "executed",
      delivery_status: "delivered",
      provider: "local-outbox",
      operation: "send",
      message_id: messageId,
      message_ref: storedMessage.message_ref,
      content_digest: contentDigest,
      postmortem_digest: postmortemDigest,
      before_version: beforeVersion,
      after_version: nextState.version,
      replayed: false,
      executed_at: nowIso(),
      idempotency_key: idempotencyKey,
    },
  };
}

if (isMainModule(import.meta.url)) {
  runCli(handler);
}

export { handler as deliverPostmortem };
