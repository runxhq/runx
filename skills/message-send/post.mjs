export function prepareNotification(inputs) {
  const channel = text(inputs.channel);
  const principal = text(inputs.principal);
  const content = object(inputs.content);
  const findings = [];

  if (!/^(#[a-z0-9_-]{1,80}|[CGD][A-Z0-9]{2,}|slack:\/\/[A-Z][A-Z0-9]{2,}\/[CGD][A-Z0-9]{2,})$/u.test(channel)) {
    findings.push({
      code: "slack.channel.invalid",
      message: "channel must be a Slack #name, id, or exact slack:// locator",
    });
  }
  if (!principal) findings.push({ code: "slack.principal.missing", message: "principal is required" });

  const suppliedUri = text(content.content_uri);
  const suppliedDigest = text(content.digest);
  const message = typeof content.message === "string" ? content.message : "";
  const contentMode = message ? "inline" : "reference";
  if (!message && (!suppliedUri || !isDigest(suppliedDigest))) {
    findings.push({ code: "slack.content.unbound", message: "content requires message or content_uri with sha256 digest" });
  }

  return {
    notification_admission: {
      principal,
      channel,
      content_mode: contentMode,
      supplied_uri: suppliedUri,
      supplied_digest: suppliedDigest,
      findings,
    },
    digest_input: message,
  };
}

export function bindNotification(inputs) {
  const admission = object(inputs.notification_admission);
  const digestResult = object(inputs.digest_result);
  const findings = Array.isArray(admission.findings) ? [...admission.findings] : [];
  const inline = admission.content_mode === "inline";
  const contentDigest = inline ? text(digestResult.digest) : text(admission.supplied_digest);
  if (!isDigest(contentDigest)) {
    findings.push({ code: "slack.content.digest_missing", message: "native content digest is missing" });
  }
  const contentUri = inline ? `inline:${contentDigest}` : text(admission.supplied_uri);

  return {
    slack_context: {
      decision: findings.length === 0 ? "ready" : "blocked",
      principal: text(admission.principal),
      channel: text(admission.channel),
      content_uri: contentUri,
      content_digest: contentDigest,
      findings,
    },
  };
}

export function finalizeNotification(inputs) {
  const context = object(inputs.slack_context);
  const sendPlan = object(inputs.send_plan);
  const findings = Array.isArray(context.findings) ? [...context.findings] : [];
  if (context.decision === "ready") {
    if (text(sendPlan.decision) !== "ready") {
      findings.push({ code: "slack.send_plan.not_ready", message: "the send plan is not ready" });
    }
    if (text(sendPlan.provider?.name) !== "slack") {
      findings.push({ code: "slack.provider.mismatch", message: "the send plan's provider is not Slack" });
    }
    if (text(sendPlan.channel) !== "chat") {
      findings.push({ code: "slack.channel_kind.mismatch", message: "the send plan's channel is not chat" });
    }
    if (text(sendPlan.principal?.uri) !== text(context.principal)) {
      findings.push({ code: "slack.principal.mismatch", message: "the send plan's principal does not match" });
    }
    if (text(sendPlan.audience?.locator) !== text(context.channel)) {
      findings.push({ code: "slack.audience.mismatch", message: "the send plan's audience does not match" });
    }
    if (text(sendPlan.content?.digest) !== text(context.content_digest)) {
      findings.push({ code: "slack.content.mismatch", message: "the send plan's content digest does not match" });
    }
  }

  return { notify_plan: notificationPacket(context, sendPlan, findings) };
}

export function finalizeExactNotification(inputs) {
  const context = object(inputs.slack_context);
  const findings = Array.isArray(context.findings) ? [...context.findings] : [];
  const sendPlan = {
    decision: context.decision === "ready" && findings.length === 0 ? "ready" : "needs_input",
    origin: "message-send.exact",
    authority: "native_delivery_gate",
  };
  return { notify_plan: notificationPacket(context, sendPlan, findings) };
}

export function prepareReconciliationRead(inputs) {
  const plan = inputs.notification_plan;
  const channel = inputs.channel_locator;
  const locator = inputs.message_locator;
  if (plan.schema !== "runx/message-send.notification_plan.v1" || plan.decision !== "ready_for_provider"
      || plan.provider !== "slack" || plan.channel !== channel || plan.validation.status !== "pass"
      || plan.content_digest !== inputs.digest_result.digest || !locator.startsWith(channel + "/")
      || !Number.isFinite(Date.parse(inputs.failed_at))) {
    throw new Error("Selected Slack message, original plan and exact text must be bound before readback");
  }
  return {
    read_request: { conversation_locator: channel, message_locator: locator, expected_text: inputs.text },
    expected_result: { conversation_locator: channel, message_locator: locator, content_digest: plan.content_digest },
  };
}

export function reconcileExactNotification(inputs) {
  const read = inputs.provider_operation;
  return { reconciliation: {
    ...read.result,
    normalization: "native_exact_message_read",
    provider_result_digest: read.result_digest,
    readback_uri: read.readback_uri,
    original_effect_status: "unchanged",
    attempt_reported_at: inputs.failed_at,
  } };
}

export function prepareMessageReadback(inputs) {
  const delivery = object(inputs.delivery);
  const result = object(delivery.result);
  const expected = typeof inputs.text === "string" ? inputs.text : "";
  if (delivery.status !== "success" || delivery.operation !== inputs.operation ||
      !text(result.message_locator) || !expected) {
    throw new Error("notification delivery lacks a bound message for readback");
  }
  return { read_request: { ...result, expected_text: expected } };
}

function notificationPacket(context, sendPlan, findings) {
  const ready = context.decision === "ready" && findings.length === 0;
  return {
      schema: "runx/message-send.notification_plan.v1",
      decision: ready ? "ready_for_provider" : "blocked",
      provider: "slack",
      principal: text(context.principal),
      channel: text(context.channel),
      content_uri: text(context.content_uri),
      content_digest: text(context.content_digest),
      send_plan: sendPlan,
      provider_status: "not_called",
      delivery_status: "not_sent",
      downstream_handoff: ready ? {
        skill: "message-send",
        method: "post",
        state: "ready_for_approval",
      } : {},
      validation: { status: ready ? "pass" : "fail", findings },
    };
}

function isDigest(value) {
  return /^sha256:[0-9a-f]{64}$/u.test(value);
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}
