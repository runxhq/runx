export function admitScan(inputs) {
  const evidence = object(inputs.cve_evidence);
  const audit = unwrap(evidence.audit_result) || (evidence.schema === "runx/cve-audit.audit.v1" ? evidence : {});
  const verification = unwrap(evidence.verification);
  const findings = [];
  const finding = (code, message) => findings.push({ code, message });
  const target = text(inputs.target, 1_000);
  const receiptId = text(evidence.receipt_id, 200);
  const evidenceDigest = text(inputs.evidence_digest, 80);

  if (!target) finding("vuln.target.missing", "target is required");
  if (audit.schema !== "runx/cve-audit.audit.v1") finding("vuln.audit.missing", "cve_evidence must include an exact cve-audit packet");
  if (verification.verified !== true) finding("vuln.audit.unverified", "cve_evidence must include successful independent verification");
  if (!/^sha256:[0-9a-f]{64}$/u.test(receiptId)) finding("vuln.receipt.missing", "cve_evidence must include its sealed receipt id");
  if (!/^sha256:[0-9a-f]{64}$/u.test(evidenceDigest)) finding("vuln.evidence.unbound", "native CVE evidence digest is required");

  const verifiedFindings = array(audit.findings).slice(0, 500).map((item, index) => {
    const value = object(item);
    const verifiedFinding = {
      dependency: text(value.dependency, 300),
      exact_version: text(value.exact_version, 100),
      advisory_id: text(value.advisory_id, 200),
      advisory_url: text(value.advisory_url, 1_000),
      dependency_path: text(value.dependency_path, 1_000),
      direct: value.direct === true,
      development: value.development === true,
      summary: text(value.summary, 2_000),
      aliases: strings(value.aliases, 50, 300),
      severity: array(value.severity).slice(0, 20),
    };
    for (const field of ["dependency", "exact_version", "advisory_id", "advisory_url"]) {
      if (!verifiedFinding[field]) finding("vuln.finding.identity", `finding[${index}].${field} is required`);
    }
    if (verifiedFinding.advisory_url && !/^https:\/\/osv\.dev\/vulnerability\//u.test(verifiedFinding.advisory_url)) {
      finding("vuln.finding.source", `finding[${index}] advisory_url is not an OSV advisory URL`);
    }
    return verifiedFinding;
  });

  const path = findings.length > 0 ? "stop" : verifiedFindings.length === 0 ? "complete" : "assess";
  return {
    triage_context: {
      schema: "runx/vuln-triage.triage_context.v1",
      path,
      stop_decision: findings.some(({ code }) => code.endsWith(".missing")) ? "needs_agent" : "needs_verified_evidence",
      target,
      source: {
        receipt_id: receiptId,
        evidence_digest: evidenceDigest,
        target: object(audit.target),
        dependency_scope: text(audit.dependency_scope, 100),
      },
      dependency_inventory: array(audit.inventory).slice(0, 5_000),
      verified_findings: verifiedFindings,
      scan_context: object(inputs.scan_context),
      findings,
    },
  };
}

export function finalizeScan(inputs) {
  const context = object(inputs.triage_context);
  const draft = object(inputs.risk_assessment_draft);
  const findings = array(context.findings).slice();
  const finding = (code, message) => findings.push({ code, message });
  const known = new Map(array(context.verified_findings).map((item) => [findingKey(item), item]));
  const proposed = new Map();

  if (context.path === "assess") {
    for (const [index, candidate] of array(draft.advisory_assessments).entries()) {
      const item = object(candidate);
      const identity = findingKey(item);
      if (!known.has(identity)) {
        finding("vuln.assessment.unbound", `assessment[${index}] does not match verified CVE evidence`);
        continue;
      }
      if (!["urgent", "high", "normal", "low", "unknown"].includes(text(item.priority))) finding("vuln.assessment.priority", `assessment[${index}] has invalid priority`);
      if (!["confirmed", "not_established"].includes(text(item.exposure))) finding("vuln.assessment.exposure", `assessment[${index}] exposure must be confirmed or not_established`);
      if (!["high", "medium", "low"].includes(text(item.confidence))) finding("vuln.assessment.confidence", `assessment[${index}] confidence must be high, medium, or low`);
      if (!text(item.rationale)) finding("vuln.assessment.rationale", `assessment[${index}] rationale is required`);
      proposed.set(identity, {
        priority: text(item.priority),
        exposure: text(item.exposure),
        confidence: text(item.confidence),
        evidence_status: "verified_exact_cve",
        rationale: text(item.rationale, 2_000),
      });
    }
    for (const identity of known.keys()) if (!proposed.has(identity)) finding("vuln.assessment.missing", `verified finding was not assessed: ${identity}`);
  }

  const advisories = [...known.entries()].map(([identity, item]) => ({
    ...item,
    assessment: proposed.get(identity) || { priority: "unknown", exposure: "not_established", confidence: "low", evidence_status: "verified_exact_cve", rationale: "No bounded assessment was available." },
  }));
  const escalationCriteria = advisories.flatMap(({ advisory_id, assessment }) => {
    const criteria = [];
    if (["urgent", "high"].includes(assessment.priority)) criteria.push({ advisory_id, trigger: "priority_high_or_urgent", action: "human_remediation_review" });
    if (assessment.exposure === "not_established") criteria.push({ advisory_id, trigger: "exposure_not_established", action: "verify_exposure_before_downgrade" });
    if (assessment.confidence === "low") criteria.push({ advisory_id, trigger: "assessment_confidence_low", action: "human_risk_review" });
    return criteria;
  });
  const requestedDecision = text(draft.decision);
  const decision = context.path === "stop"
    ? text(context.stop_decision)
    : context.path === "complete"
      ? "no_verified_findings"
      : findings.length > 0
        ? "needs_more_evidence"
        : ["remediate", "monitor", "needs_human"].includes(requestedDecision)
          ? requestedDecision
          : "needs_human";
  if (context.path === "assess" && !["remediate", "monitor", "needs_human"].includes(requestedDecision)) finding("vuln.decision.invalid", "decision must be remediate, monitor, or needs_human");

  return {
    vulnerability_scan_packet: {
      schema: "runx/vuln-triage.scan.v1",
      decision: findings.length > 0 && context.path === "assess" ? "needs_more_evidence" : decision,
      target: context.target,
      dependency_inventory: context.dependency_inventory,
      advisories,
      escalation_criteria: escalationCriteria,
      remediation_plan: context.path === "assess" ? object(draft.remediation_plan) : {},
      operator_summary: context.path === "assess" ? object(draft.operator_summary) : {
        verdict: decision,
        summary: context.path === "complete" ? "The verified exact-version audit reported no current findings." : "Verified exact-version CVE evidence is required.",
      },
      evidence_binding: context.source,
      validation: { status: findings.length === 0 ? "pass" : "fail", findings },
      publication_status: "not_published",
    },
  };
}

