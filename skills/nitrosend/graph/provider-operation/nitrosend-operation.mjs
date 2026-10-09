const READ_OPERATIONS = new Map([
  ["status", "nitro_get_status"],
  ["query", "nitro_query"],
  ["inbox", "nitro_inbox"],
  ["sender_settings", "nitro_configure_account"],
  ["insights", "nitro_get_insights"],
  ["review_delivery", "nitro_review_delivery"],
  ["import_status", "nitro_query"],
  ["compose_email", null],
  ["billing_status", "nitro_manage_billing"],
  ["billing_plans", "nitro_manage_billing"],
  ["plan_checkout_status", "nitro_manage_billing"],
]);
const ACT_OPERATIONS = new Map([
  ["plan_checkout", "nitro_manage_billing"],
  ["send_test_message", "nitro_send_test_message"],
  ["send_transactional", "nitro_send_message"],
  ["configure_sender", "nitro_configure_account"],
  ["control_delivery", "nitro_control_delivery"],
  ["import_contacts", "nitro_import_contacts"],
  ["compose_campaign", "nitro_compose_campaign"],
  ["compose_flow", "nitro_compose_flow"],
  ["manage_template", "nitro_manage_template"],
  ["define_segment", "nitro_define_segment"],
  ["ingest_image", "nitro_ingest"],
]);
// The Nitrosend connector operation each skill operation runs as.
const CONNECTOR_OPERATIONS = new Map([
  ["status", "status.read"],
  ["query", "entities.query"],
  ["import_status", "entities.query"],
  ["inbox", "inbox.read"],
  ["sender_settings", "sender.read"],
  ["insights", "insights.read"],
  ["review_delivery", "delivery.review"],
  ["billing_status", "billing.status"],
  ["billing_plans", "billing.plans"],
  ["plan_checkout_status", "checkout.status"],
  ["plan_checkout", "plan.checkout"],
  ["configure_sender", "sender.configure"],
  ["compose_campaign", "campaign.draft"],
  ["compose_flow", "flow.draft"],
  ["manage_template", "template.draft"],
  ["ingest_image", "image.ingest"],
]);
const CREATIVE_DRAFT_OPERATIONS = new Set([
  "compose_campaign", "compose_flow", "manage_template",
]);
// A composition read, by mode: intent plans and validate checks; neither
// persists.
const COMPOSITION_READS = new Map([["intent", "plan"], ["validate", "validate"]]);
// The connector operations that only read, among those an act may choose.
const READ_CONNECTOR_OPERATIONS = new Set([
  "test_message.preview", "message.preview", "contacts.records.preview", "segment.preview",
]);
// A delivery control, by target and lifecycle operation.
const CONTROL_OPERATIONS = new Map([
  ["campaign:approve", "campaign.approve"],
  ["flow:approve", "flow.approve"],
  ["campaign:live", "campaign.send"],
  ["campaign:schedule", "campaign.schedule"],
  ["flow:live", "flow.activate"],
]);
const COMPOSITION_TOOLS = new Map([
  ["campaign", "nitro_compose_campaign"],
  ["flow", "nitro_compose_flow"],
  ["template", "nitro_manage_template"],
]);
const BILLING_PROVIDER_OPERATIONS = new Map([
  ["billing_status", "status"],
  ["billing_plans", "plans"],
  ["plan_checkout", "checkout"],
  ["plan_checkout_status", "checkout_status"],
]);
const DELIVERY_OPERATIONS = new Set([
  "approve", "reject", "live", "schedule", "pause", "resume", "cancel",
  "archive", "restore", "delete",
]);
const QUERY_FILTERS = new Map([
  ["flows", new Set(["id", "status", "campaign_id", "trigger_event", "search"])],
  ["campaigns", new Set(["id", "status", "search"])],
  ["templates", new Set(["id", "subject"])],
  ["segments", new Set(["id", "name"])],
  ["contacts", new Set(["id", "query", "subscribed_email", "subscribed_phone", "list_id"])],
  ["lists", new Set(["id", "name"])],
  ["events", new Set(["id", "name", "from", "to"])],
  ["imports", new Set(["id", "status"])],
  ["messages", new Set(["id", "channel", "status", "to"])],
  ["suppressions", new Set(["id", "email", "reason", "source_provider", "active"])],
  ["history", new Set(["id", "source", "event_type", "tool", "actor", "correlation_id", "resource_uri", "from", "to"])],
  ["products", new Set(["id", "status", "query"])],
]);
const INBOX_ARGUMENTS = new Map([
  ["list_mailbox", new Set(["status", "query", "inbox_id", "view", "include_counts", "page", "per"])],
  ["get_thread", new Set(["conversation_id", "purpose"])],
  ["get_thread_page", new Set(["conversation_id", "before_occurred_at", "before_message_id"])],
  ["get_message_body", new Set(["conversation_id", "message_id", "offset"])],
]);
const SENSITIVE_KEYS = /authorization|api[_-]?key|bearer|credential|secret|token/iu;
const SECRET_VALUE = /\b(?:nskey|wpkey)_(?:live|test)_[A-Za-z0-9_-]+\b/gu;

