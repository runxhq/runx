export function prepareReceiptQuery(inputs) {
  const question = stringValue(inputs.question);
  const filter = readFilter(inputs.filter);
  const receiptIds = stringArray(inputs.receipt_ids);
  const proofRequested = record(inputs.proof).verify_chain === true;
  const replay = Array.isArray(inputs.receipts);
  return {
    ledger_context: {
      path: !question ? "no_question" : replay ? "replay" : "live",
      question: question || "",
      filter,
      native_status: filter.status.length === 1 ? filter.status[0] : null,
      receipt_ids: receiptIds,
      receipt_ids_supplied: Object.prototype.hasOwnProperty.call(inputs, "receipt_ids"),
      proof_requested: proofRequested,
      query: {
        principal: filter.principal || "",
        skill_ref: filter.skill_ref || "",
        status: filter.status,
        time_range: { from: filter.from || "", to: filter.to || "" },
        receipt_ids: receiptIds,
      },
    },
  };
}

export function finalizeReceiptQuery(inputs) {
  const context = requiredRecord(inputs.ledger_context, "ledger_context");
  if (context.path === "no_question") return noQuestion(context);
  const replay = context.path === "replay";
  const native = record(inputs.receipt_query);
  const rows = replay ? records(inputs.receipts) : records(native.receipts);
  const matched = rows
    .map(projectIdStub)
    .filter((stub) => matchesFilter(stub, context.filter))
    .filter((stub) => !context.receipt_ids_supplied || context.receipt_ids.includes(stub.receipt_id))
    .slice(0, context.filter.limit);
  const matchedIds = new Set(matched.map((receipt) => receipt.receipt_id));
  const detailRows = replay ? records(inputs.receipt_details) : records(native.receipt_details);
  const details = detailRows
    .map(projectReceiptDetail)
    .filter((detail) => matchedIds.has(detail.id))
    .slice(0, Math.min(context.filter.limit, 100));
  const chain = chainVerification({
    requested: context.proof_requested,
    replay,
    native: record(native.verification),
  });
  const decision = matched.length === 0 ? "needs_more_evidence" : "answered";
  return packet({
    decision,
    question: context.question,
    query: context.query,
    matched,
    details,
    chain,
    summary: renderSummary({ decision, matched, chain, proofRequested: context.proof_requested, query: context.query }),
  });
}

function noQuestion(context) {
  return packet({
    decision: "needs_agent",
    question: "",
    query: context.query,
    matched: [],
    details: [],
    chain: { checked: false, intact: null, breaks: [] },
    summary: "No audit question was provided, so there is nothing to query against the ledger.",
  });
}

function packet({ decision, question, query, matched, details, chain, summary }) {
  return {
    ledger_answer: { decision, question, query },
    matched_receipts: matched,
    receipt_details: details,
    chain_verification: chain,
    summary,
  };
}

function chainVerification({ requested, replay, native }) {
  if (!requested) return { checked: false, intact: null, breaks: [] };
  if (replay) return { checked: true, intact: null, breaks: [] };
  const breaks = records(native.findings).map((finding) => ({
    from_receipt_id: stringValue(finding.root_receipt_id) || "",
    to_receipt_id: stringValue(finding.path) || "",
    reason: stringValue(finding.message) || stringValue(finding.code) || "verification finding",
  }));
  const intact = typeof native.intact === "boolean" ? native.intact : null;
  return { checked: true, intact, breaks };
}

function readFilter(value) {
  const filter = record(value);
  const timeRange = record(filter.time_range);
  const status = Array.isArray(filter.status)
    ? filter.status.map(stringValue).filter(Boolean)
    : stringValue(filter.status)
      ? [stringValue(filter.status)]
      : [];
  return {
    principal: stringValue(filter.principal),
    skill_ref: stringValue(filter.skill_ref),
    status,
    from: stringValue(timeRange.from),
    to: stringValue(timeRange.to),
    issuer: stringValue(filter.issuer),
    actor: stringValue(filter.actor) || stringValue(filter.principal),
    limit: boundedLimit(filter.limit),
  };
}

function projectReceiptDetail(value) {
  const detail = requiredRecord(value, "receipt detail");
  const id = requiredString(detail.id, "receipt detail id");
  return {
    id,
    receipt_ref: stringValue(detail.receipt_ref) || `runx:receipt:${id}`,
    subject_kind: stringValue(detail.subject_kind) || "",
    subject_ref: stringValue(detail.subject_ref) || "",
    created_at: stringValue(detail.created_at) || "",
    status: stringValue(detail.status) || "",
    verification: record(detail.verification),
    authority: projectAuthority(detail.authority),
    decisions: records(detail.decisions).map(projectDecision),
    acts: records(detail.acts).map(projectAct),
    artifact_refs: stringArray(detail.artifact_refs, 500),
    lineage_refs: stringArray(detail.lineage_refs, 500),
    seal_reason_code: stringValue(detail.seal_reason_code) || "",
  };
}

