export function finalizeUpdates(inputs) {
  const transcript = typeof inputs.transcript === "string" ? inputs.transcript : "";
  const records = (Array.isArray(inputs.crm_records) ? inputs.crm_records : []).map(record);
  const allowedFields = uniqueStrings(record(inputs.crm_schema).allowed_fields);
  const draft = record(inputs.update_draft);
  const proposed = (Array.isArray(draft.updates) ? draft.updates : []).map(record);
  const sourceRead = record(inputs.source_read);
  const writeTarget = record(inputs.write_target);
  const recordsById = new Map(records.map((entry) => [stringValue(entry.id), entry]));
  const findings = [];
  const updates = [];
  const rejected = [];

  if (records.length === 0) {
    findings.push({ code: "source.empty", message: "the source read returned no CRM records." });
  }

  const takeaways = uniqueStrings(draft.takeaways).slice(0, 100);

  for (const update of proposed) {
    const recordId = stringValue(update.record_id);
    const field = stringValue(update.field);
    const to = update.to;
    const quote = stringValue(update.evidence_quote);
    const target = recordId ? recordsById.get(recordId) : undefined;
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
  const decision = failed ? "refused" : updates.length > 0 ? "applied" : "no_action";
  const writeReady = !failed && updates.length > 0 && stringValue(writeTarget.kind) === "mock";
  return {
    crm_cleanup: {
      schema: "runx.crm_cleanup.v2",
      decision,
      reason: failed
        ? "Refused: the reconciliation draft does not reconcile deterministically with the supplied records and transcript."
        : updates.length > 0
          ? `Applied ${updates.length} allowlisted field update(s) through the mock CRM transport, each traced to transcript evidence.`
          : "No actionable field updates were supported by the transcript; nothing was written.",
      takeaways: failed ? [] : takeaways,
      field_updates: failed ? [] : updates,
      rejected_updates: rejected,
      write_plan: {
        schema: "runx.write_plan.v1",
        status: writeReady ? "ready" : "withheld",
        transport: "mock-crm-outbox",
        bound_to: requiredDigest(inputs.records_digest),
      },
      source: {
        read_kind: stringValue(sourceRead.read_kind) ?? "unknown",
        fetched_ref: stringValue(sourceRead.fetched_ref) ?? "unknown",
      },
      transcript_digest: requiredDigest(inputs.transcript_digest),
      records_digest: requiredDigest(inputs.records_digest),
      validation: { status: failed ? "fail" : "pass", findings },
    },
  };
}

export function prepareWrite(inputs) {
  const cleanup = record(inputs.crm_cleanup);
  const target = record(inputs.write_target);
  const records = (Array.isArray(inputs.crm_records) ? inputs.crm_records : []).map(record);
  const updates = (Array.isArray(cleanup.field_updates) ? cleanup.field_updates : []).map(record);
  const path = stringValue(target.path);
  if (!path) {
    throw new Error("write_target.path is required for the mock CRM transport");
  }
  const byId = new Map(records.map((entry) => [stringValue(entry.id), { ...entry }]));
  const before = [];
  for (const update of updates) {
    const rid = stringValue(update.record_id);
    const field = stringValue(update.field);
    const row = rid ? byId.get(rid) : undefined;
    if (!row || !field) continue;
    before.push({ record_id: rid, field, from: row[field] ?? null, to: update.to });
    row[field] = update.to;
  }
  const after = [...byId.values()];
  return {
    write_draft: {
      path,
      contents: JSON.stringify({ records: after }, null, 2),
      before,
      after_count: after.length,
    },
  };
}

export function recordWrite(inputs) {
  const cleanup = record(inputs.crm_cleanup);
  const draft = record(inputs.write_draft);
  const applied = record(inputs.applied);
  const updates = (Array.isArray(cleanup.field_updates) ? cleanup.field_updates : []).map(record);
  const before = (Array.isArray(draft.before) ? draft.before : []).map(record);
  return {
    write_result: {
      schema: "runx.write_result.v1",
      executed: true,
      transport: "mock-crm-outbox",
      bound_to: stringValue(record(cleanup.write_plan).bound_to) ?? "",
      path: stringValue(applied.path) ?? stringValue(draft.path) ?? "",
      before,
      after: updates.map((u) => ({ record_id: stringValue(u.record_id), field: stringValue(u.field), value: u.to })),
      applied_updates: updates.length,
    },
  };
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