export function presentEvidence(evidence) {
  return Object.fromEntries(
    ["status_evidence", "plans_evidence", "checkout_evidence", "purchase_evidence"]
      .filter((key) => Object.hasOwn(evidence, key))
      .map((key) => [key, evidence[key]]),
  );
}

export function prepareOperation(inputs) {
  const mode = text(inputs.mode);
  const operation = text(inputs.operation);
  const rawArguments = inputs.arguments;
  const purchaseId = number(inputs.purchase_id);
  const args = operation === "plan_checkout_status" && purchaseId > 0
    ? { purchase_id: purchaseId }
    : record(rawArguments);
  const brandSid = text(inputs.brand_sid);
  const operations = mode === "read" ? READ_OPERATIONS : mode === "act" ? ACT_OPERATIONS : null;
  const blockers = operations
    ? [
        ...(rawArguments !== undefined && !isRecord(rawArguments)
          ? ["arguments must be a JSON object"]
          : []),
        ...validate(mode, operation, args, brandSid),
      ]
    : ["mode must be read or act"];
  const decision = blockers.some((blocker) => blocker.startsWith("refused:"))
    ? "refused"
    : blockers.length > 0
      ? "needs_input"
      : "ready";
  const tool = operation === "compose_email"
    ? COMPOSITION_TOOLS.get(text(args.target_type)) ?? null
    : operations?.get(operation) ?? null;
  const connectorOperation = decision === "ready" ? connectorOperationFor(operation, args) : null;
  return {
    operation_plan: {
      decision: decision === "ready" && !connectorOperation ? "refused" : decision,
      provider: "nitrosend",
      mode,
      operation: operation || null,
      tool,
      connector_operation: connectorOperation,
      // A preview validates without dispatching, so it runs as a read and
      // takes no approval; only a live write does.
      access: connectorOperation
        ? (mode === "act" && !READ_CONNECTOR_OPERATIONS.has(connectorOperation) ? "write" : "read")
        : null,
      provider_input: connectorOperation ? providerInput(operation, args, brandSid) : null,
      expected_provider_operation: BILLING_PROVIDER_OPERATIONS.get(operation) ?? null,
      expected_purchase_id: operation === "plan_checkout_status" ? Number(args.purchase_id) : null,
      brand_sid: brandSid || null,
      blockers: decision === "ready" && !connectorOperation
        ? [`${operation} has no Nitrosend connector operation for these arguments`]
        : blockers.map((blocker) => blocker.replace(/^refused:/u, "")),
    },
  };
}

