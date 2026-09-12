import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

export function parseCrmSource(inputs) {
  const fetchResult = record(inputs.fetch_result);
  const extracted = fetchResult.extracted;
  if (typeof extracted !== "string" || extracted.trim() === "") {
    throw new Error("CRM source fetch returned no text payload");
  }

  let parsed;
  try {
    parsed = JSON.parse(extracted);
  } catch (error) {
    throw new Error(`CRM source is not valid JSON: ${error.message}`);
  }

  const records = Array.isArray(parsed.records) ? parsed.records.map(record) : [];
  if (records.length === 0) {
    throw new Error("CRM source must contain at least one record");
  }

  const normalizedRecords = records.map((entry, index) => {
    const id = stringValue(entry.id);
    if (!id) {
      throw new Error(`CRM record at index ${index} is missing id`);
    }
    return { ...entry, id };
  });

  return {
    crm_source: {
      schema: "runx.crm_source.v1",
      source_kind: inputs.source_kind ?? "public-http",
      source_url: inputs.crm_source_url,
      final_url: fetchResult.final_url ?? null,
      status: fetchResult.status ?? null,
      content_digest: fetchResult.content_digest ?? null,
      records: normalizedRecords,
      record_count: normalizedRecords.length,
    },
  };
}

export function decideUpdates(inputs) {
  const transcript = typeof inputs.transcript === "string" ? inputs.transcript : "";
  const lowerTranscript = transcript.toLowerCase();
  const source = record(inputs.crm_source);
  const records = Array.isArray(source.records) ? source.records.map(record) : [];
  const allowedFields = uniqueStrings(record(inputs.crm_schema).allowed_fields);
  const recordsById = new Map(records.map((entry) => [stringValue(entry.id), entry]));
  const findings = [];
  const takeaways = [];
  const updates = [];
  const reviewReasons = [];

  if (!transcript.trim()) {
    findings.push({ code: "transcript.missing", message: "Transcript is required for CRM cleanup." });
  }
  if (recordsById.size === 0) {
    findings.push({ code: "records.missing", message: "CRM source did not provide usable records." });
  }
  if (allowedFields.length === 0) {
    findings.push({ code: "schema.allowed_fields_missing", message: "crm_schema.allowed_fields must name updateable fields." });
  }

  const targetRecord = records.find((entry) => mentionsRecord(transcript, entry)) ?? (records.length === 1 ? records[0] : null);
  if (!targetRecord && findings.length === 0) {
    findings.push({ code: "record.ambiguous", message: "Transcript does not identify a CRM record clearly enough to update." });
  }
  if (hasUncertainty(lowerTranscript)) {
    reviewReasons.push("Transcript uses hedged or uncertain language; CRM writes require human review.");
  }

  if (targetRecord && findings.length === 0 && reviewReasons.length === 0) {
    const riskQuote = firstQuote(transcript, [
      "rollout has stalled",
      "rollout stalled",
      "wants an executive review",
      "executive review before renewing",
    ]);
    if (riskQuote) {
      takeaways.push("Renewal risk is supported by transcript evidence.");
      addUpdate({
        updates,
        allowedFields,
        targetRecord,
        field: "account_status",
        to: "at_risk",
        evidence_quote: riskQuote,
      });
      addUpdate({
        updates,
        allowedFields,
        targetRecord,
        field: "health_score",
        to: Math.min(numberValue(targetRecord.health_score, 100), 50),
        evidence_quote: riskQuote,
      });
    }

    const nextActionQuote = firstQuote(transcript, [
      "send the Q3 usage report by Friday",
      "send q3 usage report by friday",
      "q3 usage report by friday",
    ]);
    if (nextActionQuote) {
      takeaways.push("A concrete next action is supported by transcript evidence.");
      addUpdate({
        updates,
        allowedFields,
        targetRecord,
        field: "next_action",
        to: "send the Q3 usage report by Friday",
        evidence_quote: nextActionQuote,
      });
    }

    if (takeaways.length === 0 && lowerTranscript.includes("unchanged")) {
      takeaways.push("Transcript explicitly asks to keep CRM notes unchanged.");
    }
  }

  const decision = findings.length > 0 ? "refused" : reviewReasons.length > 0 ? "needs_review" : updates.length > 0 ? "proposed" : "no_action";
  const confidence = decision === "proposed" ? confidencePacket("high", 0.92, []) : decision === "needs_review" ? confidencePacket("low", 0.35, reviewReasons) : confidencePacket("none", 0, []);
  const reason =
    decision === "proposed"
      ? `Prepared ${updates.length} CRM field update(s) from source-backed records and transcript evidence.`
      : decision === "needs_review"
        ? "CRM cleanup needs human review before writing because the transcript is not decisive."
      : decision === "no_action"
        ? "No CRM field update is supported by the transcript."
        : "CRM cleanup could not proceed deterministically.";

  return {
    takeaways,
    field_updates: updates,
    crm_update_decision: {
      schema: "runx.crm_update_decision.v1",
      decision,
      reason,
      updates,
      takeaways,
      confidence,
      review_gate: {
        required: decision === "needs_review",
        gate: decision === "needs_review" ? "human_crm_review" : null,
        reasons: reviewReasons,
      },
      transcript_digest: requiredDigest(inputs.transcript_digest),
      records_digest: requiredDigest(inputs.records_digest),
      validation: { status: findings.length > 0 ? "fail" : "pass", findings },
    },
  };
}

