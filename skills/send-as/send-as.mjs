const SEND_OPERATION = "message.send";
const READ_OPERATION = "message.read";

export function prepareDelivery(inputs) {
  const plan = object(inputs.send_plan);
  const connector = object(inputs.connector);
  const provider = requiredText(connector.provider, "connector.provider");
  const target = requiredText(connector.target, "connector.target");
  const principal = object(plan.principal);
  const audience = object(plan.audience);
  const content = object(plan.content);
  const gates = object(plan.gates);

  if (text(plan.decision) !== "ready") throw new Error("send_plan.decision must be ready");
  if (Array.isArray(plan.blockers) && plan.blockers.length > 0) {
    throw new Error("send_plan must not contain blockers");
  }
  if (gates.preflight_required !== true || gates.human_approval_required !== true) {
    throw new Error("send_plan must require preflight and human approval");
  }
  requiredText(gates.approval_ref, "send_plan.gates.approval_ref");
  if (text(plan.provider?.name) !== provider) {
    throw new Error("connector.provider does not match send_plan.provider.name");
  }

  const principalRef = requiredText(principal.ref, "send_plan.principal.ref");
  const audienceRef = requiredText(audience.ref, "send_plan.audience.ref");
  const contentDigest = requiredDigest(content.digest, "send_plan.content.digest");
  if (audience.requires_reconfirmation === true) {
    throw new Error("send_plan audience still requires reconfirmation");
  }

  const payload = {
    schema: "runx.message.send.v1",
    principal: {
      type: requiredText(principal.type, "send_plan.principal.type"),
      ref: principalRef,
    },
    send_class: requiredText(plan.send_class, "send_plan.send_class"),
    channel: requiredText(plan.channel, "send_plan.channel"),
    audience: {
      type: requiredText(audience.type, "send_plan.audience.type"),
      ref: audienceRef,
    },
    content: {
      draft_ref: requiredText(content.draft_ref, "send_plan.content.draft_ref"),
      digest: contentDigest,
      subject_or_title: text(content.subject_or_title),
    },
  };
  const expectedResult = {
    principal_ref: principalRef,
    audience_ref: audienceRef,
    content_digest: contentDigest,
  };
  const digestSubject = {
    schema: "runx.message.send.binding.v1",
    provider,
    target,
    payload,
    expected_result: expectedResult,
  };

  return {
    delivery_draft: {
      provider,
      target,
      payload,
      expected_result: expectedResult,
    },
    digest_subject: digestSubject,
  };
}

export function bindDelivery(inputs) {
  const draft = object(inputs.delivery_draft);
  const digestResult = object(inputs.digest_result);
  const digest = requiredDigest(digestResult.digest, "digest_result.digest");
  return {
    delivery_request: {
      provider: requiredText(draft.provider, "delivery_draft.provider"),
      target: requiredText(draft.target, "delivery_draft.target"),
      input: {
        target: requiredText(draft.target, "delivery_draft.target"),
        message: requiredObject(draft.payload, "delivery_draft.payload"),
      },
      expected_result: requiredObject(draft.expected_result, "delivery_draft.expected_result"),
      idempotency_key: `send-as:${digest}`,
    },
  };
}

export function finalizeSend(inputs) {
  const plan = object(inputs.send_plan);
  const apply = object(inputs.apply_result);
  const readback = object(inputs.readback_result);
  const connector = object(inputs.connector);
  const provider = text(connector.provider);
  const target = text(connector.target);
  const errors = [];

  if (text(plan.decision) !== "ready") errors.push("send plan was not ready");
  if (text(apply.status) !== "success") errors.push("provider mutation did not succeed");
  if (text(readback.status) !== "success") errors.push("provider readback did not succeed");
  if (text(apply.provider) !== provider || text(readback.provider) !== provider) {
    errors.push("provider identity changed across apply and readback");
  }
  if (text(apply.target) !== target || text(readback.target) !== target) {
    errors.push("delivery target changed across apply and readback");
  }
  if (text(apply.operation) !== SEND_OPERATION) errors.push("provider mutation operation drifted");
  if (text(readback.operation) !== READ_OPERATION) errors.push("provider readback operation drifted");
  const applied = object(apply.result);
  const observed = object(readback.result);
  for (const field of ["message_id", "principal_ref", "audience_ref", "content_digest"]) {
    if (!text(applied[field]) || text(applied[field]) !== text(observed[field])) {
      errors.push(`${field} changed across apply and readback`);
    }
  }
  if (text(applied.content_digest) !== text(plan.content?.digest)) {
    errors.push("provider result content digest does not match the approved plan");
  }
  if (text(applied.principal_ref) !== text(plan.principal?.ref)) {
    errors.push("provider result principal does not match the approved plan");
  }
  if (text(applied.audience_ref) !== text(plan.audience?.ref)) {
    errors.push("provider result audience does not match the approved plan");
  }
  if (!text(apply.idempotency_key) || text(applied.idempotency_key) !== text(apply.idempotency_key)) {
    errors.push("provider mutation did not preserve the runtime idempotency binding");
  }
  if (text(observed.idempotency_key) !== text(apply.idempotency_key)) {
    errors.push("provider readback did not observe the original mutation idempotency binding");
  }

  const complete = errors.length === 0;
  return {
    send_result: {
      schema: "runx.send_as.result.v1",
      status: complete ? "sent" : "failed",
      outcome: complete ? "completed" : "failed",
      provider,
      target,
      operation: SEND_OPERATION,
      content_digest: text(plan.content?.digest),
      operation_id: text(apply.operation_id),
      readback_ref: text(readback.readback_ref),
      idempotency_key: text(apply.idempotency_key),
      evidence: {
        mutation_readback_ref: text(apply.readback_ref),
        verification_readback_ref: text(readback.readback_ref),
      },
      errors,
    },
  };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function requiredObject(value, field) {
  const parsed = object(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}

function requiredText(value, field) {
  const parsed = text(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

function requiredDigest(value, field) {
  const parsed = requiredText(value, field);
  if (!/^sha256:[0-9a-f]{64}$/u.test(parsed)) throw new Error(`${field} must be a sha256 digest`);
  return parsed;
}

function text(value) {
  return typeof value === "string" ? value : "";
}
