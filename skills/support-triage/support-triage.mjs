// Domain checks bind the support judgment to supplied evidence. Delivery and
// its approval remain the responsibility of the downstream message operation.
export default function finalize(inputs) {
  const { support_request: request, product_evidence: documents = [], draft } = inputs;
  const sources = new Map([[request.source, `${request.subject || ""}\n${request.body}`]]);
  for (const document of documents) {
    if (sources.has(document.source_id)) throw new Error("support source references must be unique");
    sources.set(document.source_id, document.text);
  }
  const findings = [];
  if (!draft.evidence.length) findings.push("The judgment must cite the supplied customer request.");
  if (!draft.evidence.some((item) => item.source_id === request.source)) findings.push("Customer request evidence is missing.");
  for (const item of draft.evidence) {
    if (!sources.get(item.source_id)?.includes(item.quote)) findings.push(`Unbound support evidence: ${item.source_id}`);
  }
  if (draft.draft_email) {
    if (draft.classification !== "how_to" || draft.recommended_path !== "reply_draft") findings.push("This reply needs specialist review before customer drafting.");
    if (draft.missing_context.length) findings.push("Resolve missing context before presenting a definitive reply.");
    if (!draft.evidence.some((item) => item.source_id !== request.source && sources.has(item.source_id))) findings.push("A product answer requires supplied product evidence.");
  }
  return { support_triage_packet: {
    schema: "runx/support-triage.reply.v1",
    decision: findings.length ? "held" : "ready",
    classification: draft.classification,
    severity: draft.severity,
    recommended_path: findings.length ? "manual_review" : draft.recommended_path,
    rationale: draft.rationale,
    evidence: draft.evidence,
    missing_context: draft.missing_context,
    operator_notes: draft.operator_notes,
    draft_email: findings.length ? null : draft.draft_email,
    delivery_status: "not_sent",
    request_digest: inputs.request_digest,
    context_digest: inputs.context_digest,
    validation: { status: findings.length ? "fail" : "pass", findings },
  }};
}
