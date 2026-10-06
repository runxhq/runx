export function finalizePostmortem(inputs) {
  const fragments = (Array.isArray(inputs.incident_fragments) ? inputs.incident_fragments : []).map(fragmentRecord);
  const draft = record(inputs.postmortem_draft);
  const fragmentsById = new Map(fragments.map((fragment) => [stringValue(fragment.id), fragment]));
  const findings = [];

  const timeline = (Array.isArray(draft.timeline) ? draft.timeline : []).map(record);
  const validTimeline = [];
  for (const entry of timeline) {
    const cited = citedQuote(entry, fragmentsById);
    if (!cited.ok) {
      findings.push({ code: "timeline.unsupported", message: `timeline entry ${JSON.stringify(stringValue(entry.entry) ?? "")} ${cited.reason}.` });
      continue;
    }
    validTimeline.push({
      entry: stringValue(entry.entry) ?? "",
      fragment_id: cited.fragmentId,
      quote: cited.quote,
    });
  }
  if (validTimeline.length === 0) {
    findings.push({ code: "timeline.empty", message: "the draft carries no evidence-cited timeline entries." });
  }

  const rootCause = record(draft.root_cause);
  const rootStatus = stringValue(rootCause.status);
  if (!["known", "suspected", "unknown"].includes(rootStatus ?? "")) {
    findings.push({ code: "root_cause.invalid", message: "root_cause.status must be known, suspected, or unknown." });
  } else if (rootStatus !== "unknown") {
    const cited = citedQuote(rootCause, fragmentsById);
    if (!stringValue(rootCause.statement)) {
      findings.push({ code: "root_cause.invalid", message: `a ${rootStatus} root cause must carry a statement.` });
    } else if (!cited.ok) {
      findings.push({ code: "root_cause.unsupported", message: `the ${rootStatus} root cause ${cited.reason}.` });
    }
  }

  const unknowns = uniqueStrings(draft.unknowns);
  const actionItems = (Array.isArray(draft.action_items) ? draft.action_items : []).map(record).flatMap((item) => {
    const action = stringValue(item.action);
    const owner = stringValue(item.owner);
    if (!action || !owner) {
      findings.push({ code: "action_item.incomplete", message: "every action item needs an action and an owner." });
      return [];
    }
    return [{ action, owner }];
  });

  const impact = record(draft.impact);
  const invalid = findings.length > 0;
  const settled = !invalid && rootStatus !== "unknown" && unknowns.length === 0;
  const policy = record(inputs.postmortem_policy);
  const allowPublish = policy.allow_publish !== false;
  const sendPlan = {
    schema: "runx.send_plan.v1",
    status: settled && allowPublish ? "ready" : "withheld",
    transport: "sealed-outbox",
    gate: settled && allowPublish ? "human-approver" : null,
    bound_to: settled && allowPublish ? `postmortem:${stringValue(inputs.incident_ref) ?? ""}` : null,
  };

  return {
    postmortem: {
      schema: "runx.postmortem.v2",
      decision: invalid ? "refused" : settled ? "publishable" : "needs_more_evidence",
      reason: invalid
        ? "Refused: the draft asserts facts the source evidence does not contain."
        : settled
          ? "Every claim is source-cited and no unknowns remain open."
          : "Honest but incomplete: unresolved unknowns block publication.",
      status: invalid ? "refused" : "sealed",
      incident_ref: stringValue(inputs.incident_ref),
      summary: invalid ? null : stringValue(draft.summary),
      timeline: invalid ? [] : validTimeline,
      impact: invalid
        ? null
        : {
            statement: stringValue(impact.statement),
            scope: stringValue(impact.scope),
          },
      root_cause: invalid
        ? null
        : {
            status: rootStatus,
            statement: rootStatus === "unknown" ? null : stringValue(rootCause.statement),
            fragment_id: rootStatus === "unknown" ? null : stringValue(rootCause.fragment_id),
          },
      unknowns,
      action_items: invalid ? [] : actionItems,
      send_plan: sendPlan,
      fragments_digest: requiredDigest(inputs.fragments_digest),
      validation: { status: invalid ? "fail" : "pass", findings },
    },
  };
}

export function prepareDelivery(inputs) {
  const postmortem = record(inputs.postmortem);
  const policy = record(inputs.postmortem_policy);
  const ref = (stringValue(inputs.incident_ref) ?? "incident").replace(/[^A-Za-z0-9._-]/g, "_");
  const dir = (stringValue(policy.outbox_dir) ?? "outbox/postmortems").replace(/\/+$/, "");
  return {
    delivery_draft: {
      schema: "runx.sealed_outbox.v1",
      transport: "sealed-outbox",
      path: `${dir}/${ref}.postmortem.json`,
      contents: `${JSON.stringify(postmortem, null, 2)}\n`,
      bound_digest: stringValue(postmortem.fragments_digest),
    },
  };
}

export function recordDelivery(inputs) {
  const postmortem = record(inputs.postmortem);
  const delivery = record(inputs.delivery_draft);
  return {
    publish_result: {
      schema: "runx.publish_result.v1",
      executed: true,
      transport: "sealed-outbox",
      send_plan_status: "executed",
      path: stringValue(delivery.path),
      fragments_digest: stringValue(postmortem.fragments_digest),
    },
  };
}

function citedQuote(entry, fragmentsById) {
  const fragmentId = stringValue(entry.fragment_id);
  const quote = stringValue(entry.quote);
  const fragment = fragmentsById.get(fragmentId);
  if (!fragment) return { ok: false, reason: "cites an unknown fragment" };
  if (!quote) return { ok: false, reason: "carries no supporting quote" };
  const text = typeof fragment.text === "string" ? fragment.text : "";
  if (!text.includes(quote)) return { ok: false, reason: "cites a quote that is not present in its fragment" };
  return { ok: true, fragmentId, quote };
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function fragmentRecord(value) {
  const v = record(value);
  return {
    id: stringValue(v.id),
    source: stringValue(v.source),
    text: typeof v.text === "string" ? v.text : "",
  };
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function uniqueStrings(value) {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map(stringValue).filter(Boolean))];
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}