function connectorOperationFor(operation, args) {
  if (operation === "compose_email") {
    return `${text(args.target_type)}.composition.${COMPOSITION_READS.get(text(args.composition_mode))}`;
  }
  if (operation === "control_delivery") {
    return CONTROL_OPERATIONS.get(`${text(args.target_type)}:${text(args.operation)}`) ?? null;
  }
  const preview = args.dry_run === true;
  if (operation === "send_test_message") return preview ? "test_message.preview" : "test_message.send";
  if (operation === "send_transactional") return preview ? "message.preview" : "message.send";
  if (operation === "import_contacts") return preview ? "contacts.records.preview" : "contacts.records.import";
  if (operation === "define_segment") return args.preview_only === true ? "segment.preview" : "segment.define";
  return CONNECTOR_OPERATIONS.get(operation) ?? null;
}

// The connector operation's input: the brand it runs for, and the tool's
// arguments in the shape the operation declares.
function providerInput(operation, args, brandSid) {
  const brand = brandSid ? { brand_sid: brandSid } : {};
  const provided = providerArguments(operation, args);
  if (["status", "sender_settings", "billing_status", "billing_plans"].includes(operation)) {
    return brand;
  }
  if (operation === "plan_checkout_status") return { ...brand, purchase_id: Number(args.purchase_id) };
  if (operation === "plan_checkout") return { ...brand, plan_id: Number(args.plan_id) };
  // runx derives a live write's provider key from the step's own key.
  if (operation === "control_delivery") {
    const { target_type: _type, operation: _operation, idempotency_key: _key, ...control } = provided;
    return { ...brand, ...defined(control) };
  }
  if (["send_test_message", "send_transactional", "import_contacts"].includes(operation)) {
    const { dry_run: _dryRun, idempotency_key: _key, ...fields } = provided;
    return { ...brand, ...defined(fields) };
  }
  if (operation === "define_segment") {
    const { preview_only: _preview, idempotency_key: _key, ...fields } = provided;
    return { ...brand, ...defined(fields) };
  }
  return { ...brand, arguments: defined(provided) };
}

function defined(value) {
  return Object.fromEntries(Object.entries(record(value)).filter(([, child]) => child !== undefined && child !== null));
}

export function normalizeOperation(inputs) {
  const plan = record(inputs.operation_plan);
  const packet = record(inputs.provider_operation);
  if (packet.status !== "success" || packet.provider !== "nitrosend" ||
      packet.operation !== plan.connector_operation) {
    throw new Error("the Nitrosend result differs from the admitted connector operation");
  }
  try {
    const result = redact(toolResult(record(packet.result), text(plan.operation)));
    const contractBlocker = billingResultBlocker(plan, result);
    const explicitProviderError = result?.error === true || result?.isError === true;
    return {
      provider_evidence: evidence(
        plan,
        explicitProviderError || contractBlocker ? "provider_error" : "ok",
        result,
        explicitProviderError
          ? [result?.message || "Nitrosend rejected the operation"]
          : contractBlocker ? [contractBlocker] : [],
      ),
    };
  } catch (error) {
    return {
      provider_evidence: evidence(
        plan,
        "provider_error",
        null,
        [redactText(error instanceof Error ? error.message : String(error))],
      ),
    };
  }
}

function billingResultBlocker(plan, result) {
  const expectedOperation = text(plan.expected_provider_operation);
  if (!expectedOperation) return null;
  const data = record(result?.data ?? result);
  const actualOperation = text(data.operation);
  if (actualOperation !== expectedOperation) {
    return `Nitrosend returned operation ${actualOperation || "<missing>"}; expected ${expectedOperation}`;
  }
  if (expectedOperation !== "checkout_status") return null;
  const expectedPurchaseId = number(plan.expected_purchase_id);
  const actualPurchaseId = number(data.purchase_id);
  if (actualPurchaseId !== expectedPurchaseId) {
    return `Nitrosend returned purchase_id ${actualPurchaseId || "<missing>"}; expected ${expectedPurchaseId}`;
  }
  return null;
}

