const textFields = new Set([
  "access_activity_log",
  "billing_address",
  "cancellation_policy_disclosure",
  "cancellation_rebuttal",
  "customer_email_address",
  "customer_name",
  "customer_purchase_ip",
  "duplicate_charge_explanation",
  "duplicate_charge_id",
  "product_description",
  "refund_policy_disclosure",
  "refund_refusal_explanation",
  "service_date",
  "shipping_address",
  "shipping_carrier",
  "shipping_date",
  "shipping_tracking_number",
  "uncategorized_text",
]);
const fileFields = new Set([
  "cancellation_policy",
  "customer_communication",
  "customer_signature",
  "duplicate_charge_documentation",
  "receipt",
  "refund_policy",
  "service_documentation",
  "shipping_documentation",
  "uncategorized_file",
]);
const record = (value) =>
  value && typeof value === "object" && !Array.isArray(value) ? value : {};
const list = (value) => (Array.isArray(value) ? value : []);
const nonempty = (value) =>
  typeof value === "string" && value.trim().length > 0;
const safe = (value) => Number.isSafeInteger(value) && value >= 0;
const finding = (code, message) => ({ code, message });

function operation(packet, name, target, findings) {
  const value = record(record(packet).data ?? packet);
  if (
    value.schema !== "runx.provider.operation.v1" ||
    value.status !== "success" ||
    value.provider !== "stripe" ||
    value.operation !== name ||
    value.target !== target ||
    value.access !== "read"
  ) {
    findings.push(
      finding(
        "source.binding",
        `Expected successful stripe/${name} evidence for ${target}.`,
      ),
    );
  }
  return value;
}

export function chargeTarget(inputs) {
  const merchant = record(inputs.merchant);
  const findings = [];
  const packet = operation(
    inputs.case_evidence,
    "dispute.read",
    `stripe://${merchant.account_id}/disputes/${merchant.dispute_id}`,
    findings,
  );
  const dispute = record(packet.result);
  if (
    dispute.dispute_id !== merchant.dispute_id ||
    !/^ch_[A-Za-z0-9]{1,128}$/u.test(dispute.charge_id ?? "")
  )
    findings.push(
      finding(
        "case.identity",
        "The case does not identify its original Stripe charge.",
      ),
    );
  if (findings.length)
    throw new Error(findings.map((item) => item.message).join(" "));
  return {
    charge_target: {
      account_id: merchant.account_id,
      charge_id: dispute.charge_id,
    },
  };
}