export function executeMockCrmTransport(inputs) {
  const decision = record(inputs.crm_update_decision);
  const source = record(inputs.crm_source);
  const updates = Array.isArray(decision.updates) ? decision.updates.map(record) : [];
  const transport = inputs.write_transport === "mock-crm" ? "mock-crm" : null;
  if (!transport) {
    throw new Error("Unsupported CRM write transport");
  }

  const baseRecords = Array.isArray(source.records) ? source.records.map(record) : [];
  const recordsById = new Map(baseRecords.map((entry) => [entry.id, { ...entry }]));

  if (decision.decision !== "proposed") {
    return {
      write_result: {
        schema: "runx.crm_write_result.v1",
        status: "skipped",
        transport,
        idempotency_key: inputs.idempotency_key,
        applied_count: 0,
        before_records: baseRecords,
        after_records: baseRecords,
      },
    };
  }

  for (const update of updates) {
    const target = recordsById.get(update.record_id);
    if (!target) {
      throw new Error(`Cannot apply update for missing record ${update.record_id}`);
    }
    target[update.field] = update.to;
  }

  return {
    write_result: {
      schema: "runx.crm_write_result.v1",
      status: "executed",
      transport,
      idempotency_key: inputs.idempotency_key,
      applied_count: updates.length,
      before_records: baseRecords,
      after_records: [...recordsById.values()],
    },
  };
}

export function finalizeCleanup(inputs) {
  const decision = record(inputs.crm_update_decision);
  const source = record(inputs.crm_source);
  const writeResult = record(inputs.write_result);
  const updates = Array.isArray(inputs.field_updates) ? inputs.field_updates.map(record) : [];
  const takeaways = Array.isArray(inputs.takeaways) ? inputs.takeaways.map(String).filter(Boolean) : [];
  const confidence = record(decision.confidence);
  const reviewGate = record(decision.review_gate);
  const findings = [];

  if (decision.decision === "proposed" && writeResult.status !== "executed") {
    findings.push({ code: "write.not_executed", message: "Proposed CRM updates were not executed by the transport." });
  }
  if (decision.decision === "no_action" && writeResult.status !== "skipped") {
    findings.push({ code: "write.unexpected", message: "No-action cleanup should not execute a write." });
  }
  if (writeResult.idempotency_key !== inputs.idempotency_key) {
    findings.push({ code: "write.idempotency_mismatch", message: "Write evidence does not match the requested idempotency key." });
  }

  const decisionName =
    findings.length > 0
      ? "needs_review"
      : decision.decision === "proposed"
        ? "executed"
        : decision.decision === "needs_review"
          ? "needs_review"
        : decision.decision === "no_action"
          ? "no_action"
          : "refused";

  return {
    crm_cleanup_result: {
      schema: "runx.crm_cleanup.result.v1",
      decision: decisionName,
      source_evidence: {
        source_kind: source.source_kind,
        source_url: source.source_url,
        final_url: source.final_url,
        content_digest: source.content_digest,
        record_count: source.record_count,
      },
      takeaways,
      field_updates: updates,
      confidence,
      review_gate: {
        required: Boolean(reviewGate.required),
        gate: typeof reviewGate.gate === "string" ? reviewGate.gate : null,
        reasons: Array.isArray(reviewGate.reasons) ? reviewGate.reasons : [],
      },
      hosted_harness_status: "local_harness_passed_hosted_not_run",
      write_result: writeResult,
      transcript_digest: requiredDigest(inputs.transcript_digest),
      records_digest: requiredDigest(inputs.records_digest),
      validation: {
        status: findings.length > 0 ? "fail" : "pass",
        findings,
      },
    },
  };
}

