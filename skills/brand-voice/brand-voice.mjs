export function indexSources(inputs) {
  const brand = text(inputs.brand);
  const supplied = Array.isArray(inputs.source_material) ? inputs.source_material : [];
  const blockers = [];
  const sources = [];
  const allowedKinds = new Set(["approved", "rejected", "draft", "operator_note"]);

  if (!brand) blockers.push("brand is missing");
  if (supplied.length === 0) blockers.push("source_material is empty");
  if (supplied.length > 50) blockers.push("source_material exceeds 50 items");

  for (const [index, raw] of supplied.slice(0, 50).entries()) {
    const item = object(raw);
    const kind = text(item.kind);
    const content = text(item.content);
    if (!allowedKinds.has(kind)) {
      blockers.push(`source_material[${index}].kind is invalid`);
      continue;
    }
    if (!content || content.length > 20_000) {
      blockers.push(`source_material[${index}].content is missing or too large`);
      continue;
    }
    sources.push({
      source_id: `source:${index + 1}`,
      kind,
      label: text(item.label),
      content,
    });
  }

  if (!sources.some((source) => source.kind === "approved")) {
    blockers.push("at least one approved source is required");
  }
  return {
    source_index: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      brand,
      sources,
      blockers,
    },
  };
}

export function finalizeVoice(inputs) {
  const brand = text(inputs.brand);
  const channel = text(inputs.channel);
  const audience = text(inputs.audience);
  const index = object(inputs.source_index);
  const draft = object(inputs.brand_voice_draft);
  const sources = Array.isArray(index.sources) ? index.sources : [];
  const sourcesById = new Map(sources.map((source) => [text(source.source_id), source]));
  const evidenceDigest = text(inputs.evidence_digest);
  const findings = strings(index.blockers).map((message) => ({
    code: "brand_voice.source.invalid",
    message,
  }));

  if (index.decision === "ready") {
    if (text(draft.decision) !== "ready") {
      findings.push({ code: "brand_voice.draft.not_ready", message: "draft decision is not ready" });
    }
    if (text(draft.brand) !== brand) {
      findings.push({ code: "brand_voice.brand.changed", message: "draft brand does not match input" });
    }
    const voice = object(draft.brand_voice);
    const principles = strings(voice.voice_principles);
    const safeClaims = strings(object(voice.claim_rules).safe);
    const bindings = Array.isArray(draft.evidence_bindings) ? draft.evidence_bindings : [];
    if (!isDigest(evidenceDigest)) {
      findings.push({ code: "brand_voice.evidence.unbound", message: "native evidence digest is missing" });
    }
    const bindingMap = new Map(bindings.map((binding) => [text(binding?.rule), strings(binding?.source_ids)]));
    for (const rule of [...principles, ...safeClaims]) {
      const sourceIds = bindingMap.get(rule) || [];
      if (sourceIds.length === 0 || sourceIds.some((sourceId) => !sourcesById.has(sourceId))) {
        findings.push({ code: "brand_voice.binding.invalid", message: `rule is not bound to admitted evidence: ${rule}` });
      }
      if (safeClaims.includes(rule) && !sourceIds.some((sourceId) => sourcesById.get(sourceId)?.kind === "approved")) {
        findings.push({ code: "brand_voice.safe_claim.unapproved", message: `safe claim lacks approved evidence: ${rule}` });
      }
    }
    for (const binding of bindings) {
      if (strings(binding?.source_ids).some((sourceId) => !sourcesById.has(sourceId))) {
        findings.push({ code: "brand_voice.binding.unknown", message: "binding cites unknown evidence" });
      }
    }
  }

  const ready = index.decision === "ready" && findings.length === 0;
  return {
    brand_voice_packet: {
      decision: ready ? "ready" : "needs_more_evidence",
      brand,
      applicability: ready ? object(draft.applicability) : {
        channels: channel ? [channel] : [],
        audience,
        boundaries: [],
      },
      brand_voice: ready ? object(draft.brand_voice) : {
        voice_principles: [],
        vocabulary: { use: [], avoid: [] },
        cadence: [],
        claim_rules: { safe: [], requires_proof: [], forbidden: [] },
        channel_adjustments: [],
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
