const SOURCE_KINDS = ["read_projection", "connector_export", "web_fetch"];
const STALLED_QUOTES = [
  "the rollout stalled and they want an executive review before renewing",
  "the rollout stalled",
];
const OWNER_MOVE = /move the account to ([A-Za-z][A-Za-z0-9_-]*)/i;
const NEXT_STEP = /Next step agreed:\s*([^.]+)/i;

export function runCleanup(inputs) {
  // This entrypoint is the whole read -> decide -> mock-write loop for one transcript against one fetched CRM source.
  const crmSource = record(inputs.crm_source);
  const transcript = typeof inputs.transcript === "string" ? inputs.transcript : "";
  if (!transcript.trim()) fail("transcript is required");
  const allowedFields = uniqueStrings(record(inputs.crm_schema).allowed_fields);
  if (allowedFields.length === 0) fail("crm_schema.allowed_fields must name at least one field");

  const sourceRead = normalizeSource(crmSource, inputs.source_fetch);
  const records = sourceRead.records;
  const decided = decideUpdates(transcript, records, allowedFields);
  const writeResult = executeMockTransport(decided, records, inputs.transcript_digest, inputs.records_digest);
  const decision = decided.findings.length > 0
    ? "refused"
    : decided.updates.length > 0
      ? "applied"
      : "no_action";
  if (decision === "applied" && writeResult.executed !== true) fail("apply decision finalized without an executed transport result");
  if (decision !== "applied" && writeResult.executed !== false) fail("transport executed without an apply decision");

  const reason = decided.findings.length > 0
    ? "Refused: a proposed update did not reconcile deterministically with the source records and transcript."
    : decided.updates.length > 0
      ? `Applied ${decided.updates.length} allowlisted field update(s) through the CRM transport, each traced to transcript evidence.`
      : "No actionable field updates were supported by the transcript; the transport executed nothing.";

  // This packet is the typed skill output: takeaways, field_updates, and the transport's before/after write_result.
  return {
    takeaways: decided.takeaways,
    field_updates: decided.updates,
    write_result: writeResult,
    source_read: {
      kind: sourceRead.kind,
      ref: sourceRead.ref,
      count: sourceRead.count,
    },
    crm_cleanup_result: {
      schema: "runx.crm_cleanup_result.v1",
      decision,
      reason,
      takeaways: decided.takeaways,
      field_updates: decided.updates,
      rejected_updates: decided.rejected,
      write_result: writeResult,
      source_read: {
        kind: sourceRead.kind,
        ref: sourceRead.ref,
        count: sourceRead.count,
      },
      transcript_digest: requiredDigest(inputs.transcript_digest),
      records_digest: requiredDigest(inputs.records_digest),
      validation: {
        status: decided.findings.length > 0 ? "fail" : "pass",
        findings: decided.findings,
      },
    },
  };
}

function normalizeSource(crmSource, sourceFetchRaw) {
  // This block binds the live fetch to a source handle so the receipt can prove records were not pasted as fixture args.
  const kind = text(crmSource.kind);
  if (!SOURCE_KINDS.includes(kind)) fail(`crm_source.kind must be one of ${SOURCE_KINDS.join(", ")}`);
  const url = httpsUrl(crmSource.url, "crm_source.url");
  const allowlist = uniqueStrings(crmSource.allowlist);
  if (allowlist.length === 0) fail("crm_source.allowlist must name the source host");
  const host = hostOf(url);
  if (!host || !allowlist.includes(host)) fail("crm_source.url host is outside crm_source.allowlist");

  const fetched = unwrapFetch(sourceFetchRaw);
  const status = fetched.status;
  if (!Number.isInteger(status) || status < 200 || status >= 300) fail(`source read returned HTTP ${String(status)}`);
  if (record(fetched.provenance).truncated === true) fail("source read was truncated");
  const policy = record(fetched.policy);
  if (policy.allowlist_decision && policy.allowlist_decision !== "allowed") {
    fail("source read did not pass the supplied allowlist");
  }
  const extracted = fetched.extracted;
  if (typeof extracted !== "string" || !extracted.trim()) fail("source read did not return JSON text");
  let payload;
  try {
    payload = JSON.parse(extracted);
  } catch {
    fail("source read returned malformed JSON");
  }
  const records = (Array.isArray(payload) ? payload : array(record(payload).records))
    .map(record)
    .filter((entry) => text(entry.id));
  if (records.length === 0) fail("source read produced no CRM records with ids");
  return {
    kind,
    ref: url,
    count: records.length,
    records,
  };
}

function decideUpdates(transcript, records, allowedFields) {
  // This block derives allowlisted field updates from verbatim transcript quotes instead of asking a model to invent them.
  const findings = [];
  const updates = [];
  const rejected = [];
  const takeaways = [];
  const targets = selectRecords(transcript, records);

  for (const target of targets) {
    const recordId = text(target.id);
    const ownerMatch = transcript.match(OWNER_MOVE);
    if (ownerMatch) {
      const quote = ownerMatch[0];
      const to = ownerMatch[1];
      queueUpdate({
        transcript, target, recordId, field: "owner", to, quote, allowedFields, updates, rejected, findings,
      });
    }

    const stalledQuote = firstPresent(transcript, STALLED_QUOTES);
    if (stalledQuote) {
      queueUpdate({
        transcript, target, recordId, field: "account_status", to: "at_risk", quote: stalledQuote,
        allowedFields, updates, rejected, findings,
      });
    }

    const nextMatch = transcript.match(NEXT_STEP);
    if (nextMatch) {
      const to = nextMatch[1].trim();
      queueUpdate({
        transcript, target, recordId, field: "next_action", to, quote: to, allowedFields, updates, rejected, findings,
      });
    }
  }

  if (updates.length > 0) {
    for (const update of updates) {
      takeaways.push(`Update ${update.record_id}.${update.field} from ${stringify(update.from)} to ${stringify(update.to)}.`);
    }
  } else if (rejected.length > 0 && findings.length === 0) {
    takeaways.push("Transcript named a field outside crm_schema.allowed_fields; no write was executed.");
  } else if (findings.length === 0) {
    takeaways.push("No allowlisted field changes were supported by the transcript.");
  }

  return { findings, updates, rejected, takeaways };
}