export function blockedOperation(inputs) {
  const plan = record(inputs.operation_plan);
  return {
    provider_evidence: evidence(
      plan,
      text(plan.decision) || "needs_input",
      null,
      array(plan.blockers).map(String),
    ),
  };
}

function validate(mode, operation, args, brandSid) {
  const operations = mode === "read" ? READ_OPERATIONS : ACT_OPERATIONS;
  if (!operations.has(operation)) {
    return [`operation must be one of: ${[...operations.keys()].join(", ")}`];
  }
  if (mode === "read" && ["billing_status", "billing_plans"].includes(operation)) {
    if (Object.keys(args).length > 0) {
      return [`refused:${operation} does not accept provider arguments`];
    }
  }
  if (mode === "read" && operation === "query") {
    if (!brandSid) {
      return ["refused:query requires an explicit brand_sid"];
    }
    const allowed = new Set(["entity", "filters", "page", "per"]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    const entityFilters = QUERY_FILTERS.get(args.entity);
    if (unexpected.length > 0) {
      return [`refused:query received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!entityFilters) {
      return [`query requires arguments.entity to be one of: ${[...QUERY_FILTERS.keys()].join(", ")}`];
    }
    if (args.filters !== undefined && !isRecord(args.filters)) {
      return ["query requires arguments.filters as a JSON object"];
    }
    const unsupportedFilters = Object.keys(record(args.filters)).filter((key) => !entityFilters.has(key));
    if (unsupportedFilters.length > 0) {
      return [`refused:query received unsupported ${args.entity} filters: ${unsupportedFilters.join(", ")}`];
    }
    if (args.page !== undefined && !positiveInteger(args.page)) {
      return ["query requires a positive integer arguments.page when supplied"];
    }
    if (args.per !== undefined && (!positiveInteger(args.per) || Number(args.per) > 50)) {
      return ["query requires arguments.per between 1 and 50 when supplied"];
    }
    const oversizedFilter = Object.values(record(args.filters)).find(
      (value) => typeof value === "string" && value.length > 100,
    );
    if (oversizedFilter !== undefined) {
      return ["query filter strings must be at most 100 characters"];
    }
  }
  if (mode === "read" && operation === "inbox") {
    if (!brandSid) {
      return ["refused:inbox requires an explicit brand_sid"];
    }
    const allowed = new Set(["command", "arguments"]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    const commandArguments = INBOX_ARGUMENTS.get(args.command);
    if (unexpected.length > 0) {
      return [`refused:inbox received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!commandArguments) {
      return [`inbox requires arguments.command to be one of: ${[...INBOX_ARGUMENTS.keys()].join(", ")}`];
    }
    if (args.arguments !== undefined && !isRecord(args.arguments)) {
      return ["inbox requires arguments.arguments as a JSON object"];
    }
    const details = record(args.arguments);
    const unsupported = Object.keys(details).filter((key) => !commandArguments.has(key));
    if (unsupported.length > 0) {
      return [`refused:inbox ${args.command} received unsupported fields: ${unsupported.join(", ")}`];
    }
    if (args.command === "list_mailbox") {
      if (details.status !== undefined && !["open", "closed", "archived"].includes(details.status)) {
        return ["inbox list_mailbox status must be open, closed, or archived"];
      }
      if (details.view !== undefined && !["compact", "full"].includes(details.view)) {
        return ["inbox list_mailbox view must be compact or full"];
      }
      if (details.query !== undefined && (typeof details.query !== "string" || details.query.length > 100)) {
        return ["inbox list_mailbox query must be a string up to 100 characters"];
      }
      if (details.inbox_id !== undefined && !positiveInteger(details.inbox_id)) {
        return ["inbox list_mailbox inbox_id must be a positive integer"];
      }
      if (details.include_counts !== undefined && typeof details.include_counts !== "boolean") {
        return ["inbox list_mailbox include_counts must be boolean"];
      }
      if (details.page !== undefined && !positiveInteger(details.page)) {
        return ["inbox list_mailbox page must be a positive integer"];
      }
      const maxPer = details.view === "compact" || details.view === undefined ? 100 : 50;
      if (details.per !== undefined && (!positiveInteger(details.per) || Number(details.per) > maxPer)) {
        return [`inbox list_mailbox per must be between 1 and ${maxPer}`];
      }
    }
    if (["get_thread", "get_thread_page", "get_message_body"].includes(args.command) &&
        !positiveInteger(details.conversation_id)) {
      return [`inbox ${args.command} requires a positive integer conversation_id`];
    }
    if (args.command === "get_thread" && details.purpose !== undefined &&
        !["read", "reply"].includes(details.purpose)) {
      return ["inbox get_thread purpose must be read or reply"];
    }
    if (args.command === "get_thread_page" &&
        (!text(details.before_occurred_at) || !positiveInteger(details.before_message_id))) {
      return ["inbox get_thread_page requires before_occurred_at and a positive integer before_message_id"];
    }
    if (args.command === "get_message_body" && !positiveInteger(details.message_id)) {
      return ["inbox get_message_body requires a positive integer message_id"];
    }
    if (args.command === "get_message_body" &&
        details.offset !== undefined && (!Number.isInteger(Number(details.offset)) || Number(details.offset) < 0)) {
      return ["inbox get_message_body offset must be a non-negative integer"];
    }
  }
  if (mode === "read" && operation === "plan_checkout_status") {
    const allowed = new Set(["purchase_id"]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    if (unexpected.length > 0) {
      return [`refused:plan_checkout_status received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!positiveInteger(args.purchase_id)) {
      return ["plan_checkout_status requires arguments.purchase_id"];
    }
  }
  if (mode === "act" && operation === "plan_checkout") {
    const allowed = new Set(["plan_id", "confirm", "idempotency_key"]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    if (unexpected.length > 0) {
      return [`refused:plan_checkout received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!positiveInteger(args.plan_id) || args.confirm !== true ||
        !text(args.idempotency_key) || text(args.idempotency_key).length > 128) {
      return ["plan_checkout requires a positive plan_id, approved confirm=true, and stable idempotency_key"];
    }
  }
  if (["sender_settings", "configure_sender"].includes(operation) && !brandSid) {
    return ["refused:sender settings require an explicit brand_sid"];
  }
  if (mode === "read" && operation === "sender_settings" && Object.keys(args).length > 0) {
    return ["sender_settings does not accept provider arguments"];
  }
  if (mode === "act" && operation === "configure_sender") {
    const allowed = new Set([
      "from_name",
      "from_email",
      "reply_to",
      "test_email_recipients",
    ]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    if (unexpected.length > 0) {
      return [`configure_sender received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (
      !text(args.from_name) ||
      !email(args.from_email) ||
      !email(args.reply_to) ||
      !Array.isArray(args.test_email_recipients) ||
      args.test_email_recipients.length > 5 ||
      args.test_email_recipients.some((recipient) => !email(recipient))
    ) {
      return [
        "configure_sender requires from_name, valid from_email/reply_to values, and at most five valid test recipients",
      ];
    }
  }
  if (mode === "read" && operation === "insights") {
    const scopes = ["account", "flow", "campaign", "message"];
    if (!scopes.includes(args.scope)) return [`arguments.scope must be one of: ${scopes.join(", ")}`];
    if (args.scope !== "account" && !positiveInteger(args.entity_id)) {
      return [`arguments.entity_id is required for ${args.scope} insights`];
    }
  }
  if (mode === "read" && operation === "review_delivery") {
    if (!["template", "flow", "campaign"].includes(args.target_type) || !positiveInteger(args.target_id)) {
      return ["review_delivery requires a valid target_type and integer target_id"];
    }
    if (args.target_type === "flow" && !positiveInteger(args.revision_id)) {
      return ["review_delivery requires arguments.revision_id for flows"];
    }
  }
  if (mode === "read" && operation === "import_status" && !positiveInteger(args.import_id)) {
    return ["import_status requires arguments.import_id"];
  }
  if (mode === "read" && operation === "compose_email") {
    const allowed = new Set(["target_type", "composition_mode", "arguments"]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    if (unexpected.length > 0) {
      return [`refused:compose_email received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!COMPOSITION_TOOLS.has(args.target_type)) {
      return ["compose_email requires arguments.target_type campaign, flow, or template"];
    }
    if (!["intent", "validate"].includes(args.composition_mode)) {
      return ["compose_email requires arguments.composition_mode intent or validate"];
    }
    if (!isRecord(args.arguments)) {
      return ["compose_email requires arguments.arguments as a JSON object"];
    }
    if (args.composition_mode === "validate" && !text(args.arguments.contract_id)) {
      return ["compose_email validation requires arguments.arguments.contract_id"];
    }
    if (args.composition_mode === "intent" && args.arguments.contract_id !== undefined) {
      return ["compose_email intent must not receive arguments.arguments.contract_id"];
    }
  }
  if (mode === "act" && CREATIVE_DRAFT_OPERATIONS.has(operation)) {
    if (args.composition_mode !== "draft" || !text(args.contract_id) || !text(args.idempotency_key)) {
      return [`refused:${operation} requires persistence-ready composition_mode=draft arguments with contract_id and idempotency_key`];
    }
  }
  if (mode === "act" && operation === "send_transactional") {
    if (!["email", "sms"].includes(args.channel) || !text(args.to)) {
      return ["send_transactional requires channel email or sms and one recipient"];
    }
    if (args.dry_run !== true && !text(args.idempotency_key)) {
      return ["refused:a real transactional send requires arguments.idempotency_key"];
    }
  }
  if (mode === "act" && operation === "send_test_message") {
    const allowed = new Set([
      "target_type", "target_id", "template_id", "action_id", "revision_id",
      "channel", "to", "data", "dry_run", "idempotency_key",
    ]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    if (unexpected.length > 0) {
      return [`refused:send_test_message received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!brandSid) {
      return ["refused:send_test_message requires an explicit brand_sid"];
    }
    if (!["template", "flow", "campaign"].includes(args.target_type) || !positiveInteger(args.target_id)) {
      return ["send_test_message requires an exact target_type and integer target_id"];
    }
    if (args.target_type === "flow" && !positiveInteger(args.revision_id)) {
      return ["send_test_message requires arguments.revision_id for flows"];
    }
    if (args.target_type !== "flow" && args.revision_id !== undefined) {
      return ["refused:send_test_message revision_id is valid only for flow targets"];
    }
    if (args.template_id !== undefined && !positiveInteger(args.template_id)) {
      return ["send_test_message requires a positive integer template_id when supplied"];
    }
    if (args.action_id !== undefined && (!positiveInteger(args.action_id) || args.target_type !== "flow")) {
      return ["send_test_message action_id must be a positive integer on a flow target"];
    }
    if (!["auto", "email", "sms"].includes(args.channel)) {
      return ["send_test_message requires channel auto, email, or sms"];
    }
    if (!Array.isArray(args.to) || args.to.length < 1 || args.to.length > 5 ||
        args.to.some((recipient) => !text(recipient))) {
      return ["send_test_message requires between one and five explicit recipients"];
    }
    if (args.channel === "email" && args.to.some((recipient) => !email(recipient))) {
      return ["send_test_message email recipients must be valid email addresses"];
    }
    if (args.channel === "sms" && args.to.some((recipient) => !/^\+[1-9]\d{7,14}$/u.test(text(recipient)))) {
      return ["send_test_message SMS recipients must use E.164 format"];
    }
    if (args.channel === "auto" && args.to.some(
      (recipient) => !email(recipient) && !/^\+[1-9]\d{7,14}$/u.test(text(recipient)),
    )) {
      return ["send_test_message auto recipients must be valid email addresses or E.164 phone numbers"];
    }
    if (args.data !== undefined && !isRecord(args.data)) {
      return ["send_test_message requires arguments.data as a JSON object when supplied"];
    }
    if (typeof args.dry_run !== "boolean") {
      return ["send_test_message requires an explicit boolean dry_run"];
    }
    if (args.dry_run !== true && (!text(args.idempotency_key) || text(args.idempotency_key).length > 128)) {
      return ["refused:a live test send requires arguments.idempotency_key up to 128 characters"];
    }
  }
  if (mode === "act" && operation === "control_delivery") {
    if (!["flow", "campaign"].includes(args.target_type) || !positiveInteger(args.target_id) || !DELIVERY_OPERATIONS.has(args.operation)) {
      return ["control_delivery requires a valid target_type, integer target_id, and lifecycle operation"];
    }
    if (
      args.target_type === "flow" &&
      ["approve", "reject", "live"].includes(args.operation) &&
      !positiveInteger(args.revision_id)
    ) {
      return [`control_delivery requires arguments.revision_id for flow ${args.operation}`];
    }
    if (args.operation === "schedule" && !text(args.scheduled_at)) {
      return ["scheduled campaign delivery requires arguments.scheduled_at"];
    }
    if (["live", "schedule"].includes(args.operation) && args.target_type === "campaign" && !text(args.idempotency_key)) {
      return ["refused:live or scheduled campaign delivery requires arguments.idempotency_key"];
    }
  }
  if (mode === "act" && operation === "import_contacts") {
    if (!text(args.source_id) || !text(args.consent_basis)) {
      return ["contact imports require arguments.source_id and arguments.consent_basis"];
    }
    if (/purchased|scraped|data\s*broker/iu.test(args.consent_basis)) {
      return ["refused:purchased, scraped, and data-broker contact sources are not permitted"];
    }
    if (args.dry_run !== true && !text(args.idempotency_key)) {
      return ["refused:a real contact import requires arguments.idempotency_key"];
    }
  }
  if (mode === "act" && operation === "ingest_image") {
    const allowed = new Set(["image_url", "description", "filename"]);
    const unexpected = Object.keys(args).filter((key) => !allowed.has(key));
    if (unexpected.length > 0) {
      return [`ingest_image received unsupported fields: ${unexpected.join(", ")}`];
    }
    if (!/^https?:\/\/[^\s]+$/iu.test(text(args.image_url)) || !text(args.description)) {
      return ["ingest_image requires one public http/https image_url and an honest description"];
    }
  }
  return [];
}

function utf8Bytes(value) {
  let bytes = 0;
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x7f) bytes += 1;
    else if (code <= 0x7ff) bytes += 2;
    else if (code >= 0xd800 && code <= 0xdbff &&
             index + 1 < value.length && value.charCodeAt(index + 1) >= 0xdc00 &&
             value.charCodeAt(index + 1) <= 0xdfff) {
      bytes += 4;
      index += 1;
    } else bytes += 3;
  }
  return bytes;
}

// The tool arguments a skill operation's connector input carries; the
// manifest fixes the rest.
function providerArguments(operation, args) {
  if (operation === "inbox") {
    return {
      command: args.command,
      ...record(args.arguments),
      ...(args.command === "get_thread" ? { purpose: args.arguments?.purpose ?? "read" } : {}),
    };
  }
  if (operation === "compose_email") {
    const compositionArguments = { ...record(args.arguments) };
    delete compositionArguments.composition_mode;
    delete compositionArguments.validate_only;
    delete compositionArguments.dry_run;
    delete compositionArguments.idempotency_key;
    return {
      ...compositionArguments,
      composition_mode: args.composition_mode,
      ...(args.composition_mode === "validate" ? { validate_only: true } : {}),
    };
  }
  if (operation === "ingest_image") {
    return {
      kind: "image",
      image_url: args.image_url,
      description: args.description,
      ...(text(args.filename) ? { filename: args.filename } : {}),
    };
  }
  if (operation === "import_status") {
    return { entity: "imports", filters: { id: Number(args.import_id) }, page: 1, per: 1 };
  }
  if (operation !== "import_contacts") return args;
  const { source_id: sourceId, consent_basis: _consentBasis, ...providerArgs } = args;
  if (Array.isArray(providerArgs.records)) {
    providerArgs.records = providerArgs.records.map((entry) => {
      const contact = record(entry);
      return { ...contact, source: contact.source || sourceId };
    });
  }
  return providerArgs;
}

// What a Nitrosend tool returned: its `result`, with the selected sender
// settings spelled out for the sender operations.
function toolResult(value, operation) {
  if (!(isRecord(value.meta) && value.meta.tool && Object.hasOwn(value, "result"))) {
    return providerResult(value);
  }
  const result = record(value.result);
  if (operation === "ingest_image") {
    const { signed_id: _signedId, direct_upload: _directUpload, ...publicResult } = result;
    return publicResult;
  }
  if (!["sender_settings", "configure_sender"].includes(operation)) {
    return result;
  }
  const sender = record(result.sender);
  const currentBrand = record(value.meta.current_brand);
  return {
    ...result,
    current_brand: value.meta.current_brand ?? null,
    sender_settings: {
      brand_sid: text(currentBrand.sid) || null,
      from_name: text(sender.from_name) || null,
      from_email: text(sender.from_email) || null,
      reply_to: text(sender.reply_to) || null,
      test_email_recipients: Array.isArray(result.test_email_recipients)
        ? result.test_email_recipients
        : [],
    },
  };
}

function providerResult(value) {
  if (isRecord(value)) return value;
  throw new Error("Nitrosend returned a non-object tool result");
}

function evidence(plan, decision, result, blockers) {
  return {
    decision,
    provider: "nitrosend",
    mode: text(plan.mode),
    operation: plan.operation ?? null,
    tool: plan.tool ?? null,
    provider_ref: decision === "ok" ? providerReference(text(plan.operation), result) : null,
    result,
    blockers,
  };
}

function providerReference(operation, result) {
  const data = result?.data ?? result;
  if (operation === "send_test_message" && isRecord(data?.target) && data.target.id !== undefined) {
    const revision = positiveInteger(data.revision_id) ? `:revision:${Number(data.revision_id)}` : "";
    return `nitrosend:send_test_message:${text(data.target.type) || "target"}:${data.target.id}${revision}`;
  }
  if (data?.purchase_id !== undefined && data?.purchase_id !== null) {
    return `nitrosend:plan_purchase:${data.purchase_id}`;
  }
  const id = data?.id ?? data?.message_id ?? data?.import_id ?? data?.target_id ?? data?.campaign_id ?? data?.flow_id;
  return id === undefined || id === null ? null : `nitrosend:${operation}:${id}`;
}

function redact(value) {
  if (Array.isArray(value)) return value.map(redact);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, child]) => [
      key,
      SENSITIVE_KEYS.test(key) ? "[REDACTED]" : redact(child),
    ]));
  }
  return typeof value === "string" ? redactText(value) : value;
}

function redactText(value) {
  return String(value).replaceAll(SECRET_VALUE, "[REDACTED]").slice(0, 2_000);
}

function record(value) {
  return isRecord(value) ? value : {};
}

function isRecord(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function array(value) {
  return Array.isArray(value) ? value : [];
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function number(value) {
  return Number.isFinite(Number(value)) ? Number(value) : 0;
}

function positiveInteger(value) {
  return value !== "" && value !== null && value !== undefined && Number.isInteger(Number(value)) && Number(value) > 0;
}

function email(value) {
  return /^[^@\s]+@[^@\s]+\.[^@\s]+$/u.test(text(value));
}
