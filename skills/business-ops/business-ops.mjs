const LANE_IDS = Object.freeze([
  "classify",
  "docs",
  "release",
  "issue",
  "send",
  "spend",
  "audit",
]);

export function packageRoute(inputs) {
  const lanes = {};
  for (const id of LANE_IDS) {
    const packet = inputs[id];
    if (!packet || typeof packet !== "object" || Array.isArray(packet)) {
      throw new Error("business-ops finalizer requires lane packet " + id);
    }
    lanes[id] = packet;
  }
  return {
    lane_packets: {
      schema: "runx.business_ops_route.v1",
      signal: String(inputs.signal || "").trim(),
      lanes,
    },
  };
}

export function finalizeDurableRoute(inputs) {
  const lanePacket = requiredObject(inputs.lane_packet, "lane_packet");
  const append = requiredObject(inputs.append_result, "append_result");
  const readback = requiredObject(inputs.projection_result, "projection_result");
  const appendStatus = requiredText(append.status, "append_result.status");
  if (!["committed", "idempotent_replay"].includes(appendStatus)) {
    throw new Error("append_result.status must prove a commit or idempotent replay");
  }
  if (append.operation !== "events.append") {
    throw new Error("append_result.operation must be append_event");
  }
  if (readback.operation !== "projection.read" || readback.status !== "read") {
    throw new Error("projection_result must be a successful read_projection");
  }
  const aggregateId = requiredText(append.aggregate_id, "append_result.aggregate_id");
  if (readback.aggregate_id !== aggregateId) {
    throw new Error("projection_result.aggregate_id changed after append");
  }

  return {
    lane_packet: lanePacket,
    route_persistence: {
      schema: "runx.business_ops.route_persistence.v1",
      aggregate_id: aggregateId,
      append_status: appendStatus,
      before_version: requiredInteger(append.before_version, "append_result.before_version"),
      after_version: requiredInteger(append.after_version, "append_result.after_version"),
      idempotency_key: requiredText(append.idempotency_key, "append_result.idempotency_key"),
      projection: requiredObject(readback.projection, "projection_result.projection"),
      provider_evidence: requiredObject(readback.provider_evidence, "projection_result.provider_evidence"),
    },
  };
}

function requiredObject(value, field) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${field} must be an object`);
  }
  return value;
}

function requiredText(value, field) {
  const parsed = typeof value === "string" ? value.trim() : "";
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

function requiredInteger(value, field) {
  if (!Number.isInteger(value) || value < 0) {
    throw new Error(`${field} must be a non-negative integer`);
  }
  return value;
}
