import {
  eventCitation,
  field,
  isMainModule,
  nowIso,
  requireObject,
  requireString,
  runCli,
  sha256,
} from "./common.mjs";

const HEDGE_RE = /\b(?:might|may|could|possibly|perhaps|suspect(?:ed)?|likely|unlikely|maybe|not sure|unsure|unclear|appears|seems|think|guess|hypothes(?:is|ized)|if)\b/i;
const CAUSE_CUE_RE = /\b(?:cause|caused|causes|causing|because|due to|introduced|root cause|responsible|blame|suspect(?:ed)?)\b/i;
const IMPACT_RE = /\b(?:outage|degrad(?:ed|ation)|error(?:s)?|failure(?:s)?|5xx|latency|slo|affected|impact(?:ed)?|unavailable|timeout|spike|failed checkout|users? saw)\b|\b\d+(?:\.\d+)?\s*%/i;
const MITIGATION_RE = /\b(?:rollback|rolled back|roll back|restor(?:ed|e)|recover(?:ed|y)|resolved|baseline|mitigat(?:ed|ion)|fixed|reverted|stabil(?:ized|ised))\b/i;
const ACTION_RE = /\b(?:action item|follow[- ]?up|todo|next step|should add|must add|add .*?(?:test|alert|guard|monitor)|prevent recurrence)\b/i;
const GENERIC_TOKENS = new Set([
  "a", "an", "and", "are", "as", "at", "be", "because", "by", "caused", "causes", "change", "changed", "deploy", "deployment", "did", "due", "for", "from", "in", "introduced", "is", "it", "of", "on", "or", "release", "root", "that", "the", "this", "to", "was", "were", "with",
]);

function textTokens(value) {
  return new Set(
    value
      .toLowerCase()
      .replace(/v?\d+(?:\.\d+){1,3}/g, " ")
      .replace(/\b\d{1,2}:\d{2}(?:\s*utc)?\b/g, " ")
      .replace(/[^a-z0-9-]+/g, " ")
      .split(/\s+/)
      .filter((token) => token.length > 2 && !GENERIC_TOKENS.has(token)),
  );
}

function candidateSimilar(left, right) {
  const a = textTokens(left);
  const b = textTokens(right);
  if (!a.size || !b.size) return left.toLowerCase() === right.toLowerCase();
  let shared = 0;
  for (const token of a) if (b.has(token)) shared += 1;
  return shared / Math.min(a.size, b.size) >= 0.6;
}

function cleanCandidate(value) {
  return value
    .replace(/^\s*(?:the|a|an)\s+/i, "")
    .replace(/[\s,;:.!?]+$/g, "")
    .trim()
    .slice(0, 800);
}

function extractCause(event) {
  const text = event.text;
  if (!CAUSE_CUE_RE.test(text)) return null;
  const hedged = HEDGE_RE.test(text);
  const patterns = [
    /(?:might|may|could|possibly|perhaps)\s+be\s+(?:the\s+)?(.+?)(?:\s+(?:that|which)\s+(?:caused|led|resulted)|[?.!,]|$)/i,
    /(?:i\s+)?suspect(?:ed)?\s+(?:the\s+)?(.+?)(?:\s+(?:instead|that|which)|[?.!,]|$)/i,
    /(?:root\s+cause\s*(?:is|was|:))\s*(.+?)(?:[.!?]|$)/i,
    /(.+?)\s+introduced\s+(.+?)(?:,|;|\.|$)/i,
    /(.+?)\s+(?:caused|led\s+to|resulted\s+in)\s+(.+?)(?:[.!?]|$)/i,
    /(?:caused\s+by|due\s+to|because\s+of)\s+(.+?)(?:[.!?]|$)/i,
  ];
  let candidate = null;
  for (const pattern of patterns) {
    const match = text.match(pattern);
    if (!match) continue;
    if (pattern.source.includes("introduced")) {
      candidate = cleanCandidate(`${match[1]} introduced ${match[2]}`);
    } else if (pattern.source.includes("caused|led")) {
      candidate = cleanCandidate(match[1]);
    } else {
      candidate = cleanCandidate(match[1]);
    }
    if (candidate && !/^(?:that|this|it|the release|the change)$/i.test(candidate)) break;
  }
  if (!candidate || /^(?:that|this|it|the release|the change)$/i.test(candidate)) candidate = cleanCandidate(text);
  return { event, candidate, hedged };
}

function groupCandidates(candidates) {
  const groups = [];
  for (const candidate of candidates) {
    const group = groups.find((existing) => candidateSimilar(existing.candidate, candidate.candidate));
    if (group) {
      group.items.push(candidate);
    } else {
      groups.push({ candidate: candidate.candidate, items: [candidate] });
    }
  }
  return groups;
}

