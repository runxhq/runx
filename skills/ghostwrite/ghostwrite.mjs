export function prepareEvidence(inputs) {
  const objective = text(inputs.objective);
  const audience = text(inputs.audience);
  const channel = text(inputs.channel);
  const evidence = object(inputs.evidence_pack);
  const blockers = [];

  if (!objective) blockers.push("objective is missing");
  if (!audience) blockers.push("audience is missing");
  if (!channel) blockers.push("channel is missing");
  if (evidence.decision !== "ready") blockers.push("evidence decision is not ready");
  if (object(evidence.validation).status !== "pass") blockers.push("evidence validation did not pass");

  const sourceEvidence = Array.isArray(evidence.source_evidence) ? evidence.source_evidence : [];
  const sourceDigests = [...new Set(sourceEvidence.map((source) => text(source?.evidence_digest)).filter(isDigest))];
  if (sourceDigests.length === 0) blockers.push("evidence has no source digests");
  const evidenceLog = Array.isArray(evidence.evidence_log) ? evidence.evidence_log : [];
  if (evidenceLog.length === 0) blockers.push("evidence log is empty");
  for (const entry of evidenceLog) {
    if (!sourceDigests.includes(text(entry?.source_digest))) {
      blockers.push("evidence log cites an unknown source digest");
    }
  }

  const scope = { audience, channel };
  const writingContexts = [
    admitWritingContext(inputs.brand_context, "brand_voice", inputs.brand_context_digest, scope, blockers),
    admitWritingContext(inputs.taste_context, "taste_profile", inputs.taste_context_digest, scope, blockers),
  ].filter(Boolean);

  const evidencePacketDigest = text(inputs.evidence_packet_digest);
  if (!isDigest(evidencePacketDigest)) blockers.push("native evidence packet digest is missing");

  return {
    evidence_context: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      objective,
      audience,
      channel,
      evidence_packet_digest: evidencePacketDigest,
      research_brief: object(evidence.research_brief),
      evidence_log: evidenceLog,
      decision_support: Array.isArray(evidence.decision_support) ? evidence.decision_support : [],
      source_digests: sourceDigests,
      writing_contexts: writingContexts,
      context_requirements: writingContexts.map(({ type, packet_digest, scope, rules, stop_conditions }) => ({
        type, packet_digest, scope, rules, stop_conditions,
      })),
      blockers: [...new Set(blockers)],
    },
  };
}

export function prepareContent(inputs) {
  const channel = text(inputs.channel);
  const packet = object(inputs.draft);
  const draft = object(packet.draft);
  const binding = object(packet.evidence_binding);
  const evidencePacketDigest = text(binding.evidence_packet_digest);
  const sourceDigests = strings(binding.source_digests);
  const blockers = [];
  if (!channel) blockers.push("channel is missing");
  if (packet.decision !== "ready" || object(packet.validation).status !== "pass") {
    blockers.push("draft is not ready and validated");
  }
  if (!text(draft.title) || !text(draft.body)) blockers.push("draft has no title and body");
  if (!isDigest(evidencePacketDigest) || sourceDigests.length === 0 || !sourceDigests.every(isDigest)) {
    blockers.push("draft is not bound to its evidence");
  }
  const claims = Array.isArray(draft.claims) ? draft.claims : [];
  if (claims.some((claim) => {
    const cited = strings(object(claim).source_digests);
    return cited.length === 0 || cited.some((digest) => !sourceDigests.includes(digest));
  })) {
    blockers.push("a claim cites a source the draft is not bound to");
  }
  const ready = blockers.length === 0;
  const payload = ready
    ? { title: text(draft.title), body: text(draft.body) }
    : { title: "", body: "" };
  return {
    publish_draft: {
      decision: ready ? "ready_for_handoff" : "needs_input",
      channel,
      headline: payload.title,
      body: payload.body,
      evidence_binding: ready
        ? { evidence_packet_digest: evidencePacketDigest, source_digests: sourceDigests }
        : {},
      delivery_status: "not_sent",
      blockers,
    },
    digest_input: ready ? JSON.stringify({ channel, payload }) : "",
  };
}

