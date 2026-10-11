export default function auditLeastPrivilege(inputs) {
  const subject = stringValue(inputs.subject);
  const grantedScopes = stringArray(inputs.granted_scopes, "granted_scopes");
  const receiptIds = stringArray(inputs.receipt_ids, "receipt_ids");
  const evidence = record(inputs.ledger_evidence);
  const chain = record(evidence.chain_verification);
  const requested = new Set(receiptIds);
  const matchedIds = new Set(records(evidence.matched_receipts)
    .map((row) => stringValue(row.receipt_id))
    .filter((id) => requested.has(id)));
  const missingIds = receiptIds.filter((id) => !matchedIds.has(id));
  const details = new Map();
  const invalidIds = new Set();

  for (const detail of records(evidence.receipt_details)) {
    const id = stringValue(detail.id);
    if (!requested.has(id) || !matchedIds.has(id)) continue;
    if (details.has(id) || detail.verification?.status !== "verified" || !belongsToSubject(detail, subject)) {
      invalidIds.add(id);
    }
    details.set(id, detail);
  }
  const missingDetailIds = receiptIds.filter((id) => !details.has(id));
  const chainVerified = chain.checked === true && chain.intact === true
    && Array.isArray(chain.breaks) && chain.breaks.length === 0;
  const evidenceReady = Boolean(subject) && receiptIds.length > 0
    && missingIds.length === 0 && missingDetailIds.length === 0
    && invalidIds.size === 0 && chainVerified;
  const limitations = [];
  if (!subject) limitations.push("Supply the exact skill, actor or grant reference being reviewed.");
  if (receiptIds.length === 0) limitations.push("No exact receipt ids were supplied.");
  if (missingIds.length > 0) limitations.push("Some requested receipts were not resolved.");
  if (missingDetailIds.length > 0) limitations.push("Some requested receipts have no native redacted detail.");
  if (invalidIds.size > 0) limitations.push("Some receipt details are duplicated, unverified or belong to a different subject.");
  if (!chainVerified) limitations.push("Native chain verification is missing, unknown or failed; supplied replay labels are not proof.");
  limitations.push("Native scope projections describe recorded authority references and terms, not a per-operation successful-usage trace.");
  limitations.push("A bounded receipt window cannot prove that unrecorded authority is unnecessary, including reserved or infrequent work.");
  limitations.push("Scope strings are opaque. Native authority semantics must establish any proposed replacement; this skill does not infer wildcard or verb relationships.");

  const recorded = evidenceReady ? collectRecordedAuthority(details.values()) : new Map();
  const scopeDiff = grantedScopes.map((scope) => {
    const authority = recorded.get(scope);
    return {
      granted_scope: scope,
      classification: !evidenceReady ? "defer" : authority ? "recorded" : "not_recorded",
      recorded_authority: {
        receipt_count: authority?.receiptIds.size || 0,
        receipt_refs: [...(authority?.receiptIds || [])],
        sources: [...(authority?.sources || [])],
      },
      rationale: !evidenceReady
        ? "Evidence is incomplete; retain this scope."
        : authority
          ? "Verified receipts record this exact scope; this does not prove its successful use or necessity."
          : "This exact scope is not recorded in the selected receipts; retain it until policy and coverage review supports a change.",
    };
  });
  const status = evidenceReady ? "review_required" : "needs_more_evidence";
  return {
    audit_report: {
      status,
      subject: subject || "unknown",
      evidence: {
        receipt_ids: receiptIds,
        matched_receipt_ids: [...matchedIds],
        missing_receipt_ids: missingIds,
        missing_detail_ids: missingDetailIds,
        invalid_detail_ids: [...invalidIds],
        chain_verification: chain,
        observation_source: evidenceReady ? "native_receipt_detail" : "none",
        limitations,
      },
      scope_diff: scopeDiff,
      retained_grant: grantedScopes,
      not_recorded_scopes: scopeDiff.filter((row) => row.classification === "not_recorded").map((row) => row.granted_scope),
      reviewer_action: evidenceReady ? "review_policy_and_coverage" : "gather_verified_evidence",
    },
    attenuation_proposals: [],
    verdict: evidenceReady
      ? "review_required: compare recorded authority with policy and representative usage before narrowing the grant"
      : "needs_more_evidence: retain the grant until exact attributable receipt proof is available",
  };
}

function belongsToSubject(detail, subject) {
  return subject !== null && [detail.subject_ref, detail.authority?.actor_ref,
    ...(Array.isArray(detail.authority?.grant_refs) ? detail.authority.grant_refs : [])].includes(subject);
}

function collectRecordedAuthority(details) {
  const recorded = new Map();
  for (const detail of details) {
    for (const entry of records(detail.authority?.exercised_scopes)) {
      const scope = stringValue(entry.scope);
      if (!scope) continue;
      const authority = recorded.get(scope) || { receiptIds: new Set(), sources: new Set() };
      authority.receiptIds.add(detail.id);
      const source = stringValue(entry.source);
      if (source) authority.sources.add(source);
      recorded.set(scope, authority);
    }
  }
  return recorded;
}

function stringArray(value, field) {
  if (!Array.isArray(value)) throw new Error(`${field} must be an array`);
  return [...new Set(value.map((entry) => {
    const parsed = stringValue(entry);
    if (!parsed) throw new Error(`${field} entries must be non-empty strings`);
    return parsed;
  }))];
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function records(value) {
  return Array.isArray(value) ? value.map(record) : [];
}