function classifyEvent(event, cause) {
  if (cause) return cause.hedged ? "hypothesis" : "root_cause_claim";
  if (ACTION_RE.test(event.text)) return "action_item";
  if (MITIGATION_RE.test(event.text)) return "mitigation";
  if (IMPACT_RE.test(event.text)) return "impact";
  return "observation";
}

function confidenceFor(event, cause) {
  if (cause?.hedged) return "low";
  if (cause) return "high";
  if (IMPACT_RE.test(event.text) || MITIGATION_RE.test(event.text)) return "medium";
  return "source_only";
}

function extractAction(event, index) {
  if (!ACTION_RE.test(event.text)) return null;
  const ownerMatch = event.text.match(/\bowner\s*[:=]\s*([A-Za-z0-9_.@/-]+)/i);
  const owner = ownerMatch ? ownerMatch[1].replace(/[.,;:]+$/g, "") : null;
  const actionText = event.text
    .replace(/^\s*(?:action item|follow[- ]?up|todo|next step)\s*[:=-]?\s*/i, "")
    .replace(/\s*\bowner\s*[:=]\s*[A-Za-z0-9_.@/-]+\.?\s*$/i, "")
    .trim();
  return {
    id: `action-${index + 1}`,
    title: actionText || event.text,
    owner,
    evidence: [eventCitation(event)],
    policy_rule: owner ? "source_explicit" : "owner_not_stated",
  };
}

function normalizePolicy(value) {
  if (value === undefined || value === null) return {};
  const policy = requireObject(value, "postmortem_policy");
  if (Object.prototype.hasOwnProperty.call(policy, "allow_publish") && typeof policy.allow_publish !== "boolean") {
    throw new Error("postmortem_policy.allow_publish must be boolean");
  }
  if (Object.prototype.hasOwnProperty.call(policy, "require_confirmed_root_cause") && typeof policy.require_confirmed_root_cause !== "boolean") {
    throw new Error("postmortem_policy.require_confirmed_root_cause must be boolean");
  }
  if (Object.prototype.hasOwnProperty.call(policy, "require_action_items") && typeof policy.require_action_items !== "boolean") {
    throw new Error("postmortem_policy.require_action_items must be boolean");
  }
  return policy;
}

function normalizeTarget(value) {
  if (value === undefined || value === null) {
    return {
      data_source_ref: "local://runx-postmortem-maker/default",
      channel: "incident-review",
      aggregate_id: "default",
      principal: "postmortem-maker",
      audience: "incident-review",
      classification: "internal",
      visibility: "operator",
    };
  }
  const target = requireObject(value, "publish_target");
  const dataSourceRef = requireString(target.data_source_ref, "publish_target.data_source_ref", { maxLength: 500 });
  const channel = requireString(target.channel, "publish_target.channel", { maxLength: 300 });
  const aggregateId = requireString(target.aggregate_id, "publish_target.aggregate_id", { maxLength: 300 });
  return {
    data_source_ref: dataSourceRef,
    channel,
    aggregate_id: aggregateId,
    principal: typeof target.principal === "string" && target.principal.trim() ? target.principal.trim() : "postmortem-maker",
    audience: typeof target.audience === "string" && target.audience.trim() ? target.audience.trim() : channel,
    classification: typeof target.classification === "string" && target.classification.trim() ? target.classification.trim() : "internal",
    visibility: typeof target.visibility === "string" && target.visibility.trim() ? target.visibility.trim() : "operator",
  };
}

function reasonForBlock({ rootStatus, unknowns, policy, actions }) {
  if (rootStatus !== "confirmed") return "root cause is not confirmed by one unhedged source claim";
  if (unknowns.length) return "unresolved facts remain in unknowns";
  if (policy.require_action_items && actions.length === 0) return "policy requires at least one source-owned action item";
  if (policy.allow_publish !== true) return "publication requires postmortem_policy.allow_publish=true";
  return "evidence and publication policy passed";
}

