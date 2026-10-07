const object = (value) => value && typeof value === "object" && !Array.isArray(value) ? value : {};
const text = (value) => typeof value === "string" ? value : "";
const positive = (value) => Number.isSafeInteger(value) && value > 0;
const bounded = (value, max) => [...text(value)].slice(0, max).join("");
function providerMillis(value) {
  const match = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2})(?:\.(\d{1,9}))?(Z|[+-]\d{2}:\d{2})$/u.exec(text(value));
  if (!match) return NaN;
  const millis = (match[2] || "").padEnd(3, "0").slice(0, 3);
  return Date.parse(`${match[1]}.${millis}${match[3]}`);
}
const sameTime = (left, right) => Number.isFinite(providerMillis(left)) &&
  providerMillis(left) === providerMillis(right);

function configOf(value, pageLimit) {
  const config = object(value);
  const brandSid = text(config.brand_sid);
  const arguments_ = object(config.arguments);
  if (!brandSid || arguments_.view !== "full" || arguments_.page !== 1 ||
      !Number.isSafeInteger(arguments_.per) || arguments_.per !== pageLimit ||
      pageLimit < 1 || pageLimit > 20) {
    throw new Error("mail source requires an exact brand, full rows, and a bounded first page");
  }
  return { brandSid, arguments_ };
}

function resultOf(evidence) {
  if (evidence?.decision !== "ok" || !evidence.result || typeof evidence.result !== "object") {
    throw new Error("mail source read lacks an accepted provider result");
  }
  return evidence.result;
}

export function prepareAssistantScan(inputs) {
  const limit = Number(inputs.page_limit);
  const { brandSid, arguments_ } = configOf(inputs.config, limit);
  let page = 1;
  if (inputs.cursor !== null && inputs.cursor !== undefined) {
    const match = /^page:([2-9]\d{0,3}|10000)$/u.exec(text(inputs.cursor));
    if (!match) throw new Error("mail source cursor is invalid");
    page = Number(match[1]);
  }
  return { scan_request: { brand_sid: brandSid,
    arguments: { command: "list_mailbox", arguments: { ...arguments_, page } } } };
}

export function finishAssistantScan(inputs) {
  const result = resultOf(inputs.provider_evidence);
  const items = result.items;
  const pagination = object(result.pagination);
  const requestedPage = inputs.cursor ? Number(text(inputs.cursor).slice("page:".length)) : 1;
  if (!Array.isArray(items) || items.length > Number(inputs.page_limit) ||
      !positive(pagination.page) || !Number.isSafeInteger(pagination.total_pages) ||
      pagination.page !== requestedPage || pagination.page > pagination.total_pages + 1) {
    throw new Error("mail source returned an unbounded page");
  }
  return {
    assistant_page: {
      schema: 'runx.conversation_page.v1',
      items,
      next_cursor: pagination.page < pagination.total_pages
        ? `page:${pagination.page + 1}` : null,
    },
  };
}

export function prepareAssistantObserve(inputs) {
  const { brandSid } = configOf(inputs.config, object(inputs.config).arguments?.per);
  const row = object(inputs.item);
  if (!positive(row.conversation_id)) throw new Error("mail row lacks a conversation id");
  return { observe_request: { brand_sid: brandSid,
    arguments: { command: "get_thread", arguments: { conversation_id: row.conversation_id } } } };
}

function observedMail(brandSid, row, result, expectedPurpose = "read") {
  const thread = object(result.thread);
  const grounding = expectedPurpose === "reply"
    ? object(object(result.reply_context).thread_grounding) : object(result.thread_grounding);
  const id = row.conversation_id;
  if (result.status !== "ok" || result.purpose !== expectedPurpose ||
      thread.conversation_id !== id || grounding.conversation_id !== id ||
      !sameTime(row.last_message_at, thread.last_message_at) ||
      !sameTime(row.updated_at, thread.updated_at) || row.subject !== thread.subject ||
      row.preview !== thread.preview || !Array.isArray(grounding.messages)) {
    throw new Error("mail row changed before its thread was grounded");
  }
  const time = new Date(providerMillis(row.last_message_at));
  if (!Number.isFinite(time.getTime())) throw new Error("mail row has no valid time");
  const latest = grounding.messages
    .filter((message) => positive(message.id) && sameTime(message.occurred_at, row.last_message_at))
    .sort((left, right) => right.id - left.id)[0];
  if (!latest || !["inbound", "outbound"].includes(latest.direction) ||
      thread.latest_activity?.direction !== latest.direction) {
    throw new Error("mail thread lacks a matching latest message");
  }
  const threadLocator = `nitrosend://${brandSid}/${id}`;
  const connected = `nitrosend:${brandSid}`;
  const summary = bounded(`${text(row.subject) || "Mail"} — ${text(row.preview)}`, 1000);
  if (!summary.trim()) throw new Error("mail row has no summary");
  return {
    provider: "nitrosend",
    external_tenant_ref: brandSid,
    connected_subject_ref: connected,
    message_locator: `${threadLocator}/messages/${latest.id}`,
    thread_locator: threadLocator,
    author: { external_id: latest.direction === "inbound" ? text(latest.sender_address) : connected },
    conversation: { external_id: String(id), type: "mail" },
    occurred_at: time.toISOString(),
    preview: summary,
    context: [],
  };
}

