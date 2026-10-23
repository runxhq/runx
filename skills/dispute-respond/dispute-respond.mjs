const ISSUER_TYPES = new Set(["local", "hosted", "ci"]);
const INDEPENDENT_ISSUERS = new Set(["hosted", "ci"]);

export function admitDispute(inputs) {
  const findings = [];
  const add = (code, message) => findings.push({ code, message });
  const event = admitEvent(inputs.dispute_event, add);
  const originalReceiptId = opaque(inputs.original_receipt_id, "original_receipt_id", 256, add);
  const refundIds = uniqueStrings(inputs.prior_refund_receipt_ids, 100, "prior_refund_receipt_ids", add);
  const evidence = admitEvidence(inputs.evidence, add);
  const operatorPosture = text(inputs.operator_posture, 64);
  const allowedPostures = new Set(["accept", "contest", "refund_already_sent", "needs_more_evidence", "operator_review"]);
  if (operatorPosture && !allowedPostures.has(operatorPosture)) add("posture.invalid", "operator_posture is not supported");
  if (refundIds.includes(originalReceiptId)) add("receipt.duplicate_role", "the original charge receipt cannot also be a refund receipt");
  const receiptIds = [originalReceiptId, ...refundIds].filter(Boolean);
  if (new Set(receiptIds).size !== receiptIds.length) add("receipt.duplicate", "receipt ids must be unique");

  return {
    dispute_context: {
      schema: "runx/dispute-respond.dispute_context.v1",
      path: findings.length === 0 ? "review" : "stop",
      dispute_event: event,
      original_receipt_id: originalReceiptId,
      prior_refund_receipt_ids: refundIds,
      receipt_ids: receiptIds,
      evidence,
      operator_posture: operatorPosture,
      findings,
    },
  };
}

export function finalizeResponse(inputs) {
  const context = object(inputs.dispute_context);
  const receiptProof = object(inputs.receipt_proof);
  const draft = object(inputs.response_draft);
  const findings = array(context.findings).slice();
  const add = (code, message) => findings.push({ code, message });
  const expectedReceiptIds = strings(context.receipt_ids, 101);
  const requestedIds = strings(receiptProof.requested_receipt_ids, 101);
  const matchedIds = array(receiptProof.matched_receipts).map((item) => text(object(item).receipt_id, 256)).filter(Boolean);
  const details = array(receiptProof.receipt_details).map(object);
  const verification = object(receiptProof.verification);

  if (context.path === "review") {
    if (!sameSet(requestedIds, expectedReceiptIds)) add("receipt.request_mismatch", "native receipt proof did not evaluate the exact admitted receipt ids");
    if (receiptProof.decision !== "verified") add("receipt.proof_unverified", "linked receipts require complete signature verification");
    if (verification.intact !== true) add("receipt.tree_unverified", "one or more requested receipt trees are incomplete or invalid");
    for (const receiptId of expectedReceiptIds) {
      if (!matchedIds.includes(receiptId)) add("receipt.unresolved", `native receipt store did not resolve receipt: ${receiptId}`);
      const detail = details.find((item) => text(item.id, 256) === receiptId);
      if (!detail) add("receipt.detail_missing", `redacted native detail is missing for receipt: ${receiptId}`);
      else if (object(detail.verification).status !== "verified") add("receipt.detail_unverified", `receipt detail is not verified: ${receiptId}`);
      // A dispute cites receipts a party other than this operator can rely
      // on: a hosted or CI issuer's, never a runx home's own local key.
      else if (!INDEPENDENT_ISSUERS.has(detail.issuer_type)) add("receipt.local_issuer", `receipt is not signed by a hosted or CI issuer: ${receiptId}`);
    }
    validateDraft({ context, draft, expectedReceiptIds, add });
  }

  const evidence = array(context.evidence);
  const citedReceiptIds = strings(draft.cited_receipt_ids, 101);
  const citedEvidenceLocators = strings(draft.cited_evidence_locators, 100);
  const posture = text(draft.posture, 64);
  const ready = context.path === "review" && findings.length === 0;
  const event = object(context.dispute_event);

  return {
    dispute_packet: {
      schema: "runx/dispute-respond.response.v1",
      decision: ready ? "ready_for_review" : "needs_more_evidence",
      dispute: event,
      posture: ready ? posture : "needs_more_evidence",
      response_summary: text(draft.response_summary, 10_000),
      linked_receipts: {
        original_charge: context.original_receipt_id || "",
        prior_refunds: array(context.prior_refund_receipt_ids),
        cited: citedReceiptIds,
      },
      evidence: { admitted: evidence, cited_locators: citedEvidenceLocators },
      open_questions: strings(draft.open_questions, 100),
      receipt_verification: {
        matched_receipt_ids: matchedIds,
        original_receipt_status: details.find((item) => item.id === context.original_receipt_id)?.verification?.status || "unresolved",
        issuer_types: matchedIds.map((receiptId) => details.find((item) => item.id === receiptId)?.issuer_type).filter((issuerType) => ISSUER_TYPES.has(issuerType)),
        tree_status: verification.intact === true ? "intact" : "unverified",
      },
      validation: { status: ready ? "pass" : "fail", findings },
      filing: {
        provider: event.provider || "",
        dispute_id: event.dispute_id || "",
        status: "not_filed",
        provider_status: "not_called",
        approval_status: "not_requested",
      },
      next_action: ready ? "review, then route through an approved provider dispute adapter" : "resolve the recorded receipt or evidence gaps",
    },
  };
}