async function handler(envelope) {
  const incident = requireObject(field(envelope, "incident"), "incident");
  const events = Array.isArray(incident.events) ? incident.events : [];
  if (!events.length) throw new Error("incident must contain at least one event");
  const policy = normalizePolicy(field(envelope, "postmortem_policy"));
  const target = normalizeTarget(field(envelope, "publish_target"));
  const causeClaims = events.map(extractCause).filter(Boolean);
  const groups = groupCandidates(causeClaims);
  const unhedgedGroups = groups.filter((group) => group.items.some((item) => !item.hedged));

  let rootCause;
  let rootCauseStatus;
  if (unhedgedGroups.length === 1 && groups.length === 1) {
    const selected = unhedgedGroups[0];
    const citations = selected.items.map((item) => eventCitation(item.event));
    rootCauseStatus = "confirmed";
    rootCause = {
      status: "confirmed",
      statement: selected.candidate,
      citations,
      corroborated_by_mitigation: events.some((event) => MITIGATION_RE.test(event.text)),
    };
  } else {
    rootCauseStatus = "unconfirmed";
    rootCause = {
      status: "unconfirmed",
      statement: null,
      citations: causeClaims.map((item) => eventCitation(item.event)),
      corroborated_by_mitigation: false,
    };
  }

  const timeline = events.map((event) => {
    const cause = causeClaims.find((item) => item.event.id === event.id) ?? null;
    return {
      at: event.at,
      statement: event.text,
      kind: classifyEvent(event, cause),
      confidence: confidenceFor(event, cause),
      evidence: eventCitation(event),
    };
  });

  const impactEvent = events.find((event) => IMPACT_RE.test(event.text));
  const mitigationEvent = events.find((event) => MITIGATION_RE.test(event.text));
  const impact = impactEvent
    ? { status: "known", statement: impactEvent.text, citations: [eventCitation(impactEvent)] }
    : { status: "unknown", statement: null, citations: [] };
  const actions = events.map(extractAction).filter(Boolean);
  const unknowns = [];
  if (!impactEvent) {
    unknowns.push({ id: "unknown-impact", detail: "The source does not state a concrete customer or service impact.", evidence: [] });
  }
  if (!mitigationEvent) {
    unknowns.push({ id: "unknown-mitigation", detail: "The source does not state whether mitigation or recovery completed.", evidence: [] });
  }
  if (rootCauseStatus !== "confirmed") {
    unknowns.push({
      id: "unknown-root-cause",
      detail: groups.length > 1 ? "Multiple causal candidates remain in the source; the root cause is unresolved." : "The source contains no unhedged, confirmed causal claim.",
      evidence: causeClaims.map((item) => eventCitation(item.event)),
    });
  }
  if (policy.require_action_items && actions.length === 0) {
    unknowns.push({ id: "unknown-action-owner", detail: "The publication policy requires at least one source-owned action item.", evidence: [] });
  }

  const evidenceReady = rootCauseStatus === "confirmed" && unknowns.length === 0;
  const publishable = evidenceReady && policy.allow_publish === true;
  const decision = publishable ? "publishable" : "needs_more_evidence";
  const postmortem = {
    summary: `Incident ${incident.title ?? "without a title"} is reconstructed from ${events.length} source event(s).`,
    timeline,
    impact,
    root_cause: rootCause,
    status: decision,
  };
  const postmortemDigest = sha256(postmortem);
  const sourceDigest = incident.source?.source_digest ?? sha256({ title: incident.title, events });
  const stableSource = {
    kind: incident.source?.kind ?? "incident_thread",
    ref: incident.source?.ref ?? "unknown",
    read_mode: incident.source?.read_mode ?? "unknown",
    events_read: incident.source?.events_read ?? events.length,
    source_digest: sourceDigest,
  };
  const content = {
    title: `Postmortem: ${incident.title ?? "Untitled incident"}`,
    source: stableSource,
    postmortem,
    unknowns,
    action_items: actions,
  };
  const contentDigest = sha256(content);
  const idempotencyKey = `postmortem-maker:${sourceDigest}:${sha256(target.aggregate_id).slice(7, 23)}`;
  const sendPlan = publishable
    ? {
        schema: "runx.send-as.plan.v1",
        status: "authorized",
        action_family: "send-as",
        principal: { type: "account", ref: target.principal },
        provider: { name: "local-outbox", account_ref: target.data_source_ref, runtime_path: "append" },
        send_class: "incident-postmortem",
        channel: { type: "channel", ref: target.channel },
        audience: { type: "audience", ref: target.audience },
        content: { digest: contentDigest, subject_or_title: content.title },
        consent_basis: "postmortem_policy.allow_publish",
        gates: { preflight_required: true, human_approval_required: false, approval_ref: "policy:allow_publish" },
        provider_actions: ["outbox.append"],
        evidence_refs: [sourceDigest, postmortemDigest],
        transport: { kind: "equivalent_sealed_comms_transport", skill: "send-as" },
      }
    : {
        schema: "runx.send-as.plan.v1",
        status: "withheld",
        action_family: "send-as",
        reason: reasonForBlock({ rootStatus: rootCauseStatus, unknowns, policy, actions }),
        consent_basis: "postmortem_policy.allow_publish",
        provider_actions: [],
        evidence_refs: [sourceDigest, postmortemDigest],
      };

  return {
    postmortem,
    fetched_at: incident.source?.observed_at ?? nowIso(),
    unknowns,
    action_items: actions,
    root_cause_status: rootCauseStatus,
    timeline_count: timeline.length,
    publishable,
    decision,
    send_plan: sendPlan,
    content_digest: contentDigest,
    postmortem_digest: postmortemDigest,
    source_digest: sourceDigest,
    idempotency_key: idempotencyKey,
    expected_version: Number.isInteger(field(envelope, "outbox")?.version) ? field(envelope, "outbox").version : 0,
    message: publishable ? content : null,
    delivery_result: null,
  };
}

if (isMainModule(import.meta.url)) {
  runCli(handler);
}

export { handler as composePostmortem };
