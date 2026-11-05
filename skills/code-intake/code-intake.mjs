// Native runner admission owns structure. This module owns routing relationships
// and preserves caller evidence independently of model-authored recommendations.
export default function finalize(inputs) {
  const draft = inputs.intake_draft,
    source = inputs.source_context ?? {};
  const digest = inputs.input_digest;
  if (typeof digest !== "string" || !/^sha256:[0-9a-f]{64}$/.test(digest))
    throw new Error("Native input identity is required.");
  const { category, severity, summary, suggested_reply, rationale } = draft;
  const questions = [...draft.open_questions],
    notes = draft.operator_notes;
  let lane = draft.recommended_lane;
  const target_surfaces = draft.target_surfaces.map(
    ({ surface, kind, mutating, rationale }) => ({
      surface,
      kind,
      mutating,
      rationale,
    }),
  );
  const invariants = draft.shared_invariants,
    criteria = draft.success_criteria;
  if (
    lane === "github-issue-to-pr" &&
    !(typeof source.issue_locator === "string" && source.issue_locator.trim())
  )
    questions.push(
      "Supply the actual GitHub issue locator before selecting github-issue-to-pr.",
    );
  if (questions.length) lane = "manual-review";
  if (
    ["host", "code-plan", "github-issue-to-pr"].includes(lane) &&
    (!target_surfaces.length || !criteria.length)
  )
    throw new Error(
      "An implementation or planning recommendation needs actual target surfaces and success criteria.",
    );
  const intake_report = {
    category,
    severity,
    summary,
    suggested_reply,
    recommended_lane: lane,
    rationale,
    needs_human: lane === "manual-review",
    operator_notes: notes,
    open_questions: questions,
  };
  const change_set = {
    change_set_id: digest,
    source_digest: digest,
    summary,
    category,
    severity,
    recommended_lane: lane,
    target_surfaces,
    shared_invariants: invariants,
    success_criteria: criteria,
  };
  if (typeof source.thread_locator === "string" && source.thread_locator.trim())
    change_set.thread_locator = source.thread_locator;
  return {
    request_packet: {
      schema: "runx/code-intake.intake.v1",
      input_digest: digest,
      request: inputs.request,
      project_context: inputs.project_context || "",
      operator_context: inputs.operator_context || "",
      source_context: source,
      intake_report,
      change_set,
      status:
        lane === "manual-review"
          ? "needs_context"
          : lane === "reply-only"
            ? "reply_only"
            : "ready",
      execution: { status: "not_started", authority: "none_granted" },
    },
  };
}
