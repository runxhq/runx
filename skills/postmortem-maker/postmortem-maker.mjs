const SCHEMA = "runx.postmortem.v2";
const SEND_PLAN_SCHEMA = "runx.send_plan.v1";
const PUBLISH_SCHEMA = "runx.publish_result.v1";
const DELIVERY_SCHEMA = "runx.delivery_draft.v1";

function asRecord(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function asText(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function dedupeStrings(list) {
  if (!Array.isArray(list)) return [];
  const seen = new Set();
  const out = [];
  for (const item of list) {
    const text = asText(item);
    if (text && !seen.has(text)) {
      seen.add(text);
      out.push(text);
    }
  }
  return out;
}

function requireDigest(value) {
  if (typeof value !== "string" || !/^sha256:[0-9a-f]{64}$/.test(value)) {
    throw new Error("fragments digest evidence missing or malformed");
  }
  return value;
}

function verifyCitation(target, fragmentsById, findings, label) {
  const fragmentId = asText(target.fragment_id);
  const quote = asText(target.quote);
  if (!fragmentId || !fragmentsById.has(fragmentId)) {
    findings.push({ code: `${label}.unknown_fragment`, message: `${label} cites a fragment that was not read from the source.` });
    return null;
  }
  if (!quote) {
    findings.push({ code: `${label}.missing_quote`, message: `${label} carries no verbatim quote from its fragment.` });
    return null;
  }
  const source = fragmentsById.get(fragmentId);
  const text = typeof source.text === "string" ? source.text : "";
  if (!text.includes(quote)) {
    findings.push({ code: `${label}.quote_not_in_source`, message: `${label} quote is not present verbatim in fragment ${fragmentId}.` });
    return null;
  }
  return { fragmentId, quote };
}

export function finalizePostmortem(inputs) {
  const fragments = (Array.isArray(inputs.incident_fragments) ? inputs.incident_fragments : []).map(asRecord);
  const draft = asRecord(inputs.postmortem_draft);
  const policy = asRecord(inputs.postmortem_policy);
  const byId = new Map();
  for (const fragment of fragments) {
    const id = asText(fragment.id);
    if (id) byId.set(id, fragment);
  }
  const findings = [];

  if (fragments.length === 0) {
    findings.push({ code: "source.empty", message: "the incident source read produced zero fragments." });
  }

  const timeline = [];
  for (const raw of Array.isArray(draft.timeline) ? draft.timeline : []) {
    const entry = asRecord(raw);
    const cited = verifyCitation(entry, byId, findings, "timeline");
    if (cited) {
      timeline.push({ entry: asText(entry.entry) ?? "", fragment_id: cited.fragmentId, quote: cited.quote });
    }
  }
  if (timeline.length === 0 && !findings.some((f) => f.code === "source.empty")) {
    findings.push({ code: "timeline.empty", message: "no timeline entry survived citation enforcement." });
  }

  const rootCauseDraft = asRecord(draft.root_cause);
  const rootStatus = asText(rootCauseDraft.status);
  if (!["known", "suspected", "unknown"].includes(rootStatus ?? "")) {
    findings.push({ code: "root_cause.status_invalid", message: "root_cause.status must be known, suspected, or unknown." });
  } else if (rootStatus !== "unknown") {
    if (!asText(rootCauseDraft.statement)) {
      findings.push({ code: "root_cause.statement_missing", message: `a ${rootStatus} root cause needs a statement.` });
    }
    verifyCitation(rootCauseDraft, byId, findings, "root_cause");
  }

  const unknowns = dedupeStrings(draft.unknowns);
  const actionItems = [];
  for (const raw of Array.isArray(draft.action_items) ? draft.action_items : []) {
    const item = asRecord(raw);
    const action = asText(item.action);
    const owner = asText(item.owner);
    if (action && owner) {
      actionItems.push({ action, owner });
    } else {
      findings.push({ code: "action_item.incomplete", message: "action items require both action and owner." });
    }
  }

  const impactDraft = asRecord(draft.impact);
  const invalid = findings.length > 0;
  const settled = !invalid && rootStatus !== "unknown" && unknowns.length === 0;
  const decision = invalid ? "refused" : settled ? "publishable" : "needs_more_evidence";
  const publishRequested = policy.allow_publish === true;

  const sendPlan =
    settled && publishRequested
      ? { schema: SEND_PLAN_SCHEMA, status: "ready", transport: "sealed-outbox", gate: "human-approver", bound_to: "postmortem" }
      : { schema: SEND_PLAN_SCHEMA, status: "withheld", transport: "sealed-outbox" };

  return {
    postmortem: {
      schema: SCHEMA,
      decision,
      reason: invalid
        ? "Refused: the draft asserts facts the source evidence does not contain."
        : settled
          ? "Every claim is source-cited and no unknowns remain open."
          : "Honest but incomplete: unresolved unknowns block publication.",
      status: invalid ? "refused" : "sealed",
      incident_ref: asText(inputs.incident_ref),
      summary: invalid ? null : asText(draft.summary),
      timeline: invalid ? [] : timeline,
      impact: invalid
        ? null
        : { statement: asText(impactDraft.statement), scope: asText(impactDraft.scope) },
      root_cause: invalid
        ? null
        : {
            status: rootStatus,
            statement: rootStatus === "unknown" ? null : asText(rootCauseDraft.statement),
            fragment_id: rootStatus === "unknown" ? null : asText(rootCauseDraft.fragment_id),
          },
      unknowns,
      action_items: invalid ? [] : actionItems,
      send_plan: sendPlan,
      fragments_digest: requireDigest(inputs.fragments_digest),
      validation: { status: invalid ? "fail" : "pass", findings },
    },
  };
}

export function prepareDelivery(inputs) {
  const postmortem = asRecord(inputs.postmortem);
  const policy = asRecord(inputs.postmortem_policy);
  const ref = (asText(postmortem.incident_ref) ?? "incident").replace(/[^A-Za-z0-9._-]/g, "_");
  const dir = (asText(policy.outbox_dir) ?? "outbox/postmortems").replace(/\/+$/, "");
  return {
    delivery_draft: {
      schema: DELIVERY_SCHEMA,
      transport: "sealed-outbox",
      path: `${dir}/${ref}.postmortem.json`,
      contents: `${JSON.stringify(postmortem, null, 2)}\n`,
      bound_digest: asText(postmortem.fragments_digest),
    },
  };
}

export function recordDelivery(inputs) {
  const postmortem = asRecord(inputs.postmortem);
  const draft = asRecord(inputs.delivery_draft);
  const writeResult = asRecord(inputs.write_result);
  const path = asText(draft.path);
  const executed = Boolean(path) && writeResult !== null && typeof writeResult === "object";
  return {
    publish_result: {
      schema: PUBLISH_SCHEMA,
      executed,
      transport: "sealed-outbox",
      delivery_path: path,
      bound_digest: asText(postmortem.fragments_digest),
      send_plan_status: "executed",
      postmortem_decision: asText(postmortem.decision),
    },
  };
}
