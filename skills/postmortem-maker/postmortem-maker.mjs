export function buildFragments(fields) {
  const fetched = record(fields.fetch_result);
  const decision = stringValue(fetched.decision);
  if (decision !== "ready") {
    throw new Error(`incident source read failed: ${decision ?? "unknown"}`);
  }
  const status = Number(fetched.status);
  if (!Number.isFinite(status) || status < 200 || status >= 300) {
    throw new Error(`incident source returned HTTP ${fetched.status}`);
  }
  const text = typeof fetched.extracted === "string" ? fetched.extracted : "";
  const source =
    stringValue(fetched.final_url) ??
    stringValue(fields.incident_source) ??
    "incident-source";
  const normalized = text.replace(/\s+/g, " ").trim();
  let units = splitSentences(normalized)
    .map((unit) => unit.trim())
    .filter(Boolean)
    .flatMap((unit) => (unit.length > 1200 ? splitLong(unit) : [unit]));
  if (units.length === 0 && normalized) units = [normalized];
  if (units.length === 0) {
    throw new Error("incident source carried no readable incident record");
  }
  const fragments = units.slice(0, 60).map((unit, index) => ({
    id: `frag-${index + 1}`,
    source,
    text: unit,
  }));
  return {
    fragment_set: { incident_ref: source, incident_fragments: fragments },
  };
}

// Plain scanner: the runtime's deterministic worker runs Boa, whose regex
// engine does not support lookbehind, so sentence boundaries are found by
// hand. A boundary is [.!?] followed by whitespace or end of input, which
// keeps dotted identifiers (2026.10.05.1, 06:59:45.034Z) inside one unit.
function splitSentences(normalized) {
  const out = [];
  let start = 0;
  for (let i = 0; i < normalized.length; i += 1) {
    const ch = normalized.charAt(i);
    if (ch !== "." && ch !== "!" && ch !== "?") continue;
    const next = i + 1 < normalized.length ? normalized.charAt(i + 1) : "";
    if (next && next !== " ") continue;
    const chunk = normalized.slice(start, i + 1).trim();
    if (chunk) out.push(chunk);
    start = i + 1;
  }
  const tail = normalized.slice(start).trim();
  if (tail) out.push(tail);
  return out;
}

function splitLong(unit) {
  return unit
    .split(";")
    .map((part) => part.trim())
    .filter(Boolean);
}

export function composeContent(fields) {
  const postmortem = record(fields.postmortem);
  const digest = stringValue(fields.postmortem_digest);
  if (!digest || !digest.startsWith("sha256:")) {
    throw new Error("postmortem digest evidence is missing");
  }
  const summary = stringValue(postmortem.summary) ?? "Postmortem";
  return {
    content_ref: {
      draft_ref: `postmortem:${stringValue(postmortem.decision) ?? "unknown"}`,
      digest,
      subject_or_title: `Postmortem: ${summary.slice(0, 120)}`,
    },
  };
}

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

  const impact = stringValue(draft.impact);
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
  const publishable = !failed && rootStatus !== "unknown" && unknowns.length === 0 && !!impact;
  const decision = failed ? "refused" : publishable ? "publishable" : "needs_more_evidence";
  const reason = failed
    ? "Refused: the draft claims facts the supplied fragments do not support."
    : publishable
      ? "Every timeline entry and the root cause are fragment-cited, the impact is stated, and no unknowns remain."
      : "The postmortem is evidence-grounded but incomplete; unknowns, a missing impact, or an unresolved root cause keep it from publishing.";

  return {
    postmortem: {
      schema: "runx.postmortem.v1",
      decision,
      status: decision,
      reason,
      summary: failed ? null : stringValue(draft.summary),
      timeline: failed ? [] : validTimeline,
      impact: failed || !impact ? null : impact,
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
        ? { gate: "human-approver", delivery_skill: "send-as", sent: false }
        : null,
      publish_performed: false,
      publish_result: null,
      fragments_digest: requiredDigest(inputs.fragments_digest),
      validation: { status: failed ? "fail" : "pass", findings },
    },
    publish_path: { path: publishable ? "publish" : "hold", reason },
  };
}

export function stampHeld(fields) {
  return { postmortem: record(fields.postmortem) };
}

export function stampPublished(fields) {
  const postmortem = record(fields.postmortem);
  const sendResult = record(fields.send_result);
  const status = stringValue(sendResult.status);
  if (status !== "sent") {
    throw new Error(`publish did not reach provider readback: ${status ?? "missing send_result"}`);
  }
  const proposal = record(postmortem.publish_proposal);
  return {
    postmortem: {
      ...postmortem,
      publish_proposal: {
        gate: proposal.gate ?? "human-approver",
        delivery_skill: "send-as",
        sent: true,
      },
      publish_performed: true,
      publish_result: {
        schema: stringValue(sendResult.schema) ?? "runx.send_as.result.v1",
        status,
        outcome: stringValue(sendResult.outcome),
        provider: stringValue(sendResult.provider),
        target: stringValue(sendResult.target),
        operation: stringValue(sendResult.operation),
        operation_id: stringValue(sendResult.operation_id),
        readback_ref: stringValue(sendResult.readback_ref),
        idempotency_key: stringValue(sendResult.idempotency_key),
        content_digest: stringValue(sendResult.content_digest),
      },
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
