import { readFileSync } from "node:fs";

const SOURCE_KINDS = ["read_projection", "connector_export", "web_fetch"];

export function normalizeRecords(inputs) {
  const source = record(inputs.crm_source);
  const kind = text(source.kind);
  if (!SOURCE_KINDS.includes(kind)) {
    throw new Error(`crm_source.kind must be one of ${SOURCE_KINDS.join(", ")}`);
  }
  const url = httpsUrl(source.url, "crm_source.url");
  const allowlist = uniqueStrings(source.allowlist);
  if (allowlist.length === 0) throw new Error("crm_source.allowlist must name the source host");
  const host = hostOf(url);
  if (!allowlist.includes(host)) throw new Error("crm_source.url host is outside crm_source.allowlist");
  const fetched = requiredRecord(inputs.source_fetch, "source_fetch");
  if (!Number.isInteger(fetched.status) || fetched.status < 200 || fetched.status >= 300) {
    throw new Error(`source read returned HTTP ${String(fetched.status)}`);
  }
  if (record(fetched.provenance).truncated === true) throw new Error("source read was truncated");
  if (record(fetched.policy).allowlist_decision !== "allowed") {
    throw new Error("source read did not pass the supplied allowlist");
  }
  const extracted = fetched.extracted;
  if (typeof extracted !== "string" || !extracted.trim()) throw new Error("source read did not return JSON text");
  let payload;
  try {
    payload = JSON.parse(extracted);
  } catch {
    throw new Error("source read returned malformed JSON");
  }
  let records = (Array.isArray(payload) ? payload : array(record(payload).records)).map(record);
  records = records.filter((entry) => text(entry.id));
  if (records.length === 0) throw new Error("source read produced no CRM records with ids");
  return {
    source_read: {
      schema: "runx.crm_source_read.v1",
      kind,
      ref: url,
      count: records.length,
      records,
    },
  };
}

export function decideUpdates(inputs) {
  const transcript = typeof inputs.transcript === "string" ? inputs.transcript : "";
  const records = array(record(inputs.source_read).records).map(record);
  const allowedFields = uniqueStrings(record(inputs.crm_schema).allowed_fields);
  const draft = record(inputs.update_draft);
  const proposed = array(draft.updates).map(record);
  const takeaways = uniqueStrings(draft.takeaways).slice(0, 20);
  const recordsById = new Map(records.map((entry) => [text(entry.id), entry]));
  const findings = [];
  const updates = [];
  const rejected = [];

  for (const update of proposed) {
    const recordId = text(update.record_id);
    const field = text(update.field);
    const to = update.to;
    const quote = text(update.evidence_quote);
    const target = recordsById.get(recordId);
    if (!target) {
      findings.push({ code: "update.unknown_record", message: `update targets unknown record ${recordId ?? "(missing)"}.` });
      continue;
    }
    if (!field || !allowedFields.includes(field)) {
      rejected.push({ record_id: recordId, field: field ?? "", reason: "field is outside the crm_schema allowlist" });
      continue;
    }
    if (!quote || !transcript.includes(quote)) {
      findings.push({ code: "update.unsupported_evidence", message: `update to ${recordId}.${field} cites a quote that is not present in the transcript.` });
      continue;
    }
    if (to === undefined || to === null || to === "") {
      findings.push({ code: "update.empty_value", message: `update to ${recordId}.${field} carries no target value.` });
      continue;
    }
    updates.push({
      record_id: recordId,
      field,
      from: target[field] === undefined || target[field] === null ? null : target[field],
      to,
      evidence_quote: quote,
    });
  }

  const failed = findings.length > 0;
  const finalUpdates = failed ? [] : updates;
  const decision = failed ? "refused" : finalUpdates.length > 0 ? "apply" : "no_action";
  return {
    crm_decision: {
      schema: "runx.crm_decision.v1",
      decision,
      has_updates: decision === "apply",
      reason: failed
        ? "Refused: the reconciliation draft does not reconcile deterministically with the source records and transcript."
        : finalUpdates.length > 0
          ? `Decided ${finalUpdates.length} allowlisted field update(s), each traced to transcript evidence; handing to the CRM transport.`
          : "No actionable field updates were supported by the transcript; the transport executes nothing.",
      field_updates: finalUpdates,
      rejected_updates: rejected,
      takeaways,
      transcript_digest: requiredDigest(inputs.transcript_digest),
      records_digest: requiredDigest(inputs.records_digest),
      validation: { status: failed ? "fail" : "pass", findings },
    },
  };
}

