export function decidePurchase(inputs) {
  const request = record(inputs.purchase_request);
  const policy = record(inputs.procurement_policy);
  const findings = validateRequest(request, policy);
  if (findings.length > 0) {
    return refusal(request, policy, inputs, "missing_policy_authority", findings);
  }

  const vendor = request.vendor;
  const amount = request.amount;
  const policyRefs = [];
  let refusedReason = null;

  if (!policy.approved_vendors.includes(vendor)) {
    refusedReason = "unapproved_vendor";
  } else if (request.currency !== policy.currency) {
    refusedReason = "currency_mismatch";
  } else if (amount > policy.remaining_budget) {
    refusedReason = "over_remaining_budget";
  } else if (amount > policy.single_purchase_cap) {
    refusedReason = "over_single_purchase_cap";
  }
  policyRefs.push("approved_vendors", "currency", "remaining_budget", "single_purchase_cap");

  if (refusedReason) {
    return packet(inputs, {
      decision: "refused",
      reason: `Refused: ${refusedReason.replaceAll("_", " ")}.`,
      vendor,
      amount,
      currency: request.currency,
      ceiling_amount: ceiling(policy),
      human_lane: null,
      refused_reason: refusedReason,
      policy_refs: policyRefs,
      validation: { status: "pass", findings: [] },
    });
  }

  const threshold = policy.approval_threshold;
  if (typeof threshold === "number" && amount >= threshold) {
    return packet(inputs, {
      decision: "needs_human",
      reason: "Amount reaches the approval threshold; a human lane must approve before any spend.",
      vendor,
      amount,
      currency: request.currency,
      ceiling_amount: ceiling(policy),
      human_lane: typeof policy.human_lane === "string" && policy.human_lane ? policy.human_lane : "procurement-review",
      refused_reason: null,
      policy_refs: [...policyRefs, "approval_threshold"],
      validation: { status: "pass", findings: [] },
    });
  }

  return packet(inputs, {
    decision: "approved",
    reason: "Vendor is approved, currency matches policy, and the amount is within remaining budget and the single-purchase cap.",
    vendor,
    amount,
    currency: request.currency,
    ceiling_amount: ceiling(policy),
    human_lane: null,
    refused_reason: null,
    policy_refs: policyRefs,
    validation: { status: "pass", findings: [] },
  });
}

function validateRequest(request, policy) {
  const findings = [];
  if (typeof request.vendor !== "string" || !request.vendor.trim()) {
    findings.push({ code: "request.invalid", message: "purchase_request.vendor must be a non-empty string." });
  }
  if (!isPositiveNumber(request.amount)) {
    findings.push({ code: "request.invalid", message: "purchase_request.amount must be a positive number." });
  }
  if (typeof request.currency !== "string" || !request.currency.trim()) {
    findings.push({ code: "request.invalid", message: "purchase_request.currency must be a non-empty string." });
  }
  if (!Array.isArray(policy.approved_vendors) || policy.approved_vendors.length === 0) {
    findings.push({ code: "policy.missing", message: "procurement_policy.approved_vendors must be a non-empty array." });
  }
  if (!isPositiveNumber(policy.remaining_budget)) {
    findings.push({ code: "policy.missing", message: "procurement_policy.remaining_budget must be a positive number." });
  }
  if (!isPositiveNumber(policy.single_purchase_cap)) {
    findings.push({ code: "policy.missing", message: "procurement_policy.single_purchase_cap must be a positive number." });
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
    vendor: typeof request.vendor === "string" ? request.vendor : "",
    amount: isPositiveNumber(request.amount) ? request.amount : 0,
    currency: typeof request.currency === "string" ? request.currency : "",
    ceiling_amount: ceiling(policy),
    human_lane: null,
    refused_reason: refusedReason,
    policy_refs: [],
    validation: { status: "fail", findings },
  });
}

function packet(inputs, decision) {
  return {
    purchase_approval: {
      schema: "runx.purchase_approval.v1",
      ...decision,
      spend_executed: false,
      request_digest: requiredDigest(inputs.request_digest),
      policy_digest: requiredDigest(inputs.policy_digest),
    },
  };
}

function ceiling(policy) {
  const budget = isPositiveNumber(policy.remaining_budget) ? policy.remaining_budget : 0;
  const cap = isPositiveNumber(policy.single_purchase_cap) ? policy.single_purchase_cap : 0;
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
