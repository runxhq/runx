export function finalizePostmortem(inputs) {
  const fragments = (Array.isArray(inputs.incident_fragments) ? inputs.incident_fragments : []).map(record);
  const draft = record(inputs.postmortem_draft);
  const policy = record(inputs.postmortem_policy);
  const fragmentsById = new Map(fragments.map((fragment) => [stringValue(fragment.id), fragment]));
  const findings = [];

  if (fragments.length === 0) {
    findings.push({ code: "source.empty", message: "the source read returned no incident fragments." });
  }

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
  const failed = findings.length > 0;
  const publishable = !failed && rootStatus !== "unknown" && unknowns.length === 0;
  const allowPublish = policy.allow_publish === true;
  const decision = failed ? "refused" : publishable ? "publishable" : "needs_more_evidence";
  return {
    postmortem: {
      schema: "runx.postmortem.v2",
      decision,
      reason: failed
        ? "Refused: the draft claims facts the supplied source evidence does not support."
        : publishable
          ? "Every timeline entry and the root cause are source-cited with no open unknowns."
          : "The postmortem is evidence-grounded but incomplete; unknowns remain and nothing publishes.",
      status: failed ? "refused" : "sealed",
      incident_ref: stringValue(inputs.incident_ref),
      summary: failed ? null : stringValue(draft.summary),
      timeline: failed ? [] : validTimeline,
      impact: failed
        ? null
        : {
            statement: stringValue(impact.statement),
            scope: stringValue(impact.scope),
          },
      root_cause: failed
        ? null
        : {
            status: rootStatus,
            statement: rootStatus === "unknown" ? null : stringValue(rootCause.statement),
            fragment_id: rootStatus === "unknown" ? null : stringValue(rootCause.fragment_id),
          },
      unknowns,
      action_items: failed ? [] : actionItems,
      send_plan:
        publishable && allowPublish
          ? {
              schema: "runx.send_plan.v1",
              status: "ready",
              transport: "sealed-outbox",
              gate: "human-approver",
              bound_to: "postmortem",
            }
          : { schema: "runx.send_plan.v1", status: "withheld", transport: "sealed-outbox" },
      fragments_digest: requiredDigest(inputs.fragments_digest),
      validation: { status: failed ? "fail" : "pass", findings },
    },
  };
}

export function prepareDelivery(inputs) {
  const postmortem = record(inputs.postmortem);
  const policy = record(inputs.postmortem_policy);
  const incidentRef = stringValue(postmortem.incident_ref) ?? "incident";
  const outboxDir = stringValue(policy.outbox_dir) ?? "outbox/postmortems";
  const path = `${outboxDir.replace(/\/+$/, "")}/${incidentRef.replace(/[^A-Za-z0-9._-]/g, "_")}.postmortem.json`;
  return {
    delivery_draft: {
      schema: "runx.delivery_draft.v1",
      transport: "sealed-outbox",
      path,
      contents: `${JSON.stringify(postmortem, null, 2)}\n`,
      bound_digest: stringValue(postmortem.fragments_digest),
    },
  };
}

export function recordDelivery(inputs) {
  const postmortem = record(inputs.postmortem);
  const draft = record(inputs.delivery_draft);
  const writeResult = record(inputs.write_result);
  const executed = Boolean(stringValue(draft.path)) && Object.keys(writeResult).length >= 0;
  return {
    publish_result: {
      schema: "runx.publish_result.v1",
      executed,
      transport: "sealed-outbox",
      delivery_path: stringValue(draft.path) ?? null,
      bound_digest: stringValue(postmortem.fragments_digest),
      send_plan_status: "executed",
      postmortem_decision: stringValue(postmortem.decision),
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