export function executeWrites(inputs) {
  const decision = requiredRecord(inputs.crm_decision, "crm_decision");
  const sourceRecords = array(record(inputs.source_read).records).map((entry) => ({ ...record(entry) }));
  if (decision.has_updates !== true) {
    return {
      write_result: {
        schema: "runx.crm_write_result.v1",
        executed: false,
        transport: "mock-crm.v1",
        write_ref: "mock-crm:none:0",
        decision_digest_binding: {
          transcript_digest: requiredDigest(decision.transcript_digest),
          records_digest: requiredDigest(decision.records_digest),
        },
        applied: [],
        before: sourceRecords,
        after: sourceRecords,
      },
    };
  }
  const updates = array(decision.field_updates).map(record);
  const before = sourceRecords;
  const byId = new Map(before.map((entry) => [text(entry.id), entry]));
  const after = before.map((entry) => ({ ...entry }));
  const afterById = new Map(after.map((entry) => [text(entry.id), entry]));
  const applied = [];
  for (const update of updates) {
    const recordId = text(update.record_id);
    const field = text(update.field);
    const target = afterById.get(recordId);
    if (!target || !field) throw new Error(`transport cannot apply update to ${recordId}.${field}`);
    const observedFrom = byId.get(recordId)[field] === undefined || byId.get(recordId)[field] === null ? null : byId.get(recordId)[field];
    const expectedFrom = update.from === undefined ? null : update.from;
    if (JSON.stringify(observedFrom) !== JSON.stringify(expectedFrom)) {
      throw new Error(`drift detected on ${recordId}.${field}: decision expected ${JSON.stringify(expectedFrom)}, transport read ${JSON.stringify(observedFrom)}`);
    }
    target[field] = update.to;
    applied.push({ record_id: recordId, field, from: expectedFrom, to: update.to });
  }
  return {
    write_result: {
      schema: "runx.crm_write_result.v1",
      executed: true,
      transport: "mock-crm.v1",
      write_ref: `mock-crm:${requiredDigest(decision.records_digest).slice(7, 23)}:${applied.length}`,
      decision_digest_binding: {
        transcript_digest: requiredDigest(decision.transcript_digest),
        records_digest: requiredDigest(decision.records_digest),
      },
      applied,
      before,
      after,
    },
  };
}

export function finalizeResult(inputs) {
  const decision = requiredRecord(inputs.crm_decision, "crm_decision");
  const sourceRead = requiredRecord(inputs.source_read, "source_read");
  const writeResult = requiredRecord(inputs.write_result, "write_result");
  if (decision.decision === "apply" && writeResult.executed !== true) {
    throw new Error("apply decision finalized without an executed transport result");
  }
  if (decision.decision !== "apply" && writeResult.executed !== false) {
    throw new Error("transport executed without an apply decision");
  }
  const finalDecision = decision.decision === "apply" ? "applied" : decision.decision;
  return {
    crm_cleanup_result: {
      schema: "runx.crm_cleanup_result.v1",
      decision: finalDecision,
      reason: text(decision.reason) ?? "",
      takeaways: array(decision.takeaways).map((t) => String(t)).filter(Boolean),
      field_updates: array(decision.field_updates),
      rejected_updates: array(decision.rejected_updates),
      write_result: writeResult,
      source_read: {
        kind: text(sourceRead.kind) ?? "",
        ref: text(sourceRead.ref) ?? "",
        count: Number(sourceRead.count) || 0,
      },
      transcript_digest: requiredDigest(decision.transcript_digest),
      records_digest: requiredDigest(decision.records_digest),
      validation: record(decision.validation),
    },
  };
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function httpsUrl(value, field) {
  const url = text(value);
  if (!url) throw new Error(`${field} is required for a fetched source`);
  if (!hostOf(url)) throw new Error(`${field} must be a valid https URL`);
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

function requiredRecord(value, label) {
  const result = record(value);
  if (Object.keys(result).length === 0) throw new Error(`${label} evidence is missing`);
  return result;
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

function readCliInputs() {
  if (process.env.RUNX_INPUTS_PATH) {
    return JSON.parse(readFileSync(process.env.RUNX_INPUTS_PATH, "utf8"));
  }
  return JSON.parse(process.env.RUNX_INPUTS_JSON || "{}");
}

function runCli() {
  const operation = process.argv[2];
  const inputs = readCliInputs();
  if (operation === "normalize") return normalizeRecords(inputs);
  if (operation === "decide") return decideUpdates(inputs);
  if (operation === "transport") return executeWrites(inputs);
  if (operation === "finalize") return finalizeResult(inputs);
  throw new Error("operation must be normalize, decide, transport, or finalize");
}

if (process.argv[1]?.endsWith("crm-cleanup.mjs")) {
  try {
    process.stdout.write(`${JSON.stringify(runCli())}\n`);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    process.stderr.write(`crm-cleanup failed: ${message}\n`);
    process.exitCode = 1;
  }
}
