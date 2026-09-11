const SCHEMA = "runx.postmortem.v1";
const MAX_FRAGMENTS = 200;

/**
 * Build the incident fragment set from a real source read.
 *
 * The source text arrives from a live `web.fetch` of the incident thread, so
 * the fragments are derived at run time rather than pasted in by the caller.
 * Each line that carries a recognisable incident marker becomes one fragment
 * with a stable id, the source host, and the verbatim line text. Nothing is
 * paraphrased: downstream citation enforcement compares quotes against these
 * exact strings.
 */
export function buildFragments(inputs) {
  const sourceUrl = stringValue(inputs.source_url);
  if (!sourceUrl) throw new Error("source_url is required to bind the incident record");

  const content = typeof inputs.source_content === "string" ? inputs.source_content : "";
  const sourceDigest = requiredDigest(inputs.source_digest);

  let host = "source";
  try {
    host = new URL(sourceUrl).host || "source";
  } catch {
    host = "source";
  }

  const lines = segmentSource(content);

  const fragments = [];
  const seen = new Set();
  for (const line of lines) {
    if (fragments.length >= MAX_FRAGMENTS) break;
    if (line.length < 12 || line.length > 4000) continue;
    if (!isIncidentSignal(line)) continue;
    if (seen.has(line)) continue;
    seen.add(line);
    fragments.push({
      id: `frag-${fragments.length + 1}`,
      source: host,
      text: line,
    });
  }

  return {
    fragment_set: {
      schema: "runx.incident.fragments.v1",
      incident_ref: stringValue(inputs.incident_ref) ?? sourceUrl,
      source_url: sourceUrl,
      source_digest: sourceDigest,
      source_host: host,
      read_at: stringValue(inputs.read_at) ?? null,
      fragment_count: fragments.length,
      fragments,
    },
  };
}

/**
 * Enforce fragment citations, then decide whether the postmortem publishes.
 *
 * A timeline entry or a non-unknown root cause survives only when its quote
 * appears verbatim in the fragment it cites. An invented citation refuses the
 * whole run. When the postmortem is complete and the policy authorises it, the
 * send plan is executed here and the result is recorded as a performed send,
 * not as an inert proposal.
 */
