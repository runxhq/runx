export function finalizeConversation(inputs) {
  const {draft, thread} = inputs;
  const {kind, summary, reason, draft_message: message, request_quote: quote, follow_up_task: followUpTask} = draft;
  const sourceComplete = inputs.source_complete && !thread.coverage_incomplete;
  const findings = [];
  if (thread.source_locator !== inputs.source_locator || thread.source_digest !== inputs.source_digest ||
      thread.thread_locator !== inputs.thread_locator) findings.push("source binding is incomplete or inconsistent");
  if (kind === "reply" && !message?.trim()) findings.push("reply requires an exact draft");
  if (kind === "coding" && (!quote?.trim() ||
      ![inputs.title, inputs.body, ...thread.messages.map(entry => entry.text)].some(value => value.includes(quote)))) {
    findings.push("coding requires a verbatim request quote from one bound message");
  }
  if (kind === "follow_up" && !followUpTask?.trim()) findings.push("follow-up requires a concrete task proposal");
  const valid = findings.length === 0;
  const status = valid && sourceComplete ? kind : "needs_context";
  return {conversation_packet: {
    schema: "runx/conversation-review.review.v1", status,
    source_locator: inputs.source_locator, source_digest: inputs.source_digest, thread_locator: inputs.thread_locator,
    summary: valid ? summary : "",
    reason: valid ? (sourceComplete ? reason : "conversation coverage is incomplete") : findings.join("; "),
    draft_message: status === "reply" ? message : null,
    request_quote: status === "coding" ? quote : null,
    follow_up_task: status === "follow_up" ? followUpTask : null,
    follow_up_status: status === "follow_up" ? "not_queued" : null,
    delivery_status: "not_sent", validation: {status: valid ? "pass" : "fail", findings},
  }};
}

export function prepareWork({source_context: source}) {
  const evidence = source.workflow_inputs;
  if (!evidence || source.source_ref.uri !== evidence.source_locator) {
    throw new Error("conversation work requires bound normalized source evidence");
  }
  return {review_inputs: {
    source_locator: evidence.source_locator, source_digest: evidence.source_digest,
    thread_locator: evidence.thread_locator, title: evidence.title, body: evidence.body,
    thread: evidence.thread, source_complete: evidence.source_complete,
  }};
}

export function finishWork({conversation_packet: review, handoff_routes: routes = {}}) {
  let state = "held";
  let summary = review.reason;
  const followups = [];
  const completion_evidence = [];
  if (review.validation.status === "pass" && review.status === "no_action") {
    state = "satisfied";
    summary = review.summary;
    completion_evidence.push(review.source_locator + "@" + review.source_digest + ": no action required");
  } else if (review.validation.status === "pass" && ["reply", "coding", "follow_up"].includes(review.status)) {
    const route = routes?.[review.status];
    if (route) {
      state = "follow_up";
      summary = review.summary;
      const next = review.status === "reply" ? {draft_message: review.draft_message, thread_locator: review.thread_locator}
        : review.status === "coding" ? {request_quote: review.request_quote} : {objective: review.follow_up_task};
      followups.push({route_id: route, target_locator: review.thread_locator, inputs: next, due_at_unix_seconds: 0});
    } else summary = "Held: " + review.status + " needs a configured handoff route. " + review.summary;
  }
  return {work_result: {state, summary, completion_evidence, followups}};
}