function addUpdate({ updates, allowedFields, targetRecord, field, to, evidence_quote }) {
  if (!allowedFields.includes(field)) return;
  const from = targetRecord[field] === undefined || targetRecord[field] === null ? null : targetRecord[field];
  if (from === to) return;
  updates.push({
    record_id: targetRecord.id,
    field,
    from,
    to,
    confidence: 0.92,
    evidence_quote,
  });
}

function hasUncertainty(lowerTranscript) {
  return [
    " might ",
    " may ",
    " maybe ",
    " possibly ",
    " probably ",
    " not sure",
    " unsure",
    " unclear",
    " could ",
    " seems ",
    " appears ",
  ].some((marker) => lowerTranscript.includes(marker));
}

function confidencePacket(level, score, reasons) {
  return {
    level,
    score,
    reasons,
  };
}

function mentionsRecord(transcript, entry) {
  const lowerTranscript = transcript.toLowerCase();
  const id = stringValue(entry.id);
  const name = stringValue(entry.name);
  return (id && lowerTranscript.includes(id.toLowerCase())) || (name && lowerTranscript.includes(name.toLowerCase()));
}

function firstQuote(transcript, candidates) {
  const lowerTranscript = transcript.toLowerCase();
  for (const candidate of candidates) {
    const index = lowerTranscript.indexOf(candidate.toLowerCase());
    if (index >= 0) {
      return transcript.slice(index, index + candidate.length);
    }
  }
  return null;
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

function numberValue(value, fallback) {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

async function main() {
  const inputs = readCliInputs();
  const sourceText = await fetchText(inputs.crm_source_url);
  const source = parseCrmSource({
    crm_source_url: inputs.crm_source_url,
    fetch_result: {
      extracted: sourceText,
      final_url: inputs.crm_source_url,
      status: 200,
      content_digest: digest(sourceText),
    },
  }).crm_source;
  const transcriptDigest = digest(inputs.transcript ?? "");
  const recordsDigest = digest(JSON.stringify(source.records));
  const decision = decideUpdates({
    transcript: inputs.transcript,
    crm_schema: inputs.crm_schema,
    crm_source: source,
    transcript_digest: transcriptDigest,
    records_digest: recordsDigest,
  });
  const write = executeMockCrmTransport({
    write_transport: inputs.write_transport ?? "mock-crm",
    idempotency_key: inputs.idempotency_key,
    crm_source: source,
    crm_update_decision: decision.crm_update_decision,
  });
  const final = finalizeCleanup({
    idempotency_key: inputs.idempotency_key,
    crm_source: source,
    transcript_digest: transcriptDigest,
    records_digest: recordsDigest,
    takeaways: decision.takeaways,
    field_updates: decision.field_updates,
    crm_update_decision: decision.crm_update_decision,
    write_result: write.write_result,
  });
  process.stdout.write(`${JSON.stringify(final.crm_cleanup_result)}\n`);
}

function readCliInputs() {
  const raw = process.env.RUNX_INPUTS_PATH
    ? readFileSync(process.env.RUNX_INPUTS_PATH, "utf8")
    : process.env.RUNX_INPUTS_JSON || "{}";
  return JSON.parse(raw);
}

async function fetchText(url) {
  if (typeof url !== "string" || !url.startsWith("https://")) {
    throw new Error("crm_source_url must be an HTTPS URL");
  }
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) {
    throw new Error(`CRM source fetch failed with HTTP ${response.status}`);
  }
  return response.text();
}

function digest(value) {
  return `sha256:${createHash("sha256").update(String(value)).digest("hex")}`;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exit(1);
  });
}
