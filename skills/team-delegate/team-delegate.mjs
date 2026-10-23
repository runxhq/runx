export default function finalize(inputs) {
  const { roster, evidence, draft } = inputs;
  const members = new Map(roster.map((member) => [member.member, member]));
  const sources = new Map(evidence.map((source) => [source.source_id, source]));
  if (members.size !== roster.length || sources.size !== evidence.length) throw new Error("Member and source references must be unique.");
  const findings = [];
  const assignments = [];
  for (const assignment of draft.assignments) {
    const member = members.get(assignment.member);
    if (!member) { findings.push(`Unknown team member: ${assignment.member}`); continue; }
    if (assignment.needed_scope.some((scope) => !member.scope_ceiling.includes(scope))) findings.push(`Scope exceeds member ceiling: ${assignment.member}`);
    if (assignment.source_ids.some((ref) => !sources.has(ref))) findings.push(`Unknown source for member: ${assignment.member}`);
    if (!assignment.acceptance.length) findings.push(`Missing acceptance checks: ${assignment.member}`);
    assignments.push({ ...assignment, skill: member.skill, runner: member.runner, evidence: assignment.source_ids.map((ref) => sources.get(ref)).filter(Boolean), authority_status: "not_granted" });
  }
  if (draft.decision === "propose" && (!assignments.length || draft.missing_context.length)) findings.push("Resolve missing inputs before proposing delegation.");
  return { delegation_packet: {
    schema: "runx/team-delegate.delegation.v1",
    decision: findings.length ? "held" : draft.decision === "needs_input" ? "needs_input" : "proposed",
    objective: inputs.objective,
    rationale: draft.rationale,
    assignments: findings.length ? [] : assignments,
    missing_context: draft.missing_context,
    input_digest: inputs.input_digest,
    execution_status: "not_started",
    validation: { status: findings.length ? "fail" : "pass", findings },
  }};
}
