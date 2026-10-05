const text = (value) => typeof value === "string" ? value.trim() : "";
const kinds = new Set(["reply", "follow_up", "coding", "no_action", "needs_context"]);

export function finalizeConversation(inputs) {
  const draft = inputs.draft && typeof inputs.draft === "object" && !Array.isArray(inputs.draft)
    ? inputs.draft : {};
  const kind = text(draft.kind);
  const summary = text(draft.summary);
  const reason = text(draft.reason);
  const message = text(draft.draft_message);
  const quote = text(draft.request_quote);
  const followUpTask = text(draft.follow_up_task);
  const sourceComplete = inputs.source_complete === true && inputs.thread?.coverage_incomplete === false;
  const sourceText = [text(inputs.title), text(inputs.body), ...(
    Array.isArray(inputs.thread?.messages)
      ? inputs.thread.messages.map((entry) => text(entry?.text)) : []
  )].join("\n");
  const findings = [];
  if (!text(inputs.source_ref) || !/^sha256:[0-9a-f]{64}$/.test(text(inputs.source_digest))
      || !text(inputs.thread_locator) || inputs.thread?.source_ref !== inputs.source_ref
      || inputs.thread?.source_digest !== inputs.source_digest
      || inputs.thread?.thread_locator !== inputs.thread_locator) {
    findings.push("source binding is incomplete or inconsistent");
  }
  if (!kinds.has(kind) || !summary || summary.length > 1000 || !reason || reason.length > 1000
      || message.length > 4000 || quote.length > 1000 || followUpTask.length > 1000) {
    findings.push("review fields are missing or exceed bounds");
  }
  if (kind === "reply" && !message) findings.push("reply requires an exact draft");
  if (kind === "coding" && (!quote || !sourceText.includes(quote))) {
    findings.push("coding requires a verbatim request quote from the bound conversation");
  }
  if (kind === "follow_up" && !followUpTask) findings.push("follow-up requires a concrete task proposal");
  const valid = findings.length === 0;
  const status = valid ? (sourceComplete ? kind : "needs_context") : "needs_context";
  return {
    conversation_packet: {
      schema: "runx.conversation.review.v1",
      status,
      source_ref: text(inputs.source_ref),
      source_digest: text(inputs.source_digest),
      thread_locator: text(inputs.thread_locator),
      summary: valid ? summary : "",
      reason: valid ? (sourceComplete ? reason : "conversation coverage is incomplete") : findings.join("; "),
      draft_message: status === "reply" ? message : null,
      request_quote: status === "coding" ? quote : null,
      follow_up_task: status === "follow_up" ? followUpTask : null,
      follow_up_status: status === "follow_up" ? "not_queued" : null,
      delivery_status: "not_sent",
      validation: { status: valid ? "pass" : "fail", findings },
    },
  };
}