export function bindContent(inputs) {
  const draft = object(inputs.publish_draft);
  const ready = draft.decision === "ready_for_handoff";
  const payloadDigest = text(object(inputs.digest_result).digest);
  if (ready && !isDigest(payloadDigest)) throw new Error("native payload digest is missing");
  return {
    content_publish_packet: {
      ...draft,
      payload_digest: ready ? payloadDigest : "",
    },
  };
}

export function prepareHandoff(inputs) {
  const channel = text(inputs.channel);
  const boundaryKind = inputs.boundary_kind;
  const packet = object(inputs.packet);
  const target = object(inputs.target);
  const blockers = [];
  if (packet.decision !== "ready_for_handoff" || packet.delivery_status !== "not_sent") {
    blockers.push("publication packet is not ready and unsent");
  }
  if (!isDigest(text(packet.payload_digest))) blockers.push("publication packet has no payload digest");
  if (!channel || text(packet.channel) !== channel) blockers.push("publication packet is for another channel");
  if (Object.keys(target).length === 0) blockers.push("target is missing");
  const ready = blockers.length === 0;
  const external = boundaryKind === "external";
  return {
    handoff_packet: {
      decision: ready ? "handoff_ready" : "needs_input",
      channel,
      payload_digest: text(packet.payload_digest),
      packet,
      target,
      delivery_status: "not_sent",
      blockers,
    },
    boundary_state: {
      boundary_kind: boundaryKind,
      completion_state: ready ? "handoff_ready" : "needs_input",
      next_actor: ready ? "governed_provider_adapter" : "operator",
      approval_required_for_delivery: external,
      ack_expected: external,
    },
  };
}

// The applicability field each writing-context packet declares its channels in:
// runx/brand-voice.context.v1 names `channels`, runx/taste-profile.context.v1 names `surfaces`.
const CONTEXT_CHANNEL_FIELD = { brand_voice: "channels", taste_profile: "surfaces" };

function admitWritingContext(raw, type, packetDigest, scope, blockers) {
  if (raw === undefined || raw === null) return null;
  const packet = object(raw);
  if (packet.decision !== "ready" || object(packet.validation).status !== "pass") {
    blockers.push(`${type} context is not ready and validated`);
  }
  const bindings = Array.isArray(object(packet.evidence).bindings) ? object(packet.evidence).bindings : [];
  const rules = [...new Set(bindings.map((binding) => text(binding?.rule)).filter(Boolean))];
  if (rules.length === 0) blockers.push(`${type} context has no bounded rules`);
  const admittedDigest = text(packetDigest);
  if (!isDigest(admittedDigest)) blockers.push(`${type} native context digest is missing`);
  const applicability = object(packet.applicability);
  const appliesToAudience = Boolean(scope.audience) && text(applicability.audience) === scope.audience;
  const appliesToChannel = Boolean(scope.channel)
    && strings(applicability[CONTEXT_CHANNEL_FIELD[type]]).includes(scope.channel);
  if (!appliesToAudience) blockers.push(`${type} context does not apply to the requested audience`);
  if (!appliesToChannel) blockers.push(`${type} context does not apply to the requested channel`);
  if (!appliesToAudience || !appliesToChannel) return null;
  return {
    type,
    packet_digest: admittedDigest,
    scope: { audience: scope.audience, channel: scope.channel },
    applicability,
    profile: object(packet[type]),
    evidence_bindings: bindings,
    rules,
    stop_conditions: strings(packet.stop_conditions),
  };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function strings(value) {
  return Array.isArray(value) ? value.map(text).filter(Boolean) : [];
}

function isDigest(value) {
  return /^sha256:[0-9a-f]{64}$/u.test(value);
}
