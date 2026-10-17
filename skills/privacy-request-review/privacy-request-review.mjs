export function judgeRequest(inputs) {
  const request = record(inputs.request_packet);
  const proof = record(inputs.requestor_proof);
  const policy = record(inputs.policy);
  const scope = record(request.scope);
  const lawfulBases = record(policy.lawful_bases);
  const scopeBounds = record(policy.scope_bounds);
  const assertion = record(proof.assertion);

  const requestType = stringValue(request.type);
  const subjectId = stringValue(request.subject_id);
  const requestId = stringValue(scope.request_id) ?? "request:unspecified";
  const requestedClasses = uniqueStrings(scope.data_classes ?? scope.requested_scopes);
  const requestedScopes = uniqueStrings(scope.requested_scopes ?? scope.data_classes);
  const jurisdiction = stringValue(policy.jurisdiction) ?? "unspecified";
  const allowedClasses = uniqueStrings(scopeBounds.data_classes);
  const allowedForType = uniqueStrings(requestType === "export" ? scopeBounds.export_allowed : scopeBounds.erasure_allowed);
  const trustedProviders = uniqueStrings(policy.trusted_identity_providers);
  const identityProvider = stringValue(proof.identity_provider);
  const verifiedAt = stringValue(proof.verified_at);
  const assertionSubject = stringValue(assertion.subject_id);
  const assertionRef = stringValue(assertion.assertion_ref) ?? `${identityProvider ?? "unknown"}:${subjectId ?? "unknown"}`;
  const assertionDigest = stringValue(assertion.assertion_digest);
  const lawfulBasis = requestType ? stringValue(lawfulBases[requestType]) : null;

  const refusals = [];
  if (!["erasure", "export"].includes(requestType ?? "")) refusals.push("request type must be erasure or export");
  if (!subjectId) refusals.push("request_packet.subject_id is missing");
  if (requestedClasses.length === 0) refusals.push("request scope has no data_classes or requested_scopes");
  if (!identityProvider) {
    refusals.push("requestor identity_provider is missing");
  } else if (!trustedProviders.includes(identityProvider)) {
    refusals.push(`identity_provider ${identityProvider} is not trusted for ${jurisdiction}`);
  }
  if (!verifiedAt || Number.isNaN(Date.parse(verifiedAt))) refusals.push("requestor proof has no valid verified_at timestamp");
  if (!assertionSubject || assertionSubject !== subjectId) refusals.push("requestor assertion does not bind to the requested subject_id");
  if (!assertionDigest || !assertionDigest.startsWith("sha256:")) refusals.push("requestor assertion digest is missing or not sha256-prefixed");
  if (!lawfulBasis) refusals.push(`no lawful basis supplied for ${requestType ?? "unknown"} under ${jurisdiction}`);
  for (const dataClass of requestedClasses) {
    if (!allowedClasses.includes(dataClass)) refusals.push(`requested data class ${dataClass} is outside policy.scope_bounds.data_classes`);
    if (!allowedForType.includes(dataClass)) refusals.push(`requested data class ${dataClass} is outside ${requestType ?? "request"}_allowed bounds`);
  }

  const eligible = refusals.length === 0;
  const handoff = eligible ? buildHandoff(requestType, subjectId, requestedClasses, requestedScopes) : null;
  const reason = eligible
    ? `${jurisdiction} ${requestType} request is eligible: verified ${identityProvider} proof matches ${subjectId}, lawful basis is supplied, and scope is bounded to ${requestedClasses.join(", ")}.`
    : `${jurisdiction} ${requestType ?? "request"} refused: ${refusals.join("; ")}.`;
  const verdict = {
    request: {
      request_id: requestId,
      type: requestType ?? "unknown",
      subject_id: subjectId ?? "",
      requested_scopes: requestedScopes,
      data_classes: requestedClasses,
    },
    decision: { eligible, reason },
    escalation: eligible
      ? { required: false, lane: "none", reason: "Requestor identity, subject match, lawful basis, and scope all pass policy." }
      : { required: true, lane: "human_privacy_review", reason },
    legal: { jurisdiction, lawful_basis: lawfulBasis },
    requestor: {
      identity_provider: identityProvider ?? "",
      verified_at: verifiedAt ?? "",
      assertion_ref: assertionRef,
      identity_assertion_digest: assertionDigest ?? "",
    },
    scope_bounds: { data_classes: allowedClasses, requested_scopes: requestedScopes },
    handoff,
  };
  return {
    verdict_draft: {
      verdict,
      event: {
        type: "subject_request.verdict_recorded",
        payload: { packet: "runx.data_subject_request.v1", ...verdict },
      },
    },
  };
}

export function finalizeVerdict(inputs) {
  const draft = record(record(inputs.verdict_draft).verdict);
  const appended = record(inputs.append_result);
  if (appended.operation !== "events.append" || typeof appended.after_version !== "number") {
    throw new Error("durable verdict append evidence is missing");
  }
  return {
    subject_request_verdict: {
      schema: "runx.data_subject_request.v1",
      ...draft,
      persistence: {
        append_status: "committed",
        aggregate_id: stringValue(appended.aggregate_id) ?? "",
        after_version: appended.after_version,
      },
      request_digest: requiredDigest(inputs.request_digest),
      policy_digest: requiredDigest(inputs.policy_digest),
      downstream_effect_performed: false,
    },
  };
}

function buildHandoff(requestType, subjectId, dataClasses, requestedScopes) {
  const base = {
    subject_id: subjectId,
    data_classes: dataClasses,
    scopes: {
      request_type: requestType,
      requested_scopes: requestedScopes,
      downstream_operator_required: true,
    },
  };
  if (requestType === "export") {
    return {
      ...base,
      path: "downstream.read_projection.redact-pii.message-send",
      scopes: { ...base.scopes, read_operation: "projection.read", redaction_skill: "redact-pii", delivery_skill: "message-send" },
    };
  }
  return {
    ...base,
    path: "downstream.erasure-operator",
    scopes: { ...base.scopes, erasure_operator_required: true },
  };
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function uniqueStrings(value) {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map(stringValue).filter(Boolean))].sort();
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}