export function prepareContext(inputs) {
  const account = inputs.account_id;
  const disputeId = inputs.dispute_id;
  const findings = [];
  const caseEvidence = operation(
    inputs.case_evidence,
    "dispute.read",
    `stripe://${account}/disputes/${disputeId}`,
    findings,
  );
  const dispute = record(caseEvidence.result);
  const chargeEvidence = operation(
    inputs.charge_evidence,
    "payment.stripe.charge.read",
    `stripe://${account}/charges/${dispute.charge_id}`,
    findings,
  );
  const charge = record(chargeEvidence.result);
  if (
    dispute.dispute_id !== disputeId ||
    charge.charge_id !== dispute.charge_id ||
    !safe(dispute.amount_minor) ||
    dispute.amount_minor < 1 ||
    !safe(charge.amount_minor) ||
    charge.amount_minor < dispute.amount_minor ||
    charge.currency !== dispute.currency ||
    charge.status !== "succeeded" ||
    !safe(charge.amount_refunded) ||
    charge.amount_refunded > charge.amount_minor
  ) {
    findings.push(
      finding(
        "payment.binding",
        "Dispute and original successful charge must agree on merchant, charge, currency and covered amount, preserving cumulative refunds.",
      ),
    );
  }
  if (
    !safe(dispute.submission_count) ||
    dispute.submission_count >= Number.MAX_SAFE_INTEGER ||
    !nonempty(dispute.reason) ||
    !dispute.evidence ||
    typeof dispute.evidence !== "object" ||
    Array.isArray(dispute.evidence) ||
    typeof dispute.past_due !== "boolean" ||
    typeof dispute.has_evidence !== "boolean"
  )
    findings.push(
      finding(
        "case.incomplete",
        "Missing case reason, evidence state, deadline state or submission count.",
      ),
    );
  const eligible = ["needs_response", "warning_needs_response"].includes(
    dispute.status,
  );
  const observed = Date.parse(inputs.observed_at);
  if (!Number.isFinite(observed))
    throw new Error("A native observation timestamp is required.");
  if (
    !eligible ||
    dispute.past_due !== false ||
    !safe(dispute.due_by) ||
    dispute.due_by * 1000 <= observed
  )
    findings.push(
      finding(
        "case.not_actionable",
        "This case is not currently within an open evidence-submission window. Do not stage, concede, refund or resubmit.",
      ),
    );
  const evidence = list(inputs.evidence);
  if (inputs.evidence !== undefined && !Array.isArray(inputs.evidence))
    findings.push(
      finding(
        "evidence.shape",
        "Evidence must be selected source records, not a raw provider response.",
      ),
    );
  const ids = new Set();
  for (const item of evidence) {
    if (
      !nonempty(item?.id) ||
      ids.has(item.id) ||
      ["provider-dispute", "provider-charge"].includes(item?.id) ||
      !nonempty(item?.content) ||
      !nonempty(item?.source) ||
      ![
        "delivery",
        "consent",
        "support",
        "contract",
        "refund",
        "other",
      ].includes(item?.kind)
    )
      findings.push(
        finding(
          "evidence.source",
          "Each selected evidence record needs a unique ID, kind, actual content and source; no invented source verification.",
        ),
      );
    ids.add(item?.id);
  }
  return {
    case_context: {
      account_id: account,
      dispute_id: disputeId,
      provider: "stripe",
      dispute,
      charge,
      evidence: [
        ...evidence,
        {
          id: "provider-dispute",
          kind: "provider",
          content: JSON.stringify(dispute),
          source: caseEvidence.readback_uri ?? caseEvidence.target,
        },
        {
          id: "provider-charge",
          kind: "provider",
          content: JSON.stringify(charge),
          source: chargeEvidence.readback_uri ?? chargeEvidence.target,
        },
      ],
      observed_at: inputs.observed_at,
      acquisition: inputs.acquisition,
      case_evidence: caseEvidence,
      charge_evidence: chargeEvidence,
      findings,
      actionable: findings.length === 0,
    },
  };
}

