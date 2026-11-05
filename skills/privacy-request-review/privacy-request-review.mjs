export function judgeRequest({request_packet: request, requestor_proof: proof, policy}) {
  const scope = request.scope;
  const classes = scope.data_classes ?? scope.requested_scopes;
  const requestedScopes = scope.requested_scopes ?? scope.data_classes;
  const allowed = policy.scope_bounds.data_classes;
  const typeAllowed = request.type === "export" ? policy.scope_bounds.export_allowed : policy.scope_bounds.erasure_allowed;
  const basis = policy.lawful_bases[request.type] ?? null;
  const refusals = [];
  if (!policy.trusted_identity_providers.includes(proof.identity_provider)) refusals.push("the supplied assertion provider is outside policy trust");
  if (!Number.isFinite(Date.parse(proof.verified_at))) refusals.push("the assertion observation time is invalid");
  if (proof.assertion.subject_id !== request.subject_id) refusals.push("the assertion subject does not match the request");
  if (!basis) refusals.push("policy supplies no basis for this request type");
  const outside = classes.filter(value => !allowed.includes(value) || !typeAllowed.includes(value));
  if (outside.length) refusals.push(outside.length+" requested data class(es) exceed the policy/type bounds");
  const scopeMismatch = requestedScopes.filter(value => !classes.includes(value) || !allowed.includes(value) || !typeAllowed.includes(value));
  if (scopeMismatch.length) refusals.push(scopeMismatch.length+" requested scope(s) are outside the requested classes or policy/type bounds");
  const eligible = refusals.length === 0;
  const reason = eligible
    ? "Supplied "+proof.identity_provider+" assertion metadata matches the subject and configured "+policy.jurisdiction+" policy for "+classes.length+" data class(es); identity verification remains the upstream owner's responsibility."
    : "Request refused: "+refusals.join("; ")+".";
  const verdict = {
    request:{request_id:scope.request_id,type:request.type,subject_id:request.subject_id,requested_scopes:requestedScopes,data_classes:classes},
    decision:{eligible,reason},
    escalation:eligible ? {required:false,lane:"none",reason:"The supplied policy and assertion bindings pass; this grants no downstream execution authority."} : {required:true,lane:"human_privacy_review",reason},
    legal:{jurisdiction:policy.jurisdiction,lawful_basis:basis},
    requestor:{identity_provider:proof.identity_provider,verified_at:proof.verified_at,assertion_id:proof.assertion.assertion_id ?? "",identity_assertion_digest:proof.assertion.assertion_digest},
    scope_bounds:{data_classes:allowed,requested_scopes:requestedScopes},
    handoff:eligible ? {
      subject_id:request.subject_id,data_classes:classes,
      scopes:{request_type:request.type,requested_scopes:requestedScopes,downstream_operator_required:true},
      path:null,
    } : null,
  };
  return {verdict_draft:{verdict,event:{type:"subject_request.verdict_recorded",payload:{packet:"runx/privacy-request-review.verdict.v1",...verdict}}}};
}

export function finalizeVerdict(inputs) {
  const {append_result: appended, recorded_readback: readback, expected_version: expected} = inputs;
  const sameSource = value => value.data_source_uri === inputs.data_source_uri &&
    value.resource === inputs.resource && value.aggregate_id === inputs.aggregate_id;
  const committed = appended.status === "committed" && appended.before_version === expected && appended.after_version === expected + 1;
  const replayed = appended.status === "idempotent_replay" && appended.before_version >= expected + 1 && appended.after_version === appended.before_version;
  if ((!committed && !replayed) || appended.operation !== "events.append" || !sameSource(appended)) {
    throw new Error("The verdict append did not commit the exact expected source/version; retain its conflict and original request identity.");
  }
  const event = readback.events?.[0];
  if (readback.status !== "read" || !sameSource(readback) ||
      event?.version !== expected + 1 ||
      event.event_id !== appended.event_id || event.event_digest !== appended.event_digest ||
      event.event_type !== "subject_request.verdict_recorded") {
    throw new Error("The independent readback did not prove the exact recorded verdict event.");
  }
  return {subject_request_verdict:{
    schema:"runx/privacy-request-review.verdict.v1",...inputs.verdict_draft.verdict,
    persistence:{append_status:"committed",aggregate_id:appended.aggregate_id,after_version:event.version},
    request_digest:inputs.request_digest,policy_digest:inputs.policy_digest,downstream_effect_performed:false,
  }};
}