function projectAuthority(value) {
  const authority = record(value);
  return {
    actor_ref: stringValue(authority.actor_ref),
    grant_refs: stringArray(authority.grant_refs, 500),
    scope_refs: stringArray(authority.scope_refs, 500),
    exercised_scopes: records(authority.exercised_scopes).map((entry) => ({
      scope: stringValue(entry.scope) || "",
      source: stringValue(entry.source) || "",
      term_id: stringValue(entry.term_id),
      resource_ref: stringValue(entry.resource_ref),
    })).filter((entry) => entry.scope),
    approval_refs: stringArray(authority.approval_refs, 500),
    term_count: nonNegativeInteger(authority.term_count),
    credential_ref_count: nonNegativeInteger(authority.credential_ref_count),
  };
}

function projectDecision(value) {
  const decision = record(value);
  return {
    id: stringValue(decision.id) || "",
    choice: stringValue(decision.choice) || "",
    selected_act_id: stringValue(decision.selected_act_id) || "",
  };
}

function projectAct(value) {
  const act = record(value);
  return {
    id: stringValue(act.id) || "",
    form: stringValue(act.form) || "",
    purpose: stringValue(act.purpose) || "",
    legitimacy: stringValue(act.legitimacy) || "",
    reason: stringValue(act.reason),
    target_refs: stringArray(act.target_refs, 500),
    artifact_refs: stringArray(act.artifact_refs, 500),
    criterion_statuses: records(act.criterion_statuses).map((criterion) => ({
      criterion_id: stringValue(criterion.criterion_id) || "",
      status: stringValue(criterion.status) || "",
      evidence_refs: stringArray(criterion.evidence_refs, 500),
      verification_refs: stringArray(criterion.verification_refs, 500),
    })).filter((criterion) => criterion.criterion_id),
    context_ref_present: act.context_ref_present === true,
  };
}

function projectIdStub(row) {
  const receipt = requiredRecord(row, "ledger row");
  const receiptId = stringValue(receipt.receipt_id) || stringValue(receipt.id);
  if (!receiptId) throw new Error("ledger row is missing a receipt id");
  return {
    receipt_id: receiptId,
    skill_ref: stringValue(receipt.skill_ref) || stringValue(receipt.name) || "",
    status: stringValue(receipt.status) || "",
    created_at: stringValue(receipt.created_at) || "",
    verification_status: stringValue(receipt.verification_status) || stringValue(receipt.verification?.status) || "unknown",
  };
}

function matchesFilter(stub, filter) {
  if (filter.skill_ref && stub.skill_ref !== filter.skill_ref) return false;
  if (filter.status.length > 0 && !filter.status.includes(stub.status)) return false;
  if (filter.from && stub.created_at && stub.created_at < filter.from) return false;
  if (filter.to && stub.created_at && stub.created_at > filter.to) return false;
  return true;
}

function renderSummary({ decision, matched, chain, proofRequested, query }) {
  if (decision === "needs_more_evidence") {
    return `No receipts matched the resolved query against ${query.skill_ref || query.principal || "the ledger"}; the gap is the query, not a confirmed zero.`;
  }
  const noun = matched.length === 1 ? "receipt" : "receipts";
  if (!proofRequested) return `${matched.length} ${noun} matched the resolved query; chain verification was not requested.`;
  if (chain.intact === null) return `${matched.length} ${noun} matched the resolved query; the chain is unverified because bounded proof evidence was unavailable.`;
  if (chain.intact) return `${matched.length} ${noun} matched the resolved query, and the engine's tree-rooted verify verdict is intact.`;
  return `${matched.length} ${noun} matched the resolved query, but the engine's tree-rooted verify verdict reports ${chain.breaks.length} break(s).`;
}

function boundedLimit(value) {
  if (value === undefined || value === null || value === "") return 500;
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < 1 || parsed > 5_000) throw new Error("filter.limit must be an integer from 1 to 5000");
  return parsed;
}

function stringArray(value, max = 100) {
  const values = Array.isArray(value) ? [...new Set(value.map(stringValue).filter(Boolean))] : [];
  if (values.length > max) throw new Error(`array may contain at most ${max} entries`);
  return values;
}

function nonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0 ? value : 0;
}

function records(value) {
  return Array.isArray(value) ? value.map(record) : [];
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be an object`);
  return parsed;
}

function requiredString(value, field) {
  const parsed = stringValue(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}