export function prepareResponse(inputs) {
  const context = record(inputs.case_context);
  const draft = record(inputs.response_draft);
  const findings = list(context.findings).slice();
  const dispute = record(context.dispute);
  const charge = record(context.charge);
  const evidence = list(context.evidence);
  const selected = { ...record(draft.fields) };
  const citations = list(draft.citations).slice();
  const staged = record(dispute.evidence);
  for (const [field, value] of Object.entries(staged)) {
    if (value === null || value === "") continue;
    if (
      (!textFields.has(field) && !fileFields.has(field)) ||
      typeof value !== "string"
    ) {
      findings.push(
        finding(
          "staged.unsupported",
          `Existing staged evidence ${field} needs explicit provider review before submission; do not submit hidden material.`,
        ),
      );
    } else if (!Object.hasOwn(selected, field)) {
      selected[field] = value;
      citations.push({
        field,
        evidence_id: "provider-dispute",
        quote: JSON.stringify(value),
      });
    }
  }
  const posture = draft.posture;
  const questions = list(draft.open_questions);
  if (
    context.actionable &&
    (![
      "contest",
      "refund_already_sent",
      "accept",
      "operator_review",
      "needs_more_evidence",
    ].includes(posture) ||
      !nonempty(draft.summary) ||
      questions.some((item) => !nonempty(item)))
  )
    findings.push(
      finding(
        "draft.shape",
        "Return a concrete posture, grounded summary and specific unresolved questions.",
      ),
    );
  const sourceIds = new Set(evidence.map((item) => item.id));
  const form = { submit: true };
  let characters = 0;
  for (const [field, value] of Object.entries(selected)) {
    const removal = value === "" && nonempty(staged[field]);
    if (
      (!textFields.has(field) && !fileFields.has(field)) ||
      (!nonempty(value) && !removal) ||
      (fileFields.has(field) &&
        !removal &&
        !/^file_[A-Za-z0-9]{1,128}$/u.test(value))
    ) {
      findings.push(
        finding(
          "draft.field",
          `Unsupported, empty or invalid provider evidence field: ${field}.`,
        ),
      );
      continue;
    }
    characters += [...value].length;
    const bound = citations.filter((citation) => citation?.field === field);
    if (
      bound.length === 0 ||
      bound.some(
        (citation) =>
          !sourceIds.has(citation.evidence_id) ||
          !nonempty(citation.quote) ||
          !evidence
            .find((item) => item.id === citation.evidence_id)
            ?.content.includes(citation.quote),
      )
    )
      findings.push(
        finding(
          "draft.citation",
          `Evidence field ${field} requires source IDs and exact supporting quotations from admitted evidence.`,
        ),
      );
    if (
      fileFields.has(field) &&
      !removal &&
      !bound.some((citation) =>
        evidence
          .find((item) => item.id === citation.evidence_id)
          ?.content.includes(value),
      )
    )
      findings.push(
        finding(
          "draft.file",
          `File ID for ${field} must be present in the cited source; a locator or digest is not an uploaded Stripe file.`,
        ),
      );
    form[`evidence[${field}]`] = value;
  }
  if (characters > 150000)
    findings.push(
      finding(
        "draft.provider_limit",
        "Stripe limits the combined evidence text to 150,000 characters. Select relevant material; never truncate it silently.",
      ),
    );
  if (
    citations.some(
      (citation) =>
        !Object.hasOwn(selected, citation?.field) ||
        !sourceIds.has(citation?.evidence_id),
    )
  )
    findings.push(
      finding(
        "draft.unbound",
        "Every citation must belong to an actual selected field and admitted source.",
      ),
    );
  if (
    context.actionable &&
    !["contest", "refund_already_sent"].includes(posture)
  )
    findings.push(
      finding(
        "draft.review",
        "This posture is a recommendation for human review, not an instruction to accept liability or submit evidence.",
      ),
    );
  if (questions.length)
    findings.push(
      finding(
        "draft.questions",
        "Resolve the open questions before formal submission.",
      ),
    );
  if (context.actionable && Object.keys(form).length < 2)
    findings.push(
      finding(
        "draft.empty",
        "A response requires substantive provider evidence, not just an intended posture.",
      ),
    );
  if (
    posture === "refund_already_sent" &&
    !(safe(charge.amount_refunded) && charge.amount_refunded > 0)
  )
    findings.push(
      finding(
        "draft.refund",
        "The current charge read does not establish any refund; never claim one from a receipt label or source summary.",
      ),
    );
  return {
    prepared_response: {
      schema: "runx/dispute-respond.response.v1",
      decision: findings.length ? "held" : "ready",
      provider: "stripe",
      account_id: context.account_id,
      dispute_id: context.dispute_id,
      posture: nonempty(posture) ? posture : "needs_more_evidence",
      summary: draft.summary ?? findings.map((item) => item.message).join(" "),
      open_questions: questions,
      sources: evidence,
      citations,
      case_evidence: context.case_evidence,
      charge_evidence: context.charge_evidence,
      observed_at: context.observed_at,
      acquisition: context.acquisition,
      request: {
        account_id: context.account_id,
        dispute_id: context.dispute_id,
        charge_id: dispute.charge_id,
        amount_minor: dispute.amount_minor,
        currency: dispute.currency,
        expected_status: dispute.status,
        amount_refunded: charge.amount_refunded,
        prior_submission_count: dispute.submission_count,
        submission_count: dispute.submission_count + 1,
        expected_evidence: staged,
        evidence_form: form,
      },
      findings,
      filing_status: "not_attempted",
    },
  };
}