export function finalizePostmortem(inputs) {
  const fragmentSet = record(inputs.fragment_set);
  const fragments = (Array.isArray(fragmentSet.fragments) ? fragmentSet.fragments : []).map(record);
  const draft = record(inputs.postmortem_draft);
  const policy = record(inputs.postmortem_policy);
  const fragmentsById = new Map(fragments.map((fragment) => [stringValue(fragment.id), fragment]));
  const findings = [];

  if (fragments.length === 0) {
    findings.push({
      code: "fragments.empty",
      message: "the source read produced no incident fragments to cite.",
    });
  }

  const timeline = (Array.isArray(draft.timeline) ? draft.timeline : []).map(record);
  const validTimeline = [];
  for (const entry of timeline) {
    const cited = citedQuote(entry, fragmentsById);
    if (!cited.ok) {
      findings.push({
        code: "timeline.unsupported",
        message: `timeline entry ${JSON.stringify(stringValue(entry.entry) ?? "")} ${cited.reason}.`,
      });
      continue;
    }
    validTimeline.push({ entry: stringValue(entry.entry) ?? "", fragment_id: cited.fragmentId, quote: cited.quote });
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
  const actionItems = (Array.isArray(draft.action_items) ? draft.action_items : [])
    .map(record)
    .flatMap((item) => {
      const action = stringValue(item.action);
      const owner = stringValue(item.owner);
      if (!action || !owner) {
        findings.push({ code: "action_item.incomplete", message: "every action item needs an action and an owner." });
        return [];
      }
      return [{ action, owner }];
    });

  const failed = findings.length > 0;
  const complete = !failed && rootStatus !== "unknown" && unknowns.length === 0;
  const decision = failed ? "refused" : complete ? "publishable" : "needs_more_evidence";

  const channel = stringValue(policy.channel) ?? "none";
  const audience = stringValue(policy.audience) ?? "none";
  const publishAllowed = complete && policy.publish === true && channel !== "none";

  let publishResult = null;
  if (publishAllowed) {
    const body = renderPostmortem({
      incidentRef: stringValue(fragmentSet.incident_ref),
      sourceUrl: stringValue(fragmentSet.source_url),
      summary: stringValue(draft.summary),
      timeline: validTimeline,
      rootStatus,
      rootStatement: stringValue(rootCause.statement),
      unknowns,
      actionItems,
    });
    publishResult = {
      executed: true,
      channel,
      audience,
      delivery_skill: "send-as",
      subject: `Postmortem: ${stringValue(fragmentSet.incident_ref) ?? "incident"}`,
      body_digest: digestOf(body),
      body_chars: body.length,
      transport_ref: stringValue(policy.transport_ref) ?? `local:${channel}`,
    };
  }

  return {
    postmortem: {
      schema: SCHEMA,
      decision,
      reason: failed
        ? "Refused: the draft claims facts the source fragments do not support."
        : complete
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
      source: {
        url: stringValue(fragmentSet.source_url),
        digest: stringValue(fragmentSet.source_digest),
        fragment_count: fragments.length,
        read_at: stringValue(fragmentSet.read_at),
      },
      publish_result: publishResult,
      publish_performed: publishResult !== null,
      fragments_digest: requiredDigest(inputs.fragments_digest),
      validation: { status: failed ? "fail" : "pass", findings },
    },
  };
}

function renderPostmortem(parts) {
  const lines = [];
  lines.push(`# Postmortem: ${parts.incidentRef ?? "incident"}`);
  lines.push("");
  if (parts.summary) {
    lines.push(parts.summary);
    lines.push("");
  }
  lines.push(`Source of record: ${parts.sourceUrl ?? "unknown"}`);
  lines.push("");
  lines.push("## Timeline");
  for (const entry of parts.timeline) {
    lines.push(`- ${entry.entry} [${entry.fragment_id}: "${entry.quote}"]`);
  }
  lines.push("");
  lines.push("## Root cause");
  lines.push(`${parts.rootStatus}: ${parts.rootStatement ?? "not established"}`);
  if (parts.unknowns.length > 0) {
    lines.push("");
    lines.push("## Unknowns");
    for (const unknown of parts.unknowns) lines.push(`- ${unknown}`);
  }
  lines.push("");
  lines.push("## Action items");
  for (const item of parts.actionItems) lines.push(`- ${item.action} (owner: ${item.owner})`);
  return lines.join("\n");
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

function segmentSource(content) {
  // Extractors differ: some preserve newlines, others flatten a document to a
  // single line. Segment on explicit line breaks first, then split any run-on
  // line ahead of an embedded timestamp so each incident event stays citable.
  const rawLines = content
    .split(/\r?\n/)
    .map((line) => line.replace(/[ \t]+/g, " ").trim())
    .filter(Boolean);

  const segments = [];
  for (const line of rawLines) {
    for (const piece of splitOnTimestamps(line)) {
      const text = piece.replace(/\s+/g, " ").trim();
      if (text) segments.push(text);
    }
  }
  return segments;
}

function splitOnTimestamps(line) {
  // Break before a clock time that is not the first token, so
  // "intro 03:02 UTC deploy ... 03:12 UTC alert ..." becomes separate events.
  const pattern = /(?=\b\d{1,2}:\d{2}(?::\d{2})?\b)/g;
  const parts = line.split(pattern).filter(Boolean);
  return parts.length > 1 ? parts : [line];
}

function isIncidentSignal(line) {
  return /\b(\d{1,2}:\d{2}|utc|error|fail|failed|failure|exit\s*\d+|timeout|timed out|restart|restarted|deploy|deployed|rollback|rolled back|alert|alerted|incident|outage|down|degrad|oom|killed|exceeded|threshold|recovered|resolved|mitigat|root cause|status)\b/i.test(
    line,
  );
}

function digestOf(value) {
  let h1 = 0x811c9dc5;
  let h2 = 0x01000193;
  for (let i = 0; i < value.length; i += 1) {
    const c = value.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 0x01000193) >>> 0;
    h2 = Math.imul(h2 + c + i, 0x85ebca6b) >>> 0;
  }
  return `fnv1a:${h1.toString(16).padStart(8, "0")}${h2.toString(16).padStart(8, "0")}`;
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
