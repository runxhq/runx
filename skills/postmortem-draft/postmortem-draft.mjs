export function finalizePostmortem(inputs) {
  const fragments = (Array.isArray(inputs.incident_fragments) ? inputs.incident_fragments : []).map(record);
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

  const failed = findings.length > 0;
  const publishable = !failed && rootStatus !== "unknown" && unknowns.length === 0;
  const decision = failed ? "refused" : publishable ? "publishable" : "needs_more_evidence";
  return {
    postmortem: {
      schema: "runx/postmortem-draft.postmortem.v1",
      decision,
      reason: failed
        ? "Refused: the draft claims facts the supplied fragments do not support."
        : publishable
          ? "Every timeline entry and the root cause are fragment-cited with no open unknowns."
          : "The postmortem is evidence-grounded but incomplete; unknowns remain and nothing publishes.",
      summary: failed ? null : stringValue(draft.summary),
      timeline: failed ? [] : validTimeline,
      root_cause: failed
        ? null
        : {
            status: rootStatus,
            statement: rootStatus === "unknown" ? null : stringValue(rootCause.statement),
            fragment_id: rootStatus === "unknown" ? null : stringValue(rootCause.fragment_id),
          },
      unknowns,
      action_items: failed ? [] : actionItems,
      publish_proposal: publishable
        ? { gate: "human-approver", delivery_skill: "message-send", sent: false }
        : null,
      publish_performed: false,
      fragments_digest: requiredDigest(inputs.fragments_digest),
      validation: { status: failed ? "fail" : "pass", findings },
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
