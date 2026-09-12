import { createHash } from "node:crypto";

function readJsonInput(name, fallback = null) {
  const raw = process.env[`RUNX_INPUT_${name}`];
  if (raw === undefined || raw === "") return fallback;
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

function sha256(value) {
  return `sha256:${createHash("sha256").update(value).digest("hex")}`;
}

function failClosed(reason, details = {}) {
  return {
    release_execution_result: {
      rail: "mock-release",
      gate_id: "release.publish.approval",
      consumed: false,
      advanced: false,
      reason,
      details,
    },
    release_effect_ref: "",
    release_advanced: false,
    consumed_gate: {
      gate_id: "release.publish.approval",
      consumed: false,
      approved: false,
      reason,
    },
  };
}

const approval = readJsonInput("RELEASE_PUBLISH_APPROVAL", {});
const decision = readJsonInput("DECISION", {});
const targetRef = readJsonInput("ACT_TARGET_REF", "");
const monitorRunRef = readJsonInput("MONITOR_RUN_REF", "");

let packet;
if (approval?.gate_id !== "release.publish.approval") {
  packet = failClosed("wrong_release_gate", { observed_gate_id: approval?.gate_id ?? null });
} else if (approval?.approved !== true || approval?.answer?.approved !== true) {
  packet = failClosed("release_publish_approval_not_approved", {
    approved: approval?.approved ?? null,
    answer_approved: approval?.answer?.approved ?? null,
  });
} else if (decision?.action !== "rollback") {
  packet = failClosed("release_rail_only_advances_rollback_in_this_case", {
    action: decision?.action ?? null,
  });
} else if (!targetRef || typeof targetRef !== "string") {
  packet = failClosed("missing_release_target_ref");
} else {
  const version = decision?.version_target?.version ?? null;
  const digest = decision?.version_target?.digest ?? null;
  const reason = approval.reason || decision.reason || "approved";
  const command = {
    rail: "mock-release",
    operation: "advance_on_release_publish_approval",
    gate_id: approval.gate_id,
    approved: true,
    action: decision.action,
    target_ref: targetRef,
    version,
    digest,
    reason,
    source_signal_ref: monitorRunRef || null,
  };
  const commandDigest = sha256(JSON.stringify(command));
  packet = {
    release_execution_result: {
      ...command,
      consumed: true,
      advanced: true,
      command_digest: commandDigest,
      note: "Mock release rail consumed release.publish.approval in this same graph run; no deploy API was called.",
    },
    release_effect_ref: targetRef,
    release_advanced: true,
    consumed_gate: {
      gate_id: approval.gate_id,
      consumed: true,
      approved: true,
      reason,
      answer: approval.answer,
      command_digest: commandDigest,
    },
  };
}

process.stdout.write(`${JSON.stringify(packet)}\n`);
