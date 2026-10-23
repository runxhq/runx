// ICP judgment belongs to the agent; these are domain admission and decision rules.
export function admitQualification({ enrichment_packet, criteria, contact_eligibility }) {
  const blockers = [];
  if (!["enriched", "partial"].includes(enrichment_packet.decision) || !enrichment_packet.facts.length) {
    blockers.push("no admitted factual evidence is available");
  }
  const sources = new Set(enrichment_packet.sources.map((source) => source.source_id));
  const fields = enrichment_packet.facts.map((fact) => fact.field);
  const conflicts = new Set(enrichment_packet.conflicts.map((conflict) => conflict.field));
  if (new Set(fields).size !== fields.length
      || enrichment_packet.facts.some((fact) => conflicts.has(fact.field)
        || fact.source_ids.some((ref) => !sources.has(ref)))) {
    blockers.push("factual context contains duplicate, conflicting or unbound fields");
  }
  if (new Set(criteria.map((criterion) => criterion.id)).size !== criteria.length) {
    blockers.push("criterion IDs must be unique");
  }
  const bound = contact_eligibility.subject.id === enrichment_packet.subject.id
    && contact_eligibility.subject.kind === enrichment_packet.subject.kind;
  if (!bound) blockers.push("contact eligibility belongs to a different or unbound subject");
  return {
    qualification_context: {
      path: blockers.length ? "hold" : "assess",
      subject: enrichment_packet.subject,
      facts: enrichment_packet.facts,
      criteria,
      ...(bound && { contact_eligibility }),
      blockers,
    },
  };
}

export function finalizeQualification({ qualification_context: context, enrichment_digest, assessment_draft }) {
  const blockers = [...context.blockers];
  const assessments = assessment_draft?.assessments ?? [];
  const fields = new Set(context.facts.map((fact) => fact.field));
  const seen = new Set();
  for (const assessment of assessments) {
    if (seen.has(assessment.criterion_id) || !context.criteria.some((criterion) => criterion.id === assessment.criterion_id)) {
      blockers.push("assessment contains an unknown or duplicate criterion");
    }
    seen.add(assessment.criterion_id);
    if (assessment.fact_fields.some((field) => !fields.has(field))) {
      blockers.push(`${assessment.criterion_id}: assessment cites an unavailable fact`);
    }
    if (assessment.status !== "unknown" && !assessment.fact_fields.length) {
      blockers.push(`${assessment.criterion_id}: a fit decision requires factual evidence`);
    }
  }
  if (context.criteria.some((criterion) => !seen.has(criterion.id))) {
    blockers.push("not every supplied criterion was assessed");
  }

  const mandatory = context.criteria.filter((criterion) => criterion.required);
  const assessmentById = new Map(assessments.map((assessment) => [assessment.criterion_id, assessment]));
  const fit = blockers.length ? "unknown"
    : mandatory.some((criterion) => assessmentById.get(criterion.id).status === "not_met") ? "not_fit"
    : mandatory.some((criterion) => assessmentById.get(criterion.id).status === "unknown") ? "unknown" : "fit";
  // Admission keeps eligibility only when it is bound to this subject.
  const supplied = context.contact_eligibility;
  const eligibility = supplied && {
    status: supplied.status,
    policy_id: supplied.policy_id,
    evidence_ids: supplied.evidence_ids,
  };
  const decision = fit === "not_fit" ? "disqualified"
    : fit === "unknown" ? "needs_more_evidence"
    : eligibility?.status === "eligible" ? "qualified" : "hold";

  return {
    lead_qualification: {
      decision,
      fit,
      subject: context.subject,
      enrichment_digest,
      criteria: context.criteria,
      assessments: blockers.length ? [] : assessments,
      ...(eligibility && { contact_eligibility: eligibility }),
      next_action: decision === "qualified" ? "prepare_outreach"
        : decision === "disqualified" ? "stop"
        : decision === "hold" ? "resolve_contact_policy" : "request_evidence",
      blockers,
      external_status: "not_executed",
    },
  };
}
