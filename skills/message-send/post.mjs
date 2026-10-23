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
  const message = text(content.message);
  const contentMode = message ? "inline" : "reference";
  if (message) {
    if (message.length > 4000) {
      findings.push({ code: "slack.message.too_large", message: "message exceeds 4000 characters" });
    }
  } else if (!suppliedUri || !isDigest(suppliedDigest)) {
    findings.push({
      code: "slack.content.unbound",
      message: "content requires message or content_uri with sha256 digest",
    });
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

function slackDisplayedText(value) {
  return value
    .replace(/<@([UW][A-Z0-9]+)\|[^>]+>/gu, "<@$1>")
    .replace(/<(slack:\/\/[^<>\s|]+)(?:\|[^>]*)?>/gu, "$1")
    .replace(/\s+/gu, " ").trim();
}

export function reconcileExactNotification(inputs) {
  const plan = object(inputs.notification_plan);
  const read = object(inputs.provider_operation);
  const result = object(read.result);
  const messages = Array.isArray(result.messages) ? result.messages : [];
  const message = object(messages[0]);
  const observed = typeof message.preview === "string" ? message.preview : "";
  const expected = typeof inputs.text === "string" ? inputs.text : "";
  const locator = text(inputs.message_locator);
  const channel = text(inputs.channel_locator);
  const occurredAt = Date.parse(text(message.occurred_at));
  const attemptAt = Date.parse(text(inputs.failed_at));
  if (plan.schema !== "runx/message-send.notification_plan.v1" || plan.decision !== "ready_for_provider"
      || plan.provider !== "slack" || plan.channel !== channel
      || plan.content_digest !== text(object(inputs.digest_result).digest)
      || read.schema !== "runx.provider.operation.v1" || read.status !== "success"
      || read.provider !== "slack" || read.operation !== "thread.read"
      || read.finality !== "confirmed" || result.thread_locator !== locator
      || messages.length !== 1 || message.message_locator !== locator
      || !locator.startsWith(`${channel}/`)
      || message.author?.external_id !== result.connected_subject_id
      || !Number.isFinite(occurredAt) || !Number.isFinite(attemptAt)
      || occurredAt < attemptAt - 300000
      || !observed || observed.length >= 2000
      || slackDisplayedText(observed) !== slackDisplayedText(expected)
      || !text(read.readback_uri).startsWith("runx:provider-readback:sha256:")) {
    throw new Error("Slack post could not be reconciled to the exact pending notification");
  }
  // The post is reconciled from the thread read that observed it; that read
  // is the provider operation, and this is what it proves about the post.
  return { reconciliation: {
    message_locator: locator, conversation_locator: channel,
    content_digest: plan.content_digest, occurred_at: message.occurred_at,
    normalization: "slack_bounded_preview", provider_result_digest: read.result_digest,
    readback_uri: read.readback_uri,
  } };
}

export function prepareNotificationReadback(inputs) {
  const delivery = object(inputs.delivery);
  const result = object(delivery.result);
  const expected = typeof inputs.text === "string" ? inputs.text : "";
  if (delivery.status !== "success" || delivery.operation !== "channel.post" ||
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
        runner: "post",
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
