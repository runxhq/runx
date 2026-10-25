const object = (value) => value && typeof value === "object" && !Array.isArray(value) ? value : {};
const text = (value) => typeof value === "string" ? value : "";
const bounded = (value, max) => [...text(value)].slice(0, max).join("");

export function decideAssistantRequest(inputs) {
  const presentationLocator = text(inputs.presentation_locator);
  const requestDigest = text(inputs.request_digest);
  const nonce = text(inputs.nonce);
  const authorizedActor = text(inputs.authorized_actor_id);
  const operation = object(inputs.provider_operation);
  const result = object(operation.result);
  if (operation.status !== "success" || operation.provider !== "slack" ||
      operation.operation !== "thread.read" || result.thread_locator !== presentationLocator ||
      !/^sha256:[0-9a-f]{64}$/u.test(requestDigest) || !/^[0-9a-f]{16}$/u.test(nonce) ||
      !/^[UW][A-Z0-9]{2,}$/u.test(authorizedActor)) {
    throw new Error("Slack decision read differs from the bound request or provider");
  }
  const messages = result.messages;
  const root = Array.isArray(messages) ? messages[0] : null;
  const rootAuthor = object(object(root).author);
  const rootText = text(object(root).preview).replace(/\s+/gu, " ").trim();
  const requestMarker = `Approval needed for a paused Runx action. Request: ${nonce} Digest: ${requestDigest}`;
  if (!Array.isArray(messages) || messages.length < 1 || messages.length > 15 ||
      object(root).message_locator !== presentationLocator ||
      !(rootAuthor.provider_actor_type === "app_or_bot" ||
        (rootAuthor.provider_actor_type === "user_account" && rootAuthor.external_id === authorizedActor)) ||
      !rootText.includes(requestMarker)) {
    throw new Error("Slack decision presentation differs from its verified control post");
  }
  // One approval request owns one short private thread. A truncated provider
  // page cannot establish that another, conflicting human answer is absent.
  if (result.next_cursor) {
    return { decision_observation: {
      status: "pending", request_digest: requestDigest, nonce,
      coverage_complete: false, next_cursor: result.next_cursor,
    } };
  }
  const rootTime = Date.parse(messages[0].occurred_at);
  if (!Number.isFinite(rootTime)) throw new Error("Slack decision root lacks an exact time");
  const answers = [];
  for (const message of messages.slice(1)) {
    const author = object(message.author);
    if (author.external_id !== authorizedActor || author.provider_actor_type !== "user_account") continue;
    const match = new RegExp(`^(approve|deny) ${nonce}$`, "u").exec(text(message.preview));
    if (!match) continue;
    const when = Date.parse(message.occurred_at);
    if (!Number.isFinite(when) || when <= rootTime ||
        !text(message.message_locator).startsWith(presentationLocator.slice(0, presentationLocator.lastIndexOf("/") + 1))) {
      throw new Error("Slack decision answer predates or escapes its presentation thread");
    }
    answers.push({ message, approved: match[1] === "approve" });
  }
  if (answers.length > 1) throw new Error("Slack decision thread contains ambiguous human answers");
  if (answers.length === 0) {
    return { decision_observation: {
      status: "pending", request_digest: requestDigest, nonce,
      coverage_complete: true, next_cursor: null,
    } };
  }
  const answer = answers[0];
  return { decision_observation: {
    status: "decided", request_digest: requestDigest, nonce,
    presentation_locator: presentationLocator,
    actor_id: authorizedActor,
    origin: "human_account",
    evidence_locator: answer.message.message_locator,
    occurred_at: answer.message.occurred_at,
    approved: answer.approved,
  } };
}

export function prepareAssistantScan(inputs) {
  const query = { ...object(object(inputs.config).query) };
  const limit = Number(inputs.page_limit);
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 20 ||
      query.limit !== limit || Object.hasOwn(query, "cursor")) {
    throw new Error("Slack source requires an exact bounded search query");
  }
  if (inputs.cursor !== null && inputs.cursor !== undefined) {
    if (!text(inputs.cursor) || inputs.cursor.length > 500) throw new Error("Slack source cursor is invalid");
    query.cursor = inputs.cursor;
  }
  return { scan_request: { query } };
}

