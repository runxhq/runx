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
    if (!content || content.length > 20_000) {
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
    const profile = object(draft.taste_profile);
    const rules = [
      ...strings(profile.principles),
      ...strings(profile.likes),
      ...strings(profile.dislikes),
      ...strings(profile.decision_rules),
    ];
    const bindings = Array.isArray(draft.evidence_bindings) ? draft.evidence_bindings : [];
    const bindingMap = new Map(bindings.map((binding) => [text(binding?.rule), binding]));
    for (const rule of rules) {
      const binding = object(bindingMap.get(rule));
      const sourceIds = strings(binding.source_ids);
      if (sourceIds.length === 0 || sourceIds.some((sourceId) => !known.has(sourceId))) {
        findings.push({ code: "taste_profile.binding.invalid", message: `rule is not bound to admitted evidence: ${rule}` });
      }
      if (!["explicit", "observed", "inferred"].includes(text(binding.confidence))) {
        findings.push({ code: "taste_profile.confidence.invalid", message: `binding confidence is invalid: ${rule}` });
      }
    }
    for (const binding of bindings) {
      if (strings(binding?.source_ids).some((sourceId) => !known.has(sourceId))) {
        findings.push({ code: "taste_profile.binding.unknown", message: "binding cites unknown evidence" });
      }
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
      taste_profile: ready ? object(draft.taste_profile) : {
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
        bindings: ready && Array.isArray(draft.evidence_bindings) ? draft.evidence_bindings : [],
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