export function admitFiling(inputs) {
  const prior = record(inputs.prepared_response);
  if (
    prior.schema !== "runx/dispute-respond.response.v1" ||
    prior.provider !== "stripe" ||
    prior.filing_status !== "not_attempted"
  )
    throw new Error(
      "Use an unchanged current prepared response, with no prior filing attempt.",
    );
  const context = prepareContext({
    account_id: prior.account_id,
    dispute_id: prior.dispute_id,
    case_evidence: prior.case_evidence,
    charge_evidence: prior.charge_evidence,
    observed_at: inputs.observed_at,
    evidence: list(prior.sources).filter(
      (item) => !["provider-dispute", "provider-charge"].includes(item?.id),
    ),
    acquisition: prior.acquisition,
  }).case_context;
  const fields = Object.fromEntries(
    Object.entries(record(prior.request?.evidence_form))
      .filter(([key]) => /^evidence\[.+\]$/u.test(key))
      .map(([key, value]) => [key.slice(9, -1), value]),
  );
  const rebuilt = prepareResponse({
    case_context: context,
    response_draft: {
      posture: prior.posture,
      summary: prior.summary,
      open_questions: prior.open_questions,
      fields,
      citations: prior.citations,
    },
  }).prepared_response;
  const sameRequest =
    Object.keys(rebuilt.request).length ===
      Object.keys(record(prior.request)).length &&
    Object.entries(rebuilt.request).every(([key, value]) =>
      ["evidence_form", "expected_evidence"].includes(key)
        ? Object.keys(value).length ===
            Object.keys(record(prior.request[key])).length &&
          Object.entries(value).every(
            ([field, content]) => prior.request[key][field] === content,
          )
        : prior.request[key] === value,
    );
  if (
    prior.decision !== "ready" ||
    rebuilt.decision !== "ready" ||
    !sameRequest
  )
    throw new Error(
      "Prepared evidence is incomplete, changed, stale or no longer actionable; review its recorded gaps before filing.",
    );
  return { filing: { request: rebuilt.request, prepared_response: prior } };
}

export function confirmFiling(inputs) {
  const prepared = record(record(inputs.filing).prepared_response);
  const packet = record(
    record(inputs.provider_result).data ?? inputs.provider_result,
  );
  const result = record(packet.result);
  const request = record(prepared.request);
  const findings = [];
  if (
    packet.schema !== "runx.provider.operation.v1" ||
    packet.provider !== "stripe" ||
    packet.operation !== "dispute.file" ||
    packet.status !== "success" ||
    packet.access !== "mutate" ||
    packet.target !==
      `stripe://${prepared.account_id}/disputes/${prepared.dispute_id}` ||
    packet.finality !== "confirmed"
  )
    findings.push(
      finding(
        "filing.readback",
        "Formal submission needs successful native execution with independent provider readback for this exact merchant and dispute.",
      ),
    );
  for (const field of [
    "dispute_id",
    "charge_id",
    "amount_minor",
    "currency",
    "submission_count",
  ])
    if (result[field] !== request[field])
      findings.push(
        finding("filing.identity", `Provider readback did not match ${field}.`),
      );
  if (
    result.submission_count !== request.prior_submission_count + 1 ||
    result.has_evidence !== true
  )
    findings.push(
      finding(
        "filing.counter",
        "Readback did not establish one new evidence submission; do not retry the uncertain act.",
      ),
    );
  for (const [key, value] of Object.entries(record(request.evidence_form))) {
    if (
      key !== "submit" &&
      record(result.evidence)[key.slice(9, -1)] !==
        (value === "" ? null : value)
    )
      findings.push(
        finding(
          "filing.evidence",
          `Provider evidence differs from the approved ${key}. Reconcile before another act.`,
        ),
      );
  }
  return {
    filing_result: {
      schema: "runx/dispute-respond.filing.v1",
      state: findings.length ? "unconfirmed" : "filed",
      provider: "stripe",
      account_id: prepared.account_id,
      dispute_id: prepared.dispute_id,
      summary: findings.length
        ? "A filing attempt occurred but its exact evidence is not confirmed. Reconcile the original attempt; do not repeat it."
        : "The approved evidence was submitted and read back. The dispute outcome remains the provider’s separate decision.",
      prepared_response: prepared,
      provider_evidence: packet,
      findings,
    },
  };
}