function admitEvent(value, add) {
  const candidate = object(value);
  const allowed = new Set(["dispute_id", "provider", "provider_charge_id", "amount_minor", "currency", "reason_code", "response_due_at"]);
  const extras = Object.keys(candidate).filter((key) => !allowed.has(key));
  if (extras.length > 0) add("dispute.raw_fields", `dispute_event contains unsupported fields: ${extras.join(", ")}`);
  const disputeId = opaque(candidate.dispute_id, "dispute.dispute_id", 256, add);
  const provider = opaque(candidate.provider, "dispute.provider", 64, add);
  const providerChargeId = opaque(candidate.provider_charge_id, "dispute.provider_charge_id", 256, add);
  const amountMinor = Number.isSafeInteger(candidate.amount_minor) && candidate.amount_minor > 0 ? candidate.amount_minor : null;
  if (!amountMinor) add("dispute.amount", "dispute_event.amount_minor must be a positive safe integer");
  const currency = text(candidate.currency, 3);
  if (!/^[A-Z]{3}$/u.test(currency)) add("dispute.currency", "dispute_event.currency must be an uppercase ISO 4217 code");
  const reasonCode = opaque(candidate.reason_code, "dispute.reason_code", 128, add);
  const responseDueAt = text(candidate.response_due_at, 64);
  if (responseDueAt && Number.isNaN(Date.parse(responseDueAt))) add("dispute.deadline", "dispute_event.response_due_at must be an ISO timestamp");
  return { dispute_id: disputeId, provider, provider_charge_id: providerChargeId, amount_minor: amountMinor, currency, reason_code: reasonCode, response_due_at: responseDueAt || null };
}

