const text = (value) => (typeof value === "string" ? value.trim() : "");
const rows = (value) => (Array.isArray(value) ? value : []);

export function prepareDiagnosis(input) {
  const id = text(input.receipt_id);
  const matched = rows(input.receipt_evidence?.receipt_details).some(
    (d) => !id || d.id === id,
  );
  return {
    preparation: {
      ready:
        matched ||
        Boolean(text(input.test_output)) ||
        Boolean(text(input.receipt_summary)),
    },
  };
}

export default function finalizeDiagnosis(input) {
  const candidate = input.candidate;
  if (
    !candidate ||
    !["pass", "needs_update", "blocked"].includes(candidate.verdict) ||
    !text(candidate.failure_summary)
  )
    throw new Error(
      "A diagnosis needs an explicit verdict and useful failure summary.",
    );
  const proposals = rows(candidate.improvement_proposals);
  if (
    proposals.length > 3 ||
    proposals.some(
      (p) =>
        !p ||
        ["target", "change", "rationale", "risk"].some((k) => !text(p[k])),
    )
  ) {
    throw new Error(
      "Return at most three concrete proposals, each with target, change, rationale and risk.",
    );
  }
  if (
    !Array.isArray(candidate.next_test_checks) ||
    candidate.next_test_checks.some((c) => !text(c))
  )
    throw new Error("Next checks must be concrete strings.");
  if (candidate.verdict === "pass" && proposals.length)
    throw new Error(
      "A passing diagnosis cannot also propose a required repair.",
    );
  if (candidate.verdict === "needs_update" && !proposals.length)
    throw new Error("A required update needs an actionable correction.");

  const id = text(input.receipt_id);
  const details = rows(input.receipt_evidence?.receipt_details).filter(
    (d) => !id || d.id === id,
  );
  const evidence = details.map((d) => ({
    source: "prior_receipt_metadata",
    reference: text(d.id),
    observation: `outcome=${text(d.outcome)}; seal_reason=${text(d.seal_reason)}; reported_verification=${text(d.verification?.status)}`,
    limitation:
      "Ledger projection and original verification posture; this phase does not independently reverify it or expose omitted execution bodies.",
  }));
  if (text(input.test_output))
    evidence.push({
      source: "supplied_test",
      reference: "input.test_output",
      limitation:
        "Caller-supplied bounded failure evidence; not an independently read diagnostic.",
    });
  if (text(input.receipt_summary))
    evidence.push({
      source: "supplied_summary",
      reference: "input.receipt_summary",
      limitation:
        "Supplemental caller context; not a replacement for native receipt facts.",
    });
  for (const ref of rows(candidate.evidence)) {
    if (text(ref))
      evidence.push({
        source: "reviewer_reference",
        reference: text(ref),
        limitation:
          "Review judgment; inspect the native turn calls for the actual read and its result.",
      });
  }
  if (
    !details.length &&
    !text(input.test_output) &&
    !text(input.receipt_summary)
  ) {
    return {
      verdict: "blocked",
      failure_summary:
        "No matched receipt detail or bounded failure evidence is available. Supply the exact receipt or the relevant sanitized failure material before diagnosing a cause.",
      improvement_proposals: [],
      next_test_checks: [
        "Resolve the exact failed receipt and relevant child, or supply the bounded harness assertion/diagnostic.",
      ],
      evidence,
    };
  }
  return {
    verdict: candidate.verdict,
    failure_summary: candidate.failure_summary,
    improvement_proposals: proposals,
    next_test_checks: candidate.next_test_checks,
    evidence,
  };
}
