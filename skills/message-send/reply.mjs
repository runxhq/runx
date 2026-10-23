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
  const ready = source.schema === "runx.source_packet.v1"
    && source.source_ref?.uri === evidence.source_locator
    && evidence.source_complete === true && thread.coverage_incomplete === false
    && evidence.thread_locator === target && thread.thread_locator === target
    && evidence.source_locator.startsWith(`${provider}://`)
    && /^sha256:[0-9a-f]{64}$/.test(evidence.source_digest)
    && /^sha256:[0-9a-f]{64}$/.test(digest)
    && !!idempotencyKey && !!message.trim() && message === message.trim() && message.length <= 4000;
  return { reply_plan: { ready, provider, target, content_digest: digest,
    idempotency_key: idempotencyKey, request: { thread_locator: target, text: message },
    reason: ready ? "" : "Reply needs complete, exact source evidence and an admitted turn." } };
}

export function prepareReadback(inputs) {
  const plan = object(inputs.reply_plan);
  const mutation = object(inputs.mutation);
  const messageLocator = text(object(mutation.result).message_locator);
  if (mutation.status !== "success" || mutation.provider !== plan.provider ||
      mutation.target !== plan.target) {
    throw new Error("reply mutation differs from its admitted provider and thread");
  }
  if (!messageLocator) throw new Error("reply mutation lacks a stable provider message identity");
  return { read_request: { thread_locator: plan.target, message_locator: messageLocator,
    expected_text: plan.request.text } };
}

export function holdReply(inputs) {
  const plan = object(inputs.reply_plan);
  return { work_result: { state: "held", summary: text(plan.reason) || "Reply could not be bound.",
    completion_evidence: [], followups: [] } };
}

export function finishReply(inputs) {
  const plan = object(inputs.reply_plan);
  const mutation = object(inputs.mutation);
  const readback = object(inputs.readback);
  const observed = object(readback.result);
  const accepted = object(mutation.result);
  const same = !!observed.message_locator && observed.message_locator === accepted.message_locator
    && observed.thread_locator === plan.target && observed.content_digest === plan.content_digest;
  if (readback.status !== "success" || readback.finality !== "confirmed" ||
      readback.provider !== plan.provider || readback.target !== plan.target || !same) {
    return { work_result: { state: "held",
      summary: "Provider accepted the reply, but exact delivery readback is incomplete; reconcile this message before further action.",
      completion_evidence: [], followups: [] } };
  }
  return { work_result: { state: "satisfied",
    summary: "The approved reply was read back in its bound thread.",
    completion_evidence: [`${observed.message_locator}@${plan.content_digest}`, readback.readback_uri], followups: [] } };
}
