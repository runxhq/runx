const object = (v) =>
  v && typeof v === "object" && !Array.isArray(v) ? v : {};
const array = (v) => (Array.isArray(v) ? v : []);
const text = (v) => (typeof v === "string" ? v.trim() : "");
const strings = (v) => array(v).filter((x) => typeof x === "string");
const nativeDigest = (v) => /^sha256:[0-9a-f]{64}$/.test(text(v));
const key = (f) =>
  JSON.stringify([
    text(f.ecosystem),
    text(f.dependency),
    text(f.exact_version),
    text(f.advisory_id),
  ]);

export function admitScan({
  target,
  evidence,
  scan_context = {},
  evidence_digest,
}) {
  const source = object(evidence),
    diagnostics = [];
  const note = (code, message) => diagnostics.push({ code, message });
  if (!text(target))
    note(
      "target.missing",
      "Name the repository, deployment or package set being reviewed.",
    );
  if (!nativeDigest(evidence_digest))
    note(
      "evidence.unbound",
      "A native digest of the supplied evidence is required.",
    );
  if (!Array.isArray(source.findings))
    note(
      "findings.missing",
      "Supply actual exact-coordinate findings, not a verification boolean or a delivery digest.",
    );
  const known = new Map();
  for (const [i, raw] of array(source.findings).entries()) {
    const f = object(raw);
    if (!text(f.dependency) || !text(f.exact_version) || !text(f.advisory_id)) {
      note(
        "finding.identity",
        "Finding " + i + " lacks dependency, exact_version or advisory_id.",
      );
      continue;
    }
    const identity = key(f),
      before = known.get(identity);
    const locations = [
      ...new Set([
        ...array(before?.dependency_paths),
        ...array(f.dependency_paths),
        ...(text(f.dependency_path) ? [f.dependency_path] : []),
      ]),
    ];
    if (before) {
      if (
        locations.length ||
        Array.isArray(before.dependency_paths) ||
        Array.isArray(f.dependency_paths)
      )
        before.dependency_paths = locations;
      before.additional_observations = [
        ...array(before.additional_observations),
        f,
      ];
      if (before.modified && f.modified && before.modified !== f.modified)
        note(
          "finding.conflict",
          "Repeated advisory identity has different modification evidence: " +
            identity,
        );
      continue;
    }
    known.set(identity, {
      ...f,
      finding_key: identity,
      ...(Array.isArray(f.dependency_paths) || text(f.dependency_path)
        ? { dependency_paths: locations }
        : {}),
    });
  }
  const isScan = source.schema === "runx/cve-audit.audit.v1";
  const complete =
    isScan &&
    source.status === "complete" &&
    source.coverage?.complete === true &&
    source.coverage?.inventory_complete === true &&
    source.coverage?.omitted_components === 0 &&
    source.findings_scope === "all_observed" &&
    source.previous_result_part === null;
  const findings = [...known.values()];
  return {
    triage_context: {
      path: diagnostics.length ? "stop" : findings.length ? "assess" : "empty",
      target: text(target),
      findings,
      source: {
        evidence_digest,
        kind: isScan ? "provided_cve_scan" : "provided_findings",
        assurance: "source_bound_not_independently_verified",
        scan_status: isScan ? source.status : null,
        coverage: isScan ? source.coverage : object(source.coverage),
        analysis_complete: complete,
        inventory_digest: isScan ? source.inventory_digest : null,
        inventory_artifact: isScan ? (source.inventory_artifact ?? null) : null,
        components_artifact: isScan
          ? (source.components_artifact ?? null)
          : null,
        result_part: isScan ? source.result_part : null,
        previous_result_part: isScan ? source.previous_result_part : null,
        continuation: isScan ? source.continuation : null,
      },
      scan_context: object(scan_context),
      diagnostics,
    },
  };
}
function fixedVersions(f) {
  return new Set(
    array(f.affected)
      .filter(
        (a) =>
          a?.package?.name === f.dependency &&
          (!f.ecosystem || a?.package?.ecosystem === f.ecosystem),
      )
      .flatMap((a) =>
        array(a.ranges).flatMap((r) =>
          array(object(r).events).map((e) => text(object(e).fixed)),
        ),
      )
      .filter(Boolean),
  );
}
export function finalizeScan({ triage_context, risk_assessment_draft = {} }) {
  const c = triage_context,
    d = object(risk_assessment_draft),
    diagnostics = [...c.diagnostics];
  const note = (code, message) => diagnostics.push({ code, message });
  const known = new Map(c.findings.map((f) => [f.finding_key, f])),
    proposed = new Map();
  const contextEvidence = new Set(
    array(c.scan_context.evidence)
      .filter((e) => text(e.id) && text(e.source) && text(e.excerpt))
      .map((e) => e.id),
  );
  if (c.path === "assess")
    for (const [i, raw] of array(d.advisory_assessments).entries()) {
      const a = object(raw),
        identity = text(a.finding_key),
        f = known.get(identity);
      if (!f || proposed.has(identity)) {
        note(
          "assessment.identity",
          "Assessment " + i + " has an unknown or repeated finding_key.",
        );
        continue;
      }
      const priority = ["urgent", "high", "normal", "low", "unknown"].includes(
        a.priority,
      )
        ? a.priority
        : "unknown";
      if (priority !== a.priority)
        note(
          "assessment.priority",
          "Select a supported priority for " + identity + ".",
        );
      const confidence = ["high", "medium", "low"].includes(a.confidence)
        ? a.confidence
        : "low";
      if (confidence !== a.confidence)
        note("assessment.confidence", "Select high, medium or low confidence.");
      const refs = strings(a.exposure_evidence);
      let exposure = a.exposure;
      if (!["confirmed", "not_established"].includes(exposure)) {
        exposure = "not_established";
        note(
          "assessment.exposure",
          "Exposure must be confirmed or not_established.",
        );
      }
      if (
        exposure === "confirmed" &&
        (!refs.length || refs.some((r) => !contextEvidence.has(r)))
      ) {
        exposure = "not_established";
        note(
          "assessment.exposure_unbound",
          "Confirmed exposure needs actual scan_context.evidence references for " +
            identity +
            ".",
        );
      }
      if (refs.some((r) => !contextEvidence.has(r)))
        note("assessment.source", "An exposure source reference is missing.");
      if (!text(a.rationale) || !text(a.recommendation))
        note(
          "assessment.incomplete",
          "Each finding needs rationale and a concrete recommendation.",
        );
      const fixed = text(a.fixed_version);
      if (fixed && !fixedVersions(f).has(fixed))
        note(
          "assessment.fixed_version",
          "The proposed fixed version is not declared by this package advisory; retain it as an unverified investigation instead.",
        );
      proposed.set(identity, {
        priority,
        confidence,
        exposure,
        exposure_evidence: refs.filter((r) => contextEvidence.has(r)),
        evidence_status: "source_bound",
        rationale: text(a.rationale),
        recommendation: text(a.recommendation),
        fixed_version: fixed && fixedVersions(f).has(fixed) ? fixed : null,
      });
    }
  const advisories = c.findings.map((f) => {
    const assessment = proposed.get(f.finding_key);
    if (!assessment && c.path === "assess")
      note(
        "assessment.missing",
        "A finding was not assessed: " + f.finding_key,
      );
    return {
      ...f,
      assessment: assessment || {
        priority: "unknown",
        confidence: "low",
        exposure: "not_established",
        exposure_evidence: [],
        evidence_status: "source_bound",
        rationale: "No valid assessment was available.",
        recommendation: "Review the retained source evidence.",
        fixed_version: null,
      },
    };
  });
  let decision =
    c.path === "empty"
      ? c.source.analysis_complete
        ? "no_matches_in_scope"
        : "needs_more_evidence"
      : ["remediate", "monitor", "needs_human"].includes(d.decision)
        ? d.decision
        : "needs_human";
  if (
    c.path === "assess" &&
    !["remediate", "monitor", "needs_human"].includes(d.decision)
  )
    note("decision.invalid", "Choose remediate, monitor or needs_human.");
  if (diagnostics.length) decision = "needs_more_evidence";
  const summary =
    c.path === "assess"
      ? text(object(d.operator_summary).summary)
      : c.path === "empty"
        ? c.source.analysis_complete
          ? "No active advisory matches were supplied for the complete selected scan."
          : "No finding is present in this part; the overall review is incomplete."
        : "Usable exact-coordinate evidence is missing.";
  if (c.path === "assess" && !summary)
    note("summary.missing", "Provide a useful operator summary.");
  if (diagnostics.length) decision = "needs_more_evidence";
  return {
    vulnerability_review: {
      schema: "runx/vuln-triage.scan.v1",
      decision: diagnostics.length ? "needs_more_evidence" : decision,
      target: c.target,
      advisories,
      remediation_plan: advisories.map((f) => ({
        finding_key: f.finding_key,
        recommendation: f.assessment.recommendation,
        fixed_version: f.assessment.fixed_version,
      })),
      escalation_criteria: advisories.flatMap((f) => {
        const triggers = [];
        if (["urgent", "high"].includes(f.assessment.priority))
          triggers.push("priority_high_or_urgent");
        if (f.assessment.exposure === "not_established")
          triggers.push("exposure_not_established");
        if (f.assessment.confidence === "low")
          triggers.push("assessment_confidence_low");
        return triggers.map((trigger) => ({
          finding_key: f.finding_key,
          advisory_id: f.advisory_id,
          trigger,
          action: "human_remediation_review",
        }));
      }),
      operator_summary: { verdict: decision, summary },
      evidence_binding: c.source,
      exposure_evidence: array(c.scan_context.evidence),
      validation: {
        status: diagnostics.length ? "fail" : "pass",
        findings: diagnostics,
      },
      publication_status: "not_published",
    },
  };
}
export function admitAdvisory({ triage_packet, audience = "affected_users" }) {
  const p = object(triage_packet),
    diagnostics = [];
  if (
    p.schema !== "runx/vuln-triage.scan.v1" ||
    p.validation?.status !== "pass" ||
    !nativeDigest(p.evidence_binding?.evidence_digest)
  )
    diagnostics.push({
      code: "advisory.evidence",
      message: "Supply a validated source-bound vulnerability review.",
    });
  return {
    advisory_context: {
      path:
        diagnostics.length || !array(p.advisories).length ? "stop" : "draft",
      target: p.target,
      audience,
      advisories: array(p.advisories),
      remediation_plan: array(p.remediation_plan),
      evidence_binding: object(p.evidence_binding),
      diagnostics,
    },
  };
}
export function finalizeAdvisory({ advisory_context, advisory_draft = {} }) {
  const c = advisory_context,
    d = object(advisory_draft),
    diagnostics = [...c.diagnostics];
  const known = new Set(c.advisories.map((a) => a.advisory_id)),
    ids = strings(d.affected_advisory_ids);
  if (c.path === "draft") {
    if (!text(d.title) || !text(d.body) || !ids.length)
      diagnostics.push({
        code: "advisory.incomplete",
        message: "Supply title, body and affected advisory ids.",
      });
    if (ids.some((id) => !known.has(id)))
      diagnostics.push({
        code: "advisory.identity",
        message: "The draft names an advisory outside the reviewed evidence.",
      });
  }
  return {
    vulnerability_advisory_packet: {
      schema: "runx/vuln-triage.advisory.v1",
      decision: diagnostics.length
        ? "needs_more_evidence"
        : c.path === "stop"
          ? "no_advisory_needed"
          : "ready_for_review",
      target: c.target,
      audience: c.audience,
      advisory_draft:
        c.path === "draft"
          ? {
              title: text(d.title),
              summary: text(d.summary),
              body: text(d.body),
              affected_advisory_ids: ids,
            }
          : {},
      disclosure_checklist: strings(d.disclosure_checklist),
      source_advisories: c.advisories.filter((a) =>
        ids.includes(a.advisory_id),
      ),
      remediation_plan: { actions: c.remediation_plan },
      evidence_binding: c.evidence_binding,
      validation: {
        status: diagnostics.length ? "fail" : "pass",
        findings: diagnostics,
      },
      publication_status: "not_published",
      provider_status: "not_called",
    },
  };
}
