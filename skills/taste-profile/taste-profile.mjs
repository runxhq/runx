export function indexEvidence(inputs) {
  const subject = text(inputs.subject);
  const supplied = Array.isArray(inputs.evidence) ? inputs.evidence : [];
  const blockers = [];
  const sources = [];
  const allowedKinds = new Set(["liked", "disliked", "correction", "instruction"]);

  if (!subject) blockers.push("subject is missing");
  if (supplied.length < 2) blockers.push("at least two evidence items are required");
  if (supplied.length > 50) blockers.push("evidence exceeds 50 items");

  for (const [position, raw] of supplied.slice(0, 50).entries()) {
    const item = object(raw);
    const kind = text(item.kind);
    const content = text(item.content);
    if (!allowedKinds.has(kind)) {
      blockers.push(`evidence[${position}].kind is invalid`);
      continue;
    }
    if (!content || [...content].length > 20_000) {
      blockers.push(`evidence[${position}].content is missing or too large`);
      continue;
    }
    sources.push({
      source_id: `source:${position + 1}`,
      kind,
      label: text(item.label),
      content,
    });
  }

  return {
    evidence_index: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      subject,
      sources,
      blockers,
    },
  };
}

export function finalizeProfile(inputs) {
  const subject = text(inputs.subject);
  const surface = text(inputs.surface);
  const audience = text(inputs.audience);
  const index = object(inputs.evidence_index);
  const draft = object(inputs.taste_profile_draft);
  const sources = Array.isArray(index.sources) ? index.sources : [];
  const known = new Set(sources.map((source) => text(source.source_id)));
  const evidenceDigest = text(inputs.evidence_digest);
  const findings = strings(index.blockers).map((message) => ({
    code: "taste_profile.evidence.invalid",
    message,
  }));
  const profile = {};
  const bindingMap = new Map();
  const collections = ["principles", "likes", "dislikes", "decision_rules", "examples_to_emulate", "examples_to_avoid"];
  for (const collection of collections) profile[collection] = [];

  if (index.decision === "ready") {
    if (!isDigest(evidenceDigest)) {
      findings.push({ code: "taste_profile.evidence.unbound", message: "native evidence digest is missing" });
    }
    if (text(draft.decision) !== "ready") {
      findings.push({ code: "taste_profile.draft.not_ready", message: "draft decision is not ready" });
    }
    if (text(draft.subject) !== subject) {
      findings.push({ code: "taste_profile.subject.changed", message: "draft subject does not match input" });
    }
    for (const collection of collections) {
      for (const binding of object(draft.taste_profile)[collection] ?? []) {
        const { rule, source_ids: sourceIds, confidence } = object(binding);
        if (!text(rule) || !Array.isArray(sourceIds) || sourceIds.length === 0 || sourceIds.some((sourceId) => !known.has(sourceId))) {
          findings.push({ code: "taste_profile.binding.invalid", message: `rule is not bound to admitted evidence: ${text(rule)}` });
          continue;
        }
        if (!["explicit", "observed", "inferred"].includes(confidence)) {
          findings.push({ code: "taste_profile.confidence.invalid", message: `binding confidence is invalid: ${rule}` });
          continue;
        }
        const previous = bindingMap.get(rule);
        if (previous && (previous.confidence !== confidence || JSON.stringify([...previous.source_ids].sort()) !== JSON.stringify([...sourceIds].sort()))) {
          findings.push({ code: "taste_profile.binding.conflict", message: `repeated rule has conflicting evidence or confidence: ${rule}` });
          continue;
        }
        profile[collection].push(rule);
        if (!previous) bindingMap.set(rule, { rule, source_ids: sourceIds, confidence });
      }
    }
    if (bindingMap.size === 0) {
      findings.push({ code: "taste_profile.profile.empty", message: "ready profile has no evidence-bound rules" });
    }
  }

  const ready = index.decision === "ready" && findings.length === 0;
  return {
    taste_profile_packet: {
      decision: ready ? "ready" : "needs_more_evidence",
      subject,
      applicability: ready ? object(draft.applicability) : {
        surfaces: surface ? [surface] : [],
        audience,
        expires_when: [],
      },
      taste_profile: ready ? profile : {
        principles: [],
        likes: [],
        dislikes: [],
        decision_rules: [],
        examples_to_emulate: [],
        examples_to_avoid: [],
      },
      evidence: {
        evidence_digest: evidenceDigest,
        sources: sources.map((source) => ({
          source_id: text(source.source_id),
          kind: text(source.kind),
          label: text(source.label),
        })),
        bindings: ready ? [...bindingMap.values()] : [],
      },
      redactions: ready && Array.isArray(draft.redactions) ? draft.redactions : [],
      stop_conditions: ready && Array.isArray(draft.stop_conditions) ? draft.stop_conditions : [],
      receipt_notes: { authority: "context-only", mutation: false },
      validation: { status: ready ? "pass" : "fail", findings },
    },
  };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function strings(value) {
  return Array.isArray(value)
    ? value.filter((entry) => typeof entry === "string").map((entry) => entry.trim()).filter(Boolean)
    : [];
}

function isDigest(value) {
  return /^sha256:[0-9a-f]{64}$/u.test(value);
}