export function finishAssistantScan(inputs) {
  const result = object(object(inputs.provider_operation).result);
  const messages = result.messages;
  if (!Array.isArray(messages) || messages.length > Number(inputs.page_limit)) {
    throw new Error("Slack source returned an unbounded search page");
  }
  const cursor = result.next_cursor;
  if (cursor !== null && cursor !== undefined &&
      (typeof cursor !== "string" || cursor.length > 500)) {
    throw new Error("Slack source returned an invalid cursor");
  }
  return { assistant_page: { schema: 'runx.conversation_page.v1', items: messages, next_cursor: cursor || null } };
}

export function observeAssistantMessage(inputs) {
  const item = object(inputs.item);
  const summary = bounded(item.preview, 1000);
  if (!summary.trim() || !text(item.message_locator) || !text(item.thread_locator) ||
      !text(item.occurred_at) || !text(object(item.author).external_id)) {
    throw new Error("Slack search row lacks a grounded message identity");
  }
  return { assistant_observation: {
    schema: 'runx.conversation_observation.v1',
    source_kind: "chat",
    summary,
    message: { ...item, preview: summary },
  } };
}

export function prepareAssistantHydrate(inputs) {
  const locator = text(inputs.thread_locator);
  const match = /^slack:\/\/([^/]+)\/([^/]+)\/([^/]+)$/u.exec(locator);
  const cursor = inputs.cursor;
  if (!match || !text(inputs.source_locator).startsWith(`slack://${match[1]}/${match[2]}/`) ||
      (cursor !== null && cursor !== undefined && (!text(cursor) || cursor.length > 500))) {
    throw new Error("Slack hydration locator or cursor is invalid");
  }
  const action = object(inputs.action);
  if (action.latest_message?.message_locator !== inputs.source_locator ||
      action.thread_locator !== locator || action.status !== "open") {
    throw new Error("Slack source differs from the canonical open action");
  }
  return { hydrate_request: { thread_locator: locator, cursor: cursor || null, limit: 15 } };
}

export function finishAssistantHydrate(inputs) {
  const action = object(inputs.action);
  const page = object(object(inputs.provider_operation).result);
  const locator = text(inputs.thread_locator);
  const tenant = locator.slice("slack://".length).split("/")[0];
  if (page.thread_locator !== locator ||
      page.external_tenant_ref !== tenant ||
      page.external_tenant_ref !== action.external_tenant_ref ||
      page.connected_subject_ref !== action.connected_subject_ref) {
    throw new Error("Slack thread readback differs from the admitted source");
  }
  if (!Array.isArray(page.messages) || page.messages.length > 15) {
    throw new Error("Slack thread page exceeds the message bound");
  }
  const channel = locator.slice(0, locator.lastIndexOf("/"));
  let complete = true;
  let source = "";
  let sourceMessage = null;
  const messages = page.messages.map((message) => {
    if (!text(message.message_locator).startsWith(`${channel}/`)) {
      throw new Error("Slack thread page contains another channel");
    }
    const preview = text(message.preview);
    const occurred = text(message.occurred_at);
    const author = text(object(message.author).external_id);
    if (!preview || [...preview].length > 1000 || !occurred || !author) complete = false;
    if (message.message_locator === inputs.source_locator) {
      if (occurred !== action.latest_message?.occurred_at ||
          author !== action.requester?.external_id || author === page.connected_subject_ref) {
        throw new Error("Slack source occurrence changed during hydration");
      }
      source = bounded(preview, 1000);
      sourceMessage = { ...message, provider: "slack",
        external_tenant_ref: page.external_tenant_ref,
        connected_subject_ref: page.connected_subject_ref,
        thread_locator: locator };
    }
    return { message_locator: message.message_locator, occurred_at: occurred,
      author_id: author, is_self: author === page.connected_subject_ref,
      text: bounded(preview, 1000) };
  });
  const cursor = page.next_cursor;
  if (cursor !== null && cursor !== undefined &&
      (typeof cursor !== "string" || cursor.length > 500)) {
    throw new Error("Slack thread returned an invalid cursor");
  }
  return { assistant_context: {
      schema: "runx.source_packet.v1",
      packet_id: `${inputs.source_locator}:${inputs.source_digest}`,
      source_ref: { type: "event", uri: inputs.source_locator, provider: "slack" },
      signal_type: "conversation",
      title: "Conversation evidence",
      observed_at: inputs.action.latest_message.occurred_at,
      redaction_status: "unredacted",
      workflow_inputs: {
    source_locator: inputs.source_locator,
    source_digest: inputs.source_digest,
    thread_locator: locator,
    source_message: sourceMessage,
    title: source ? bounded(source.split("\n")[0], 200) : "",
    body: source,
    context: { messages },
    complete,
    next_cursor: cursor || null,
    },
  } };
}