function queueUpdate({
  transcript, target, recordId, field, to, quote, allowedFields, updates, rejected, findings,
}) {
  // This helper either records an allowlisted traced update, names an out-of-schema rejection, or refuses a quote that is not in the transcript.
  if (!recordId) {
    findings.push({ code: "update.unknown_record", message: "update targets a record with no id." });
    return;
  }
  if (!quote || !transcript.includes(quote)) {
    findings.push({
      code: "update.unsupported_evidence",
      message: `update to ${recordId}.${field} cites a quote that is not present in the transcript.`,
    });
    return;
  }
  if (!allowedFields.includes(field)) {
    rejected.push({ record_id: recordId, field, reason: "field is outside the crm_schema allowlist" });
    return;
  }
  if (to === undefined || to === null || to === "") {
    findings.push({ code: "update.empty_value", message: `update to ${recordId}.${field} carries no target value.` });
    return;
  }
  const from = target[field] === undefined || target[field] === null ? null : target[field];
  if (JSON.stringify(from) === JSON.stringify(to)) return;
  if (updates.some((entry) => entry.record_id === recordId && entry.field === field)) return;
  updates.push({ record_id: recordId, field, from, to, evidence_quote: quote });
}

function executeMockTransport(decided, records, transcriptDigest, recordsDigest) {
  // This mock CRM transport applies decided updates in-process and seals the before/after snapshot bound to both input digests.
  const before = records.map((entry) => ({ ...entry }));
  const digestBinding = {
    transcript_digest: requiredDigest(transcriptDigest),
    records_digest: requiredDigest(recordsDigest),
  };
  if (decided.findings.length > 0 || decided.updates.length === 0) {
    return {
      schema: "runx.crm_write_result.v1",
      executed: false,
      transport: "mock-crm.v1",
      write_ref: "mock-crm:none:0",
      decision_digest_binding: digestBinding,
      applied: [],
      before,
      after: before.map((entry) => ({ ...entry })),
    };
  }
  const after = before.map((entry) => ({ ...entry }));
  const afterById = new Map(after.map((entry) => [text(entry.id), entry]));
  const applied = [];
  for (const update of decided.updates) {
    const target = afterById.get(update.record_id);
    if (!target) fail(`transport cannot apply update to unknown record ${update.record_id}`);
    const observedFrom = target[update.field] === undefined || target[update.field] === null ? null : target[update.field];
    if (JSON.stringify(observedFrom) !== JSON.stringify(update.from)) {
      fail(`drift detected on ${update.record_id}.${update.field}`);
    }
    target[update.field] = update.to;
    applied.push({
      record_id: update.record_id,
      field: update.field,
      from: update.from,
      to: update.to,
    });
  }
  return {
    schema: "runx.crm_write_result.v1",
    executed: true,
    transport: "mock-crm.v1",
    write_ref: `mock-crm:${requiredDigest(recordsDigest).slice(7, 23)}:${applied.length}`,
    decision_digest_binding: digestBinding,
    applied,
    before,
    after,
  };
}

function selectRecords(transcript, records) {
  // This selector uses a single-record source as the target, otherwise only records whose id tokens appear in the transcript.
  if (records.length === 1) return records;
  const lower = transcript.toLowerCase();
  const hits = records.filter((entry) => {
    const id = String(entry.id || "").toLowerCase();
    const name = String(entry.name || entry.account || "").toLowerCase();
    if (name && lower.includes(name)) return true;
    return id.split(/[-_]/).some((part) => part.length > 3 && lower.includes(part));
  });
  return hits;
}

function unwrapFetch(value) {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  if (value.fetch_result && typeof value.fetch_result === "object") return unwrapFetch(value.fetch_result);
  if (value.data && typeof value.data === "object" && !Array.isArray(value.data)) return value.data;
  return value;
}

function firstPresent(transcript, snippets) {
  for (const snippet of snippets) {
    if (transcript.includes(snippet)) return snippet;
  }
  return null;
}

function stringify(value) {
  if (value === null || value === undefined) return "null";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) fail("native digest evidence is missing");
  return value;
}

function httpsUrl(value, field) {
  const url = text(value);
  if (!url) fail(`${field} is required`);
  if (!hostOf(url)) fail(`${field} must be a valid https URL`);
  return url;
}

function hostOf(url) {
  const match = /^https:\/\/([^/?#]+)(?:[/?#]|$)/.exec(url ?? "");
  if (!match) return null;
  const authority = match[1];
  const withoutUserinfo = authority.includes("@") ? authority.slice(authority.lastIndexOf("@") + 1) : authority;
  const host = withoutUserinfo.replace(/:\d+$/, "");
  return host || null;
}

function fail(message) {
  throw new Error(message);
}

function text(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function uniqueStrings(value) {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map(text).filter(Boolean))];
}

function array(value) {
  return Array.isArray(value) ? value : [];
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

export default runCleanup;
