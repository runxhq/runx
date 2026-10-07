const object = (value) => value && typeof value === "object" && !Array.isArray(value) ? value : {};
const text = (value) => typeof value === "string" ? value.trim() : "";

function uuidFromTurn(turn) {
  const hex = text(turn).match(/^sha256:([0-9a-f]{64})$/)?.[1];
  if (!hex) return null;
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-8${hex.slice(17, 20)}-${hex.slice(20, 32)}`;
}

export function prepareReply(inputs) {
  const source = object(inputs.source_context);
  const evidence = object(source.workflow_inputs);
  const thread = object(evidence.thread);
  const provider = text(inputs.expected_provider);
  const target = text(inputs.thread_locator);
  const message = typeof inputs.draft_message === "string" ? inputs.draft_message : "";
  const digest = text(object(inputs.digest).digest);
  const idempotencyKey = uuidFromTurn(inputs.turn_id);
  const conversationId = thread.conversation_id;
  const deliveryRoute = text(inputs.delivery_route);
  const ready = source.schema === "runx.source_packet.v1"
    && source.source_ref?.uri === evidence.source_ref
    && evidence.source_complete === true && thread.coverage_incomplete === false
    && evidence.thread_locator === target && thread.thread_locator === target
    && evidence.source_ref.startsWith(`${provider}://`)
    && /^sha256:[0-9a-f]{64}$/.test(evidence.source_digest)
    && /^sha256:[0-9a-f]{64}$/.test(digest)
    && (thread.requires_reply_context !== true || (!!text(thread.reply_context_digest) &&
      text(thread.reply_context_digest).length <= 128))
    && (conversationId == null || (Number.isSafeInteger(conversationId) &&
      conversationId > 0 && target.endsWith(`/${conversationId}`)))
    && !!idempotencyKey && !!message.trim() && message === message.trim() && message.length <= 4000;
  const request = { thread_locator: target, text: message };
  if (conversationId != null) request.conversation_id = conversationId;
  const contextDigest = text(thread.reply_context_digest);
  if (contextDigest) request.reply_context_digest = contextDigest;
  return { reply_plan: { ready, provider, target, content_digest: digest,
    idempotency_key: idempotencyKey, request, delivery_route: deliveryRoute,
    conversation_id: conversationId ?? null,
    reason: ready ? "" : "Reply needs complete, exact source evidence and an admitted turn." } };
}

export function prepareReadback(inputs) {
  const plan = object(inputs.reply_plan);
  const mutation = object(inputs.mutation);
  const result = object(mutation.result);
  if (mutation.status !== "success" || mutation.provider !== plan.provider ||
      mutation.target !== plan.target) {
    throw new Error("reply mutation differs from its admitted provider and thread");
  }
  const messageLocator = text(result.message_locator);
  const messageId = result.message_id;
  if (!messageLocator && (!Number.isSafeInteger(messageId) || messageId <= 0)) {
    throw new Error("reply mutation lacks a stable provider message identity");
  }
  return { read_request: { thread_locator: plan.target,
    ...(Number.isSafeInteger(plan.conversation_id) ? { conversation_id: plan.conversation_id } : {}),
    ...(messageLocator ? { message_locator: messageLocator, expected_text: plan.request.text } : {}),
    ...(Number.isSafeInteger(messageId) ? { message_id: messageId } : {}) } };
}

export function holdReply(inputs) {
  const plan = object(inputs.reply_plan);
  return { work_result: { disposition: "held", summary: text(plan.reason) || "Reply could not be bound.",
    completion_evidence: [], followups: [] } };
}

export function finishReply(inputs) {
  const plan = object(inputs.reply_plan);
  const mutation = object(inputs.mutation);
  const readback = object(inputs.readback);
  const observed = object(readback.result);
  const accepted = object(mutation.result);
  const slackSame = !!observed.message_locator && observed.message_locator === accepted.message_locator
    && observed.thread_locator === plan.target && observed.content_digest === plan.content_digest;
  const queuedSame = Number.isSafeInteger(accepted.message_id) && accepted.message_id > 0
    && observed.message_id === accepted.message_id && observed.to === accepted.to
    && observed.subject === accepted.subject;
  if (readback.status !== "success" || readback.finality !== "confirmed" ||
      readback.provider !== plan.provider || readback.target !== plan.target ||
      (!slackSame && !queuedSame)) {
    return { work_result: { disposition: "held",
      summary: "Provider accepted the reply, but exact delivery readback is incomplete; reconcile this message before further action.",
      completion_evidence: [], followups: [] } };
  }
  if (queuedSame) return deliveryResult(plan, observed, 1, readback.readback_ref);
  return { work_result: { disposition: "satisfied",
    summary: "The approved reply was read back in its bound thread.",
    completion_evidence: [`${observed.message_locator}@${plan.content_digest}`, readback.readback_ref], followups: [] } };
}

function deliveryResult(plan, observed, attempt, readbackRef) {
  const messageLocator = `${plan.target}/messages/${observed.message_id}`;
  if (observed.status === "sent") {
    return { work_result: { disposition: "satisfied",
      summary: "The approved reply reached the provider's sent state.",
      completion_evidence: [`${messageLocator}@${plan.content_digest}`, readbackRef], followups: [] } };
  }
  if (observed.status === "queued" && plan.delivery_route && attempt <= 24) {
    return { work_result: { disposition: "follow_up",
      summary: "The approved reply is queued; delivery remains under observation.",
      completion_evidence: [], followups: [{ route_id: plan.delivery_route,
        target_ref: plan.target, due_at_unix_seconds: 0,
        due_after_seconds: Math.min(300 * 2 ** Math.min(attempt - 1, 4), 3600),
        inputs: { thread_locator: plan.target, message_id: observed.message_id,
          ...(Number.isSafeInteger(plan.conversation_id) ? { conversation_id: plan.conversation_id } : {}),
          content_digest: plan.content_digest, attempt, delivery_route: plan.delivery_route } }] } };
  }
  return { work_result: { disposition: "held",
    summary: observed.status === "failed" ? "The provider reported reply delivery failure." :
      "Reply delivery has not reached a verified sent state.",
    completion_evidence: [], followups: [] } };
}

export function finishDeliveryCheck(inputs) {
  const readback = object(inputs.readback);
  const observed = object(readback.result);
  const messageId = inputs.message_id;
  if (readback.status !== "success" || readback.finality !== "confirmed" ||
      readback.provider !== text(inputs.expected_provider) ||
      readback.target !== text(inputs.thread_locator) ||
      observed.message_id !== messageId) {
    return { work_result: { disposition: "held",
      summary: "Reply delivery check lacks an exact provider readback.",
      completion_evidence: [], followups: [] } };
  }
  return deliveryResult({ target: text(inputs.thread_locator),
    conversation_id: inputs.conversation_id, content_digest: text(inputs.content_digest),
    delivery_route: text(inputs.delivery_route) },
  observed, Number(inputs.attempt) + 1, readback.readback_ref);
}

export function prepareDeliveryCheck(inputs) {
  const target = text(inputs.thread_locator);
  const id = inputs.message_id;
  const conversationId = inputs.conversation_id;
  if (!target || !Number.isSafeInteger(id) || id <= 0 ||
      !Number.isSafeInteger(conversationId) || conversationId <= 0 ||
      !target.endsWith(`/${conversationId}`)) {
    throw new Error("reply delivery check lacks its bound message identity");
  }
  return { read_request: { thread_locator: target, conversation_id: conversationId, message_id: id } };
}