function admitEvidence(value, add) {
  const values = Array.isArray(value) ? value.slice(0, 100) : [];
  if (Array.isArray(value) && value.length > 100) add("evidence.limit", "evidence is limited to 100 items");
  const seen = new Set();
  return values.map((candidate, index) => {
    const item = object(candidate);
    const allowed = new Set(["locator", "digest", "kind", "summary"]);
    const extras = Object.keys(item).filter((key) => !allowed.has(key));
    if (extras.length > 0) add("evidence.raw_fields", `evidence[${index}] contains unsupported fields: ${extras.join(", ")}`);
    const locator = opaque(item.locator, `evidence[${index}].locator`, 512, add);
    const digest = text(item.digest, 80);
    if (!/^sha256:[0-9a-f]{64}$/u.test(digest)) add("evidence.digest", `evidence[${index}].digest must be sha256`);
    const kind = text(item.kind, 64);
    if (!new Set(["delivery", "consent", "support", "provider", "contract", "refund"]).has(kind)) add("evidence.kind", `evidence[${index}].kind is unsupported`);
    const summary = text(item.summary, 1_000);
    if (!summary) add("evidence.summary", `evidence[${index}].summary is required`);
    if (seen.has(locator)) add("evidence.duplicate", `duplicate evidence locator: ${locator}`);
    seen.add(locator);
    return { locator, digest, kind, summary };
  });
}

function validateDraft({ context, draft, expectedReceiptIds, add }) {
  const allowedFields = new Set(["posture", "response_summary", "cited_receipt_ids", "cited_evidence_locators", "open_questions"]);
  const extras = Object.keys(draft).filter((key) => !allowedFields.has(key));
  if (extras.length > 0) add("draft.effect_fields", `response draft contains unsupported fields: ${extras.join(", ")}`);
  const allowedPostures = new Set(["accept", "contest", "refund_already_sent", "needs_more_evidence", "operator_review"]);
  const posture = text(draft.posture, 64);
  if (!allowedPostures.has(posture)) add("draft.posture", "response posture is invalid");
  if (!text(draft.response_summary, 10_000)) add("draft.summary", "response_summary is required");
  const admittedReceipts = new Set(expectedReceiptIds);
  const citedReceipts = strings(draft.cited_receipt_ids, 101);
  if (!citedReceipts.includes(context.original_receipt_id)) add("draft.original_receipt", "response must cite the original charge receipt");
  for (const receiptId of citedReceipts) if (!admittedReceipts.has(receiptId)) add("draft.receipt_unbound", `response cites an unadmitted receipt: ${receiptId}`);
  const admittedEvidence = new Map(array(context.evidence).map((item) => [text(object(item).locator, 512), object(item)]));
  const citedEvidence = strings(draft.cited_evidence_locators, 100);
  for (const locator of citedEvidence) if (!admittedEvidence.has(locator)) add("draft.evidence_unbound", `response cites unadmitted evidence: ${locator}`);
  if (posture === "contest" && !citedEvidence.some((locator) => ["delivery", "consent"].includes(admittedEvidence.get(locator)?.kind))) {
    add("draft.contest_evidence", "contest requires cited delivery or consent evidence");
  }
  if (array(context.prior_refund_receipt_ids).length > 0 && !["refund_already_sent", "operator_review"].includes(posture)) {
    add("draft.prior_refund", "a prior refund requires refund_already_sent or operator_review posture");
  }
}

function uniqueStrings(value, max, field, add) {
  if (!Array.isArray(value)) return [];
  if (value.length > max) add(`${field}.limit`, `${field} is limited to ${max} items`);
  return value.slice(0, max).map((item, index) => opaque(item, `${field}[${index}]`, 256, add)).filter(Boolean);
}

function opaque(value, field, max, add) {
  const result = text(value, max);
  if (!result) add(`${field}.missing`, `${field} is required`);
  if (result && (/\s/u.test(result) || /^(?:sk-|bearer:|-----begin)/iu.test(result))) {
    add(`${field}.unsafe`, `${field} must be an opaque non-secret reference`);
    return "";
  }
  return result;
}

function object(value) { return value && typeof value === "object" && !Array.isArray(value) ? value : {}; }
function array(value) { return Array.isArray(value) ? value : []; }
function strings(value, max) { return array(value).map((item) => text(item, 1_000)).filter(Boolean).slice(0, max); }
function text(value, max) { return typeof value === "string" ? value.trim().slice(0, max) : ""; }
function sameSet(left, right) { return left.length === right.length && new Set(left).size === left.length && left.every((item) => right.includes(item)); }
