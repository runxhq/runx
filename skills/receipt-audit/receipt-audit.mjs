// Domain packet invariants only. Native code owns proof and authority semantics.
export default function finalizeAudit(inputs) {
  const evidence = object(inputs.supplied_evidence ?? inputs.queried_evidence);
  const candidate = object(inputs.candidate);
  const id = typeof inputs.receipt_id === 'string' ? inputs.receipt_id : '';
  const details = array(evidence.receipt_details).filter(row => row.id === id);
  const rows = array(evidence.matched_receipts).filter(row => row.receipt_id === id);
  const detail = object(details[0]);
  const chain = object(evidence.chain_verification);
  const baseline = Array.isArray(inputs.granted_scopes) && inputs.granted_scopes.every(scope => typeof scope === 'string' && scope.length);
  const granted = baseline ? [...new Set(inputs.granted_scopes)].sort() : [];
  const recorded = [...new Set(array(object(detail.authority).exercised_scopes).map(row => row.scope).filter(scope => typeof scope === 'string' && scope.length))].sort();
  const gaps = [];
  if (!id || rows.length !== 1 || details.length !== 1) gaps.push('Exact receipt identity/detail is missing or ambiguous.');
  if (object(detail.verification).status !== 'verified' || chain.checked !== true || chain.intact !== true || !Array.isArray(chain.breaks) || chain.breaks.length) gaps.push('The source evidence does not establish an intact verified receipt tree.');
  if (!baseline) gaps.push('The operator grant baseline is missing; recorded authority is not its own permission ceiling.');
  if (candidate.receipt_id !== id) gaps.push('The assessment names a different receipt.');
  const findings = array(candidate.findings).filter(row => row.id !== 'receipt.clean');
  const errors = findings.some(row => row.severity === 'error');
  // Do not reinterpret opaque scope strings as wildcard or verb relationships.
  if (candidate.verdict === 'clean' && recorded.some(scope => !granted.includes(scope))) gaps.push('Recorded scopes lack exact baseline coverage; native authority semantics are needed before concluding clean.');
  const conclusive = gaps.length === 0 && candidate.decision === 'ready' && ['clean', 'anomaly'].includes(candidate.verdict);
  const verdict = !conclusive ? 'needs_more_evidence' : errors ? 'anomaly' : candidate.verdict;
  if (gaps.length) findings.push({id:'receipt.evidence.incomplete',severity:'warning',message:gaps.join(' ')});
  if (verdict === 'clean') findings.push({id:'receipt.clean',severity:'info',message:'No anomaly was found in the inspected recorded evidence against the supplied baseline.'});
  const source = inputs.supplied_evidence != null ? 'Supplied ledger evidence was reused; this run did not independently reread its receipts.' : 'Native ledger evidence was resolved for this review.';
  return {receipt_audit:{
    decision: conclusive ? 'ready' : 'needs_more_evidence', receipt_id:id, granted_scopes:granted, exercised_scopes:recorded,
    refusals:array(candidate.refusals), findings, verdict,
    rationale:[candidate.rationale || 'No supported assessment was provided.',source,'Coverage is limited to inspected recorded evidence. Authority references do not prove complete or successful usage; hidden inputs and diagnostics were not checked for secrets.'].join(' '),
    recommendations:[...array(candidate.recommendations),...gaps],
    success_checkpoint:{milestone:conclusive ? 'recorded_evidence_reviewed' : 'awaiting_evidence',description:conclusive ? 'Completed a scoped assessment of the inspected receipt evidence.' : 'Preserved the receipt identity and missing evidence without a clean verdict.'}
  }};
}
function object(value) { return value && typeof value === 'object' && !Array.isArray(value) ? value : {}; }
function array(value) { return Array.isArray(value) ? value : []; }
