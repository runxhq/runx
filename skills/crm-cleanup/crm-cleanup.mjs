import { readFileSync } from "node:fs";

export function reconcileCleanup(inputs) {
  const transcript = requiredString(inputs.transcript, "transcript");
  const sourceHandle = requiredRecord(inputs.source_handle, "source_handle");
  const crmSchema = requiredRecord(inputs.crm_schema, "crm_schema");
  const sourceRead = requiredRecord(inputs.source_read, "source_read");
  const transcriptDigest = requiredDigest(inputs.transcript_digest, "transcript_digest");
  const schemaDigest = requiredDigest(inputs.crm_schema_digest, "crm_schema_digest");

  validateSourceHandle(sourceHandle);
  assertClosedKeys(crmSchema, ["record_id_field", "fields"], "crm_schema");
  if (transcript.length > 100000) {
    throw new Error("transcript exceeds 100000 characters");
  }
  const sourceEvidence = validateSourceRead(sourceHandle, sourceRead);
  const records = parseRecords(sourceRead.extracted);
  const idField = requiredString(crmSchema.record_id_field, "crm_schema.record_id_field");
  const targetId = requiredString(sourceHandle.record_id, "source_handle.record_id");
  const matches = records.filter((candidate) => scalarKey(candidate[idField]) === targetId);
  if (matches.length === 0) {
    throw new Error(`source record ${targetId} was not found by ${idField}`);
  }
  if (matches.length > 1) {
    throw new Error(`source record ${targetId} is ambiguous by ${idField}`);
  }
  const [before] = matches;

  const fieldDefinitions = requiredRecord(crmSchema.fields, "crm_schema.fields");
  const fieldNames = Object.keys(fieldDefinitions);
  if (fieldNames.length === 0) {
    throw new Error("crm_schema.fields must declare at least one writable field");
  }
  if (fieldNames.length > 100) {
    throw new Error("crm_schema.fields exceeds 100 writable fields");
  }
  for (const field of fieldNames) {
    if (!/^[A-Za-z][A-Za-z0-9_]{0,127}$/.test(field)) {
      throw new Error(`crm_schema field ${field} has an invalid name`);
    }
    validateFieldDefinition(field, requiredRecord(fieldDefinitions[field], `crm_schema.fields.${field}`));
  }

  const extractionDraft = requiredRecord(inputs.extraction_draft, "extraction_draft");
  const parsed = validateExtractionDraft(transcript, fieldDefinitions, extractionDraft);
  const findings = [...parsed.findings];
  const candidates = {};
  const seen = new Set();

  for (const directive of parsed.directives) {
    if (!Object.hasOwn(fieldDefinitions, directive.field)) {
      findings.push({
        code: "directive.field_not_allowed",
        message: `${directive.field} is not declared in crm_schema.fields.`,
      });
      continue;
    }
    if (seen.has(directive.field)) {
      findings.push({
        code: "directive.duplicate_field",
        message: `${directive.field} appears in more than one CRM update directive.`,
      });
      continue;
    }
    seen.add(directive.field);

    try {
      const definition = requiredRecord(
        fieldDefinitions[directive.field],
        `crm_schema.fields.${directive.field}`,
      );
      const to = parseTypedValue(directive.rawValue, definition, directive.field);
      const from = Object.hasOwn(before, directive.field) ? before[directive.field] : null;
      if (!sameJsonValue(from, to)) {
        candidates[directive.field] = {
          from: cloneJson(from),
          to: cloneJson(to),
          evidence_quote: directive.evidenceQuote,
        };
      }
    } catch (error) {
      findings.push({
        code: "directive.invalid_value",
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }

  const refused = findings.length > 0;
  const changed = Object.keys(candidates).length > 0;
  return {
    cleanup_plan: {
      decision: refused ? "refused" : changed ? "ready" : "no_action",
      reason: refused
        ? parsed.needsReview.length > 0
          ? "The transcript contains a possible CRM change that is ambiguous, hedged, or not safely extractable; human review is required and no write was executed."
          : "The transcript contained a directive that the CRM schema does not authorize."
        : changed
          ? `Prepared ${Object.keys(candidates).length} schema-authorized field update(s) for the CRM transport.`
          : "No schema-authorized candidate changed the current CRM record.",
      takeaways: parsed.takeaways,
      needs_review: parsed.needsReview,
      field_updates: refused ? {} : candidates,
      record_id: targetId,
      before: cloneJson(before),
      source_read: sourceEvidence,
      transcript_digest: transcriptDigest,
      crm_schema_digest: schemaDigest,
      validation: {
        status: refused ? "fail" : "pass",
        findings,
      },
    },
  };
}

function validateExtractionDraft(transcript, fieldDefinitions, draft) {
  const allowedKeys = new Set(["takeaways", "candidates", "review_items"]);
  for (const key of Object.keys(draft)) {
    if (!allowedKeys.has(key)) {
      throw new Error(`extraction_draft.${key} is not supported`);
    }
  }
  if (!Array.isArray(draft.takeaways) || draft.takeaways.length > 50) {
    throw new Error("extraction_draft.takeaways must be an array of at most 50 strings");
  }
  if (!Array.isArray(draft.candidates) || draft.candidates.length > 100) {
    throw new Error("extraction_draft.candidates must be an array of at most 100 objects");
  }
  if (!Array.isArray(draft.review_items) || draft.review_items.length > 100) {
    throw new Error("extraction_draft.review_items must be an array of at most 100 objects");
  }

  const takeaways = draft.takeaways.map((value, index) => {
    const takeaway = requiredString(value, `extraction_draft.takeaways.${index}`);
    if (takeaway.length > 2000) throw new Error("extraction_draft takeaway exceeds 2000 characters");
    return takeaway;
  });
  const directives = [];
  const findings = [];
  const needsReview = [];
  const candidateFields = new Set();
  for (const [index, rawCandidate] of draft.candidates.entries()) {
    const candidate = requiredRecord(rawCandidate, `extraction_draft.candidates.${index}`);
    assertClosedKeys(
      candidate,
      ["field", "to", "evidence_quote"],
      `extraction_draft.candidates.${index}`,
    );
    const field = requiredString(candidate.field, `extraction_draft.candidates.${index}.field`);
    const evidenceQuote = requiredString(
      candidate.evidence_quote,
      `extraction_draft.candidates.${index}.evidence_quote`,
    );
    if (evidenceQuote.length > 2000) {
      throw new Error(`extraction_draft.candidates.${index}.evidence_quote exceeds 2000 characters`);
    }
    if (!Object.hasOwn(fieldDefinitions, field)) {
      findings.push({
        code: "extraction.field_not_allowed",
        message: `${field} is not declared in crm_schema.fields.`,
      });
      continue;
    }
    if (!transcript.includes(evidenceQuote)) {
      findings.push({
        code: "extraction.quote_not_found",
        message: `${field} evidence_quote is not an exact transcript substring.`,
      });
      continue;
    }
    if (candidateFields.has(field)) {
      needsReview.push({
        field,
        evidence_quote: evidenceQuote,
        reason: `${field} has conflicting or duplicate extracted values; confirm one exact value before writing.`,
      });
      continue;
    }
    candidateFields.add(field);
    if (/\b(?:maybe|might|may|possibly|probably|perhaps|uncertain|unsure|not\s+sure|i\s+think|i\s+guess|seems?|appears?|looks?)\b/i.test(evidenceQuote)) {
      needsReview.push({
        field,
        evidence_quote: evidenceQuote,
        reason: `${field} is mentioned with hedged or uncertain language; confirm an exact value before writing.`,
      });
      continue;
    }
    const rawValue = scalarExtractionValue(candidate.to, field);
    directives.push({ field, rawValue, evidenceQuote });
  }

  for (const [index, rawItem] of draft.review_items.entries()) {
    const item = requiredRecord(rawItem, `extraction_draft.review_items.${index}`);
    assertClosedKeys(
      item,
      ["field", "evidence_quote", "reason"],
      `extraction_draft.review_items.${index}`,
    );
    const evidenceQuote = requiredString(
      item.evidence_quote,
      `extraction_draft.review_items.${index}.evidence_quote`,
    );
    if (evidenceQuote.length > 2000) {
      throw new Error(`extraction_draft.review_items.${index}.evidence_quote exceeds 2000 characters`);
    }
    if (!transcript.includes(evidenceQuote)) {
      findings.push({
        code: "extraction.review_quote_not_found",
        message: `review_items.${index}.evidence_quote is not an exact transcript substring.`,
      });
      continue;
    }
    const field = item.field === null || item.field === undefined
      ? "unmapped"
      : requiredString(item.field, `extraction_draft.review_items.${index}.field`);
    if (field !== "unmapped" && !Object.hasOwn(fieldDefinitions, field)) {
      findings.push({
        code: "extraction.review_field_not_allowed",
        message: `${field} is not declared in crm_schema.fields.`,
      });
      continue;
    }
    const reason = requiredString(item.reason, `extraction_draft.review_items.${index}.reason`);
    if (reason.length > 2000) {
      throw new Error(`extraction_draft.review_items.${index}.reason exceeds 2000 characters`);
    }
    needsReview.push({
      field,
      evidence_quote: evidenceQuote,
      reason,
    });
  }
  for (const item of needsReview) {
    findings.push({
      code: "transcript.needs_review",
      field: item.field,
      evidence_quote: item.evidence_quote,
      message: item.reason,
    });
  }
  return { directives, findings, needsReview, takeaways };
}

function scalarExtractionValue(value, field) {
  if (!["string", "number", "boolean"].includes(typeof value)) {
    throw new Error(`extraction candidate ${field}.to must be a string, number, or boolean`);
  }
  return String(value);
}

export function finalizeCleanup(inputs) {
  const plan = requiredRecord(inputs.cleanup_plan, "cleanup_plan");
  const writeResult = requiredRecord(inputs.write_result, "write_result");
  const planDecision = requiredString(plan.decision, "cleanup_plan.decision");
  if (!["ready", "no_action", "refused"].includes(planDecision)) {
    throw new Error("cleanup_plan decision is invalid");
  }
  const fieldUpdates = requiredRecord(plan.field_updates, "cleanup_plan.field_updates");
  const updateFields = Object.keys(fieldUpdates).sort();
  const executed = writeResult.executed === true;
  const before = requiredRecord(writeResult.before, "write_result.before");
  const after = requiredRecord(writeResult.after, "write_result.after");
  const recordId = requiredString(plan.record_id, "cleanup_plan.record_id");
  if (
    writeResult.transport !== "mock-crm" ||
    writeResult.operation !== "update_fields" ||
    writeResult.record_id !== recordId
  ) {
    throw new Error("CRM transport identity does not match the reconciliation plan");
  }

  if (!sameJsonValue(before, requiredRecord(plan.before, "cleanup_plan.before"))) {
    throw new Error("CRM transport before record does not match the reconciliation plan");
  }
  if (planDecision === "ready") {
    if (updateFields.length === 0 || !executed || writeResult.status !== "applied") {
      throw new Error("CRM transport did not execute the ready update plan");
    }
    const appliedFields = Array.isArray(writeResult.applied_fields)
      ? [...writeResult.applied_fields].map(String).sort()
      : [];
    if (!sameJsonValue(appliedFields, updateFields)) {
      throw new Error("CRM transport applied fields do not match the reconciliation plan");
    }
    if (writeResult.writes_attempted !== 1 || writeResult.writes_completed !== 1) {
      throw new Error("CRM transport did not complete exactly one atomic write");
    }
    const expectedAfter = cloneJson(before);
    for (const field of updateFields) {
      const update = requiredRecord(fieldUpdates[field], `cleanup_plan.field_updates.${field}`);
      expectedAfter[field] = cloneJson(update.to);
    }
    if (!sameJsonValue(after, expectedAfter)) {
      throw new Error("CRM transport after record contains an unplanned change");
    }
  } else {
    if (
      updateFields.length !== 0 ||
      executed ||
      writeResult.status !== "no_op" ||
      writeResult.writes_attempted !== 0 ||
      writeResult.writes_completed !== 0 ||
      !sameJsonValue(before, after)
    ) {
      throw new Error("CRM transport must not write a no-action or refused plan");
    }
  }

  const validation = requiredRecord(plan.validation, "cleanup_plan.validation");
  const expectedValidation = planDecision === "refused" ? "fail" : "pass";
  if (validation.status !== expectedValidation || !Array.isArray(validation.findings)) {
    throw new Error("cleanup_plan validation does not match its decision");
  }

  const crmCleanupResult = {
    decision: planDecision === "ready" ? "updated" : planDecision,
    reason: planDecision === "ready"
      ? `Applied ${updateFields.length} schema-authorized field update(s) through the mock CRM transport.`
      : requiredString(plan.reason, "cleanup_plan.reason"),
    takeaways: Array.isArray(plan.takeaways) ? plan.takeaways : [],
    needs_review: Array.isArray(plan.needs_review) ? plan.needs_review : [],
    field_updates: fieldUpdates,
    write_result: writeResult,
    source_read: requiredRecord(plan.source_read, "cleanup_plan.source_read"),
    transcript_digest: requiredDigest(plan.transcript_digest, "cleanup_plan.transcript_digest"),
    crm_schema_digest: requiredDigest(plan.crm_schema_digest, "cleanup_plan.crm_schema_digest"),
    validation,
  };
  return {
    crm_cleanup_result: crmCleanupResult,
    takeaways: crmCleanupResult.takeaways,
    needs_review: crmCleanupResult.needs_review,
    field_updates: crmCleanupResult.field_updates,
    write_result: crmCleanupResult.write_result,
  };
}

export function applyMockCrmWrite(inputs) {
  const plan = requiredRecord(inputs.cleanup_plan, "cleanup_plan");
  const decision = requiredString(plan.decision, "cleanup_plan.decision");
  if (!["ready", "no_action", "refused"].includes(decision)) {
    throw new Error("cleanup_plan decision is invalid");
  }
  const before = cloneJson(requiredRecord(plan.before, "cleanup_plan.before"));
  const fieldUpdates = requiredRecord(plan.field_updates, "cleanup_plan.field_updates");
  const fields = Object.keys(fieldUpdates).sort();

  if (decision === "ready" && fields.length === 0) {
    throw new Error("ready cleanup_plan has no field updates");
  }
  if (decision !== "ready" && fields.length > 0) {
    throw new Error("non-ready cleanup_plan must not carry field updates");
  }

  const execute = decision === "ready";
  const after = cloneJson(before);
  for (const field of execute ? fields : []) {
    const update = requiredRecord(fieldUpdates[field], `cleanup_plan.field_updates.${field}`);
    if (!Object.hasOwn(update, "from") || !Object.hasOwn(update, "to")) {
      throw new Error(`${field} update is missing from or to`);
    }
    if (JSON.stringify(before[field] ?? null) !== JSON.stringify(update.from)) {
      throw new Error(`${field} update does not match current source state`);
    }
    after[field] = cloneJson(update.to);
  }

  return {
    write_result: {
      transport: "mock-crm",
      operation: "update_fields",
      status: execute ? "applied" : "no_op",
      executed: execute,
      record_id: requiredString(plan.record_id, "cleanup_plan.record_id"),
      applied_fields: execute ? fields : [],
      writes_attempted: execute ? 1 : 0,
      writes_completed: execute ? 1 : 0,
      before,
      after,
    },
  };
}

function validateSourceHandle(sourceHandle) {
  assertClosedKeys(
    sourceHandle,
    ["kind", "url", "allowlist", "record_id"],
    "source_handle",
  );
  if (requiredString(sourceHandle.kind, "source_handle.kind") !== "connector_export") {
    throw new Error("source_handle.kind must be connector_export");
  }
  const sourceUrl = requiredString(sourceHandle.url, "source_handle.url");
  if (sourceUrl.length > 2048) {
    throw new Error("source_handle.url exceeds 2048 characters");
  }
  if (!/^https:\/\/[^/\s]+(?:\/|$)/i.test(sourceUrl)) {
    throw new Error("source_handle.url must be a valid HTTPS URL");
  }
  if (!Array.isArray(sourceHandle.allowlist) || sourceHandle.allowlist.length === 0) {
    throw new Error("source_handle.allowlist must contain at least one host");
  }
  if (sourceHandle.allowlist.length > 64) {
    throw new Error("source_handle.allowlist exceeds 64 hosts");
  }
  for (const hostValue of sourceHandle.allowlist) {
    const host = requiredString(hostValue, "source_handle.allowlist host");
    if (host.length > 253 || !/^(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)(?:\.(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?))*$/.test(host)) {
      throw new Error("source_handle.allowlist contains an invalid host");
    }
  }
  const recordId = requiredString(sourceHandle.record_id, "source_handle.record_id");
  if (recordId.length > 500) throw new Error("source_handle.record_id exceeds 500 characters");
}

function validateSourceRead(sourceHandle, sourceRead) {
  if (sourceRead.decision !== "ready") {
    const blockers = Array.isArray(sourceRead.blockers)
      ? sourceRead.blockers.map((entry) => String(entry)).join("; ")
      : "";
    throw new Error(
      `source read was not ready: ${String(sourceRead.decision)}${blockers ? ` (${blockers})` : ""}`,
    );
  }
  if (!Number.isInteger(sourceRead.status) || sourceRead.status < 200 || sourceRead.status >= 300) {
    throw new Error(`source read returned HTTP ${String(sourceRead.status)}`);
  }
  const provenance = requiredRecord(sourceRead.provenance, "source_read.provenance");
  if (provenance.truncated === true) {
    throw new Error("source read was truncated");
  }
  const policy = requiredRecord(sourceRead.policy, "source_read.policy");
  if (policy.allowlist_decision !== "allowed") {
    throw new Error("source read did not pass the supplied allowlist");
  }
  const finalUrl = requiredString(sourceRead.final_url, "source_read.final_url");
  const requestedUrl = requiredString(sourceHandle.url, "source_handle.url");
  const contentDigest = requiredDigest(sourceRead.content_digest, "source_read.content_digest");
  return {
    handle_kind: requiredString(sourceHandle.kind, "source_handle.kind"),
    record_id: requiredString(sourceHandle.record_id, "source_handle.record_id"),
    allowlist: cloneJson(sourceHandle.allowlist),
    requested_url: requestedUrl,
    final_url: finalUrl,
    status: sourceRead.status,
    content_digest: contentDigest,
    fetched_at: requiredString(provenance.fetched_at, "source_read.provenance.fetched_at"),
    bytes: requiredNonNegativeInteger(provenance.bytes, "source_read.provenance.bytes"),
    truncated: false,
  };
}

function parseRecords(extracted) {
  if (typeof extracted !== "string" || !extracted.trim()) {
    throw new Error("source read did not return JSON text");
  }
  let payload;
  try {
    payload = JSON.parse(extracted);
  } catch {
    throw new Error("source read returned malformed JSON");
  }
  const records = Array.isArray(payload) ? payload : record(payload).records;
  if (!Array.isArray(records) || records.length === 0) {
    throw new Error("source export must contain at least one record");
  }
  return records.map((entry, index) => requiredRecord(entry, `source record ${index}`));
}

function validateFieldDefinition(field, definition) {
  const allowedKeys = new Set([
    "type",
    "enum",
    "max_length",
    "minimum",
    "maximum",
    "aliases",
    "semantic_role",
  ]);
  for (const key of Object.keys(definition)) {
    if (!allowedKeys.has(key)) {
      throw new Error(`crm_schema.fields.${field}.${key} is not supported`);
    }
  }
  const type = requiredString(definition.type, `crm_schema.fields.${field}.type`);
  if (!new Set(["string", "number", "boolean"]).has(type)) {
    throw new Error(`crm_schema.fields.${field}.type is not supported`);
  }
  if (definition.aliases !== undefined) {
    if (!Array.isArray(definition.aliases) || definition.aliases.length > 20) {
      throw new Error(`crm_schema.fields.${field}.aliases must contain at most 20 strings`);
    }
    for (const alias of definition.aliases) {
      if (typeof alias !== "string" || !alias.trim() || alias.length > 100) {
        throw new Error(`crm_schema.fields.${field}.aliases contains an invalid alias`);
      }
    }
  }
  if (
    definition.semantic_role !== undefined
    && !new Set(["status", "score", "next_action"]).has(definition.semantic_role)
  ) {
    throw new Error(`crm_schema.fields.${field}.semantic_role is not supported`);
  }
  if (definition.enum !== undefined) {
    if (!Array.isArray(definition.enum) || definition.enum.length === 0 || definition.enum.length > 100) {
      throw new Error(`crm_schema.fields.${field}.enum must contain 1 to 100 values`);
    }
    const enumMatchesType = definition.enum.every((value) => typeof value === type);
    if (!enumMatchesType) {
      throw new Error(`crm_schema.fields.${field}.enum values must match type ${type}`);
    }
  }
  if (definition.max_length !== undefined) {
    if (type !== "string" || !Number.isInteger(definition.max_length) || definition.max_length < 1 || definition.max_length > 10000) {
      throw new Error(`crm_schema.fields.${field}.max_length requires a string field and an integer from 1 to 10000`);
    }
  }
  if (definition.minimum !== undefined && (type !== "number" || !Number.isFinite(definition.minimum))) {
    throw new Error(`crm_schema.fields.${field}.minimum requires a finite number field`);
  }
  if (definition.maximum !== undefined && (type !== "number" || !Number.isFinite(definition.maximum))) {
    throw new Error(`crm_schema.fields.${field}.maximum requires a finite number field`);
  }
  if (
    typeof definition.minimum === "number" &&
    typeof definition.maximum === "number" &&
    definition.minimum > definition.maximum
  ) {
    throw new Error(`crm_schema.fields.${field}.minimum exceeds maximum`);
  }
}

function parseTypedValue(rawValue, definition, field) {
  const type = requiredString(definition.type, `crm_schema.fields.${field}.type`);
  let value;
  if (type === "string") {
    value = rawValue.trim();
    if (!value) throw new Error(`${field} must not be empty`);
    if (Number.isInteger(definition.max_length) && value.length > definition.max_length) {
      throw new Error(`${field} exceeds max_length ${definition.max_length}`);
    }
  } else if (type === "number") {
    if (!/^-?(?:\d+\.?\d*|\.\d+)$/.test(rawValue.trim())) {
      throw new Error(`${field} must be a number`);
    }
    value = Number(rawValue);
    if (!Number.isFinite(value)) throw new Error(`${field} must be finite`);
    if (typeof definition.minimum === "number" && value < definition.minimum) {
      throw new Error(`${field} is below minimum ${definition.minimum}`);
    }
    if (typeof definition.maximum === "number" && value > definition.maximum) {
      throw new Error(`${field} is above maximum ${definition.maximum}`);
    }
  } else if (type === "boolean") {
    const normalized = rawValue.trim().toLowerCase();
    if (normalized !== "true" && normalized !== "false") {
      throw new Error(`${field} must be true or false`);
    }
    value = normalized === "true";
  } else {
    throw new Error(`${field} uses unsupported type ${type}`);
  }

  if (Array.isArray(definition.enum) && !definition.enum.some((entry) => sameJsonValue(entry, value))) {
    throw new Error(`${field} must be one of the declared enum values`);
  }
  return value;
}

function requiredRecord(value, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value;
}

function assertClosedKeys(value, allowedKeys, label) {
  const allowed = new Set(allowedKeys);
  for (const key of Object.keys(value)) {
    if (!allowed.has(key)) throw new Error(`${label}.${key} is not supported`);
  }
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function requiredString(value, label) {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`${label} must be a non-empty string`);
  }
  return value.trim();
}

function requiredDigest(value, label) {
  const digest = requiredString(value, label);
  if (!/^sha256:[0-9a-f]{64}$/.test(digest)) {
    throw new Error(`${label} must be a sha256 digest`);
  }
  return digest;
}

function requiredNonNegativeInteger(value, label) {
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(`${label} must be a non-negative integer`);
  }
  return value;
}

function scalarKey(value) {
  return typeof value === "string" || typeof value === "number" ? String(value) : "";
}

function sameJsonValue(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function cloneJson(value) {
  return value === undefined ? null : JSON.parse(JSON.stringify(value));
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
  if (operation === "reconcile") return reconcileCleanup(inputs);
  if (operation === "transport") return applyMockCrmWrite(inputs);
  if (operation === "finalize") return finalizeCleanup(inputs);
  throw new Error("operation must be reconcile, transport, or finalize");
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