export function admitAdvisory(inputs) {
  const packet = object(inputs.triage_packet);
  const findings = [];
  const finding = (code, message) => findings.push({ code, message });
  if (packet.schema !== "runx/vuln-triage.scan.v1") finding("vuln.advisory.packet", "triage_packet must be runx/vuln-triage.scan.v1");
  if (object(packet.validation).status !== "pass") finding("vuln.advisory.validation", "triage packet validation must pass");
  const advisories = array(packet.advisories).slice(0, 500);
  const evidenceBinding = object(packet.evidence_binding);
  if (!/^sha256:[0-9a-f]{64}$/u.test(text(evidenceBinding.receipt_id))) finding("vuln.advisory.receipt", "triage packet requires a sealed receipt id");
  if (!/^sha256:[0-9a-f]{64}$/u.test(text(evidenceBinding.evidence_digest))) finding("vuln.advisory.digest", "triage packet requires an evidence digest");
  for (const [index, advisory] of advisories.entries()) {
    const item = object(advisory);
    const assessment = object(item.assessment);
    if (!text(item.advisory_id)) finding("vuln.advisory.identity", `advisories[${index}].advisory_id is required`);
    if (!["high", "medium", "low"].includes(text(assessment.confidence))) finding("vuln.advisory.confidence", `advisories[${index}] requires bounded assessment confidence`);
    if (text(assessment.evidence_status) !== "verified_exact_cve") finding("vuln.advisory.evidence", `advisories[${index}] is not bound to verified exact-version CVE evidence`);
  }
  const path = findings.length > 0 || advisories.length === 0 ? "stop" : "draft";
  return {
    advisory_context: {
      schema: "runx/vuln-triage.advisory_context.v1",
      path,
      stop_decision: findings.length > 0 ? "needs_verified_evidence" : "no_advisory_needed",
      target: packet.target,
      decision: packet.decision,
      advisories,
      remediation_plan: object(packet.remediation_plan),
      evidence_binding: evidenceBinding,
      audience: text(inputs.audience) || "affected_users",
      findings,
    },
  };
}

export function finalizeAdvisory(inputs) {
  const context = object(inputs.advisory_context);
  const draft = object(inputs.advisory_draft);
  const findings = array(context.findings).slice();
  const finding = (code, message) => findings.push({ code, message });
  const allowedIds = new Set(array(context.advisories).map((item) => text(object(item).advisory_id)).filter(Boolean));
  const affectedIds = strings(draft.affected_advisory_ids, 500);
  if (context.path === "draft") {
    if (!text(draft.title)) finding("vuln.advisory.title", "title is required");
    if (!text(draft.body)) finding("vuln.advisory.body", "body is required");
    if (affectedIds.length === 0) finding("vuln.advisory.ids", "at least one affected advisory id is required");
    for (const id of affectedIds) if (!allowedIds.has(id)) finding("vuln.advisory.unbound", `advisory id is not in the verified triage packet: ${id}`);
  }
  const decision = context.path === "stop" ? context.stop_decision : findings.length === 0 ? "ready_for_review" : "needs_more_evidence";
  return {
    vulnerability_advisory_packet: {
      schema: "runx/vuln-triage.advisory.v1",
      decision,
      target: context.target,
      audience: context.audience,
      advisory_draft: context.path === "draft" ? {
        title: text(draft.title, 500),
        summary: text(draft.summary, 2_000),
        body: text(draft.body, 20_000),
        affected_advisory_ids: affectedIds,
      } : {},
      disclosure_checklist: strings(draft.disclosure_checklist, 100),
      source_advisories: array(context.advisories).filter((item) => affectedIds.includes(text(object(item).advisory_id))),
      remediation_plan: context.remediation_plan,
      evidence_binding: context.evidence_binding,
      validation: { status: findings.length === 0 ? "pass" : "fail", findings },
      publication_status: "not_published",
      provider_status: "not_called",
    },
  };
}

function findingKey(value) {
  const item = object(value);
  return [text(item.dependency), text(item.exact_version), text(item.advisory_id)].join("@");
}

function unwrap(value) {
  const item = object(value);
  return object(item.data || item);
}

function object(value) { return value && typeof value === "object" && !Array.isArray(value) ? value : {}; }
function array(value) { return Array.isArray(value) ? value : []; }
function strings(value, max, maxText = 1_000) { return array(value).map((item) => text(item, maxText)).filter(Boolean).slice(0, max); }
function text(value, max = 1_000) { return typeof value === "string" ? value.trim().slice(0, max) : ""; }
