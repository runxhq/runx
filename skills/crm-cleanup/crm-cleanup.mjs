export function finalizeUpdates(inputs) {
  const transcript = typeof inputs.transcript === "string" ? inputs.transcript : "";
  const records = (Array.isArray(inputs.crm_records) ? inputs.crm_records : []).map(record);
  const allowedFields = uniqueStrings(record(inputs.crm_schema).allowed_fields);
  const draft = record(inputs.update_draft);
  const proposed = (Array.isArray(draft.updates) ? draft.updates : []).map(record);
  const recordsById = new Map(records.map((entry) => [stringValue(entry.id), entry]));
  const findings = [];
  const updates = [];
  const rejected = [];
  const seenUpdates = new Set();
  const slotWinners = new Map();

  for (const update of proposed) {
    const recordId = stringValue(update.record_id);
    const field = stringValue(update.field);
    const to = update.to;
    const quote = stringValue(update.evidence_quote);
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
    const toKey = JSON.stringify(to) ?? "undefined";
    const updateKey = `${recordId}${field}${toKey}${quote}`;
    if (seenUpdates.has(updateKey)) {
      continue;
    }
    seenUpdates.add(updateKey);
    const slotKey = `${recordId}${field}`;
    if (slotWinners.has(slotKey) && slotWinners.get(slotKey) !== toKey) {
      findings.push({ code: "update.conflicting_values", message: `conflicting target values proposed for ${recordId}.${field}; refusing the run rather than picking a winner silently.` });
      continue;
    }
    slotWinners.set(slotKey, toKey);
    updates.push({
      record_id: recordId,
      field,
      from: target[field] === undefined || target[field] === null ? null : target[field],
      to,
      evidence_quote: quote,
    });
  }

  const failed = findings.length > 0;
  const decision = failed ? "refused" : updates.length > 0 ? "proposed" : "no_action";
  return {
    crm_update_proposal: {
      schema: "runx.crm_update_proposal.v1",
      decision,
      reason: failed
        ? "Refused: the reconciliation draft does not reconcile deterministically with the supplied records and transcript."
        : updates.length > 0
          ? `Proposed ${updates.length} allowlisted field update(s), each traced to transcript evidence.`
          : "No actionable field updates were supported by the transcript.",
      updates: failed ? [] : updates,
      rejected_updates: rejected,
      write_performed: false,
      gate: "crm-operator-or-human-approver",
      transcript_digest: requiredDigest(inputs.transcript_digest),
      records_digest: requiredDigest(inputs.records_digest),
      validation: { status: failed ? "fail" : "pass", findings },
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

export function applyUpdates(inputs) {
  const proposal = record(inputs.crm_update_proposal);
  const records = (Array.isArray(inputs.crm_records) ? inputs.crm_records : []).map(record);
  const recordsById = new Map(records.map((entry) => [stringValue(entry.id), entry]));
  const applied = [];
  const writeLog = [];

  if (proposal.decision !== "proposed" || !Array.isArray(proposal.updates)) {
    return {
      crm_write_result: {
        schema: "runx.crm_write_result.v1",
        executed: false,
        reason: "No proposed updates to apply; nothing was written.",
        write_result: { before: [], after: [] },
      },
    };
  }

  for (const update of proposal.updates) {
    const recordId = stringValue(update.record_id);
    const field = stringValue(update.field);
    const target = recordsById.get(recordId);
    if (!target || !field) continue;
    const before = target[field] === undefined ? null : target[field];
    const after = update.to;
    target[field] = after;
    applied.push({ record_id: recordId, field, before, after });
    writeLog.push(`mock-transport: ${recordId}.${field} ${JSON.stringify(before)} -> ${JSON.stringify(after)}`);
  }

  return {
    crm_write_result: {
      schema: "runx.crm_write_result.v1",
      executed: applied.length > 0,
      reason: applied.length > 0
        ? `Applied ${applied.length} update(s) through the mock CRM transport.`
        : "No updates were applicable; nothing was written.",
      write_result: {
        before: applied.map((a) => ({ record_id: a.record_id, field: a.field, value: a.before })),
        after: applied.map((a) => ({ record_id: a.record_id, field: a.field, value: a.after })),
      },
      transport_log: writeLog,
    },
  };
}
