export function decideProcurement(inputs) {
  const request = record(inputs.procurement_request);
  const policy = record(inputs.procurement_policy);
  const findings = validateRequest(request, policy);
  if (findings.length > 0) {
    return refusal(request, policy, inputs, "missing_policy_authority", findings);
  }

  const payee = request.payee;
  const amount = request.amount;
  const policyIds = [];
  let refusedReason = null;

  if (!policy.approved_payees.includes(payee)) {
    refusedReason = "unapproved_payee";
  } else if (request.currency !== policy.currency) {
    refusedReason = "currency_mismatch";
  } else if (amount > policy.remaining_budget) {
    refusedReason = "over_remaining_budget";
  } else if (amount > policy.request_cap) {
    refusedReason = "over_request_cap";
  }
  policyIds.push("approved_payees", "currency", "remaining_budget", "request_cap");

  if (refusedReason) {
    return packet(inputs, {
      decision: "refused",
      reason: `Refused: ${refusedReason.replaceAll("_", " ")}.`,
      payee,
      amount,
      currency: request.currency,
      ceiling_amount: ceiling(policy),
      human_lane: null,
      refused_reason: refusedReason,
      policy_ids: policyIds,
      validation: { status: "pass", findings: [] },
    });
  }

  const threshold = policy.approval_threshold;
  if (typeof threshold === "number" && amount >= threshold) {
    return packet(inputs, {
      decision: "needs_human",
      reason: "Amount reaches the approval threshold; a human lane must approve before any spend.",
      payee,
      amount,
      currency: request.currency,
      ceiling_amount: ceiling(policy),
      human_lane: typeof policy.human_lane === "string" && policy.human_lane ? policy.human_lane : "procurement-review",
      refused_reason: null,
      policy_ids: [...policyIds, "approval_threshold"],
      validation: { status: "pass", findings: [] },
    });
  }

  return packet(inputs, {
    decision: "approved",
    reason: "Payee is approved, currency matches policy, and the amount is within remaining budget and the request cap.",
    payee,
    amount,
    currency: request.currency,
    ceiling_amount: ceiling(policy),
    human_lane: null,
    refused_reason: null,
    policy_ids: policyIds,
    validation: { status: "pass", findings: [] },
  });
}

function validateRequest(request, policy) {
  const findings = [];
  if (typeof request.payee !== "string" || !request.payee.trim()) {
    findings.push({ code: "request.invalid", message: "procurement_request.payee must be a non-empty string." });
  }
  if (!isPositiveNumber(request.amount)) {
    findings.push({ code: "request.invalid", message: "procurement_request.amount must be a positive number." });
  }
  if (typeof request.currency !== "string" || !request.currency.trim()) {
    findings.push({ code: "request.invalid", message: "procurement_request.currency must be a non-empty string." });
  }
  if (!Array.isArray(policy.approved_payees) || policy.approved_payees.length === 0) {
    findings.push({ code: "policy.missing", message: "procurement_policy.approved_payees must be a non-empty array." });
  }
  if (!isPositiveNumber(policy.remaining_budget)) {
    findings.push({ code: "policy.missing", message: "procurement_policy.remaining_budget must be a positive number." });
  }
  if (!isPositiveNumber(policy.request_cap)) {
    findings.push({ code: "policy.missing", message: "procurement_policy.request_cap must be a positive number." });
  }
  if (typeof policy.currency !== "string" || !policy.currency.trim()) {
    findings.push({ code: "policy.missing", message: "procurement_policy.currency must be a non-empty string." });
  }
  return findings;
}

function refusal(request, policy, inputs, refusedReason, findings) {
  return packet(inputs, {
    decision: "refused",
    reason: "Refused: the procurement policy does not carry the authority this decision needs.",
    payee: typeof request.payee === "string" ? request.payee : "",
    amount: isPositiveNumber(request.amount) ? request.amount : 0,
    currency: typeof request.currency === "string" ? request.currency : "",
    ceiling_amount: ceiling(policy),
    human_lane: null,
    refused_reason: refusedReason,
    policy_ids: [],
    validation: { status: "fail", findings },
  });
}

function packet(inputs, decision) {
  return {
    procurement_approval: {
      schema: "runx/procurement-policy-check.approval.v1",
      ...decision,
      spend_executed: false,
      request_digest: requiredDigest(inputs.request_digest),
      policy_digest: requiredDigest(inputs.policy_digest),
    },
  };
}

function ceiling(policy) {
  const budget = isPositiveNumber(policy.remaining_budget) ? policy.remaining_budget : 0;
  const cap = isPositiveNumber(policy.request_cap) ? policy.request_cap : 0;
  return Math.min(budget, cap);
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function isPositiveNumber(value) {
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}