export function finishAssistantObserve(inputs) {
  const row = object(inputs.item);
  const brandSid = text(object(inputs.config).brand_sid);
  const message = observedMail(brandSid, row, resultOf(inputs.provider_evidence));
  return { assistant_observation: { schema: 'runx.conversation_observation.v1', source_kind: "mail", summary: message.preview, message } };
}

export function prepareAssistantHydrate(inputs) {
  if (inputs.cursor !== null && inputs.cursor !== undefined) {
    throw new Error("mail hydration does not accept a continuation cursor");
  }
  const config = object(inputs.config);
  const brandSid = text(config.brand_sid);
  const match = /^nitrosend:\/\/([^/]+)\/(\d+)$/u.exec(text(inputs.thread_locator));
  if (!match || match[1] !== brandSid || !positive(Number(match[2])) ||
      !text(inputs.source_ref).startsWith(`${inputs.thread_locator}/messages/`)) {
    throw new Error("mail source locator is outside the configured brand");
  }
  return { hydrate_request: { brand_sid: brandSid,
    arguments: { command: "get_thread", arguments: { conversation_id: Number(match[2]), purpose: "reply" } } } };
}

export function finishAssistantHydrate(inputs) {
  const result = resultOf(inputs.provider_evidence);
  const action = object(inputs.action);
  const detail = object(result.thread);
  const row = {
    conversation_id: detail.conversation_id,
    last_message_at: detail.last_message_at,
    updated_at: detail.updated_at,
    subject: detail.subject,
    preview: detail.preview,
  };
  const brandSid = text(object(inputs.config).brand_sid);
  const current = observedMail(brandSid, row, result, "reply");
  if (current.message_locator !== inputs.source_ref ||
      action.latest_message?.message_locator !== inputs.source_ref ||
      action.latest_message?.occurred_at !== current.occurred_at ||
      action.latest_message?.preview !== current.preview ||
      action.requester?.external_id !== current.author.external_id) {
    throw new Error("mail source changed after admission");
  }
  const replyContext = object(result.reply_context);
  const grounding = object(replyContext.thread_grounding);
  const replyContextDigest = text(replyContext.context_digest);
  if (!/^[0-9a-f]{64}$/u.test(replyContextDigest)) {
    throw new Error("mail source lacks a current reply context digest");
  }
  const messages = grounding.messages;
  if (!Array.isArray(messages) || messages.length > 25) {
    throw new Error("mail thread returned an unbounded grounding window");
  }
  let complete = grounding.omitted_before?.count === 0 && grounding.returned_count === messages.length;
  let remaining = 20000;
  const contextMessages = messages.map((message) => {
    const body = text(message.text_body);
    const limit = Math.min(4000, remaining);
    const excerpt = bounded(body, limit);
    remaining -= [...excerpt].length;
    if (!body || [...body].length > limit || message.body_truncated !== false ||
        !text(message.occurred_at) || !text(message.direction) ||
        !Array.isArray(message.attachments) || message.attachments.length !== 0) complete = false;
    return { message_ref: String(message.id), direction: text(message.direction),
      occurred_at: text(message.occurred_at), text: excerpt };
  });
  const sourceId = Number(text(inputs.source_ref).split("/").at(-1));
  const selected = messages.find((message) => message.id === sourceId);
  const body = text(selected?.text_body);
  if (!selected || !body.trim()) throw new Error("mail source body is absent from the grounded thread");
  complete &&= selected.body_truncated === false && [...body].length <= 4000;
  return {
    assistant_context: {
      schema: "runx.source_packet.v1",
      packet_id: `${inputs.source_ref}:${inputs.source_digest}`,
      source_ref: { type: "event", uri: inputs.source_ref, provider: "nitrosend" },
      signal_type: "conversation",
      title: "Conversation evidence",
      observed_at: inputs.action.latest_message.occurred_at,
      redaction_status: "unredacted",
      workflow_inputs: {
      source_ref: inputs.source_ref,
      source_digest: inputs.source_digest,
      thread_locator: inputs.thread_locator,
      source_message: current,
      title: bounded(text(detail.subject) || "Mail request", 200),
      body: bounded(body, 4000),
      context: { thread_locator: inputs.thread_locator, conversation_id: detail.conversation_id,
        source_ref: inputs.source_ref,
        source_digest: inputs.source_digest, messages: contextMessages,
        omitted_before_count: grounding.omitted_before?.count,
        coverage_incomplete: !complete, requires_reply_context: true,
        reply_context_digest: replyContextDigest },
      complete,
      next_cursor: null,
      },
    },
  };
}
