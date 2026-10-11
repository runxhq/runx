export function loadProjections(inputs) {
  const explicit = Array.isArray(inputs.explicit_reflect_projections)
    ? inputs.explicit_reflect_projections
    : undefined;
  const skillFilter = stringValue(inputs.skill_filter) ?? undefined;
  const sinceMs = stringValue(inputs.since) ? Date.parse(inputs.since) : undefined;
  const storedRows = Array.isArray(inputs.stored_reflect_projections?.rows)
    ? inputs.stored_reflect_projections.rows
    : [];
  const projections = explicit ?? storedRows.map((row) =>
    row?.event?.projection ?? row?.event?.payload?.projection ?? row?.event,
  ).filter(Boolean);
  return {
    reflect_projections_packet: {
      items: projections
        .filter((entry) => entry && entry.entry_kind === "projection" && entry.scope === "reflect")
        .filter((entry) => !skillFilter || entry?.value?.skill_ref === skillFilter)
        .filter((entry) => {
          if (sinceMs === undefined) return true;
          const createdAt = typeof entry.created_at === "string" ? Date.parse(entry.created_at) : NaN;
          return Number.isFinite(createdAt) && createdAt >= sinceMs;
        }),
    },
  };
}

export function groupProjections(inputs) {
  const projections = Array.isArray(inputs.reflect_projections_packet?.items)
    ? inputs.reflect_projections_packet.items
    : [];
  const parsedSupport = Number(inputs.min_support);
  const parsedConfidence = Number(inputs.min_confidence);
  const minSupport = Number.isFinite(parsedSupport) ? parsedSupport : 2;
  const minConfidence = Number.isFinite(parsedConfidence) ? parsedConfidence : 0.5;
  const grouped = new Map();
  for (const entry of projections) {
    if (!entry || entry.entry_kind !== "projection" || entry.scope !== "reflect") continue;
    if (typeof entry.confidence !== "number" || entry.confidence < minConfidence) continue;
    const projection = entry.value;
    const skillRef = projection && typeof projection === "object" ? stringValue(projection.skill_ref) : null;
    if (!skillRef) continue;
    const current = grouped.get(skillRef) ?? { skill_ref: skillRef, support: 0, supporting_receipt_ids: [], projections: [] };
    current.support += 1;
    if (typeof entry.receipt_id === "string") current.supporting_receipt_ids.push(entry.receipt_id);
    current.projections.push(entry);
    grouped.set(skillRef, current);
  }
  return {
    grouped_reflections_packet: {
      items: Array.from(grouped.values())
        .filter((group) => group.support >= minSupport)
        .map((group) => ({ ...group, supporting_receipt_ids: Array.from(new Set(group.supporting_receipt_ids)) }))
        .sort((left, right) => right.support - left.support || left.skill_ref.localeCompare(right.skill_ref)),
    },
  };
}

export function buildHandoffs(inputs) {
  const groups = Array.isArray(inputs.grouped_reflections) ? inputs.grouped_reflections : [];
  const proposals = Array.isArray(inputs.proposals) ? inputs.proposals : [];
  if (proposals.length > 20) throw new Error("reflect digest may emit at most 20 proposals");
  const groupsBySkill = new Map(groups.map((group) => [stringValue(group?.skill_ref), group]));
  const seen = new Set();
  const normalized = proposals.map((proposal) => {
    if (!proposal || typeof proposal !== "object" || Array.isArray(proposal)) throw new Error("proposals must contain objects");
    const skillRef = requiredString(proposal.skill_ref, "proposal.skill_ref");
    if (seen.has(skillRef)) throw new Error(`duplicate proposal for ${skillRef}`);
    seen.add(skillRef);
    const group = groupsBySkill.get(skillRef);
    if (!group) throw new Error(`proposal has no grouped reflection evidence: ${skillRef}`);
    const receiptIds = stringArray(proposal.supporting_receipt_ids, `${skillRef}.supporting_receipt_ids`);
    const admittedIds = new Set(Array.isArray(group.supporting_receipt_ids) ? group.supporting_receipt_ids.map(String) : []);
    if (receiptIds.some((receiptId) => !admittedIds.has(receiptId))) {
      throw new Error(`${skillRef} proposal cites a receipt outside its grouped evidence`);
    }
    return {
      skill_ref: skillRef,
      target_dir: normalizeTarget(proposal.target_dir),
      objective: requiredString(proposal.objective, `${skillRef}.objective`),
      evidence_summary: requiredString(proposal.evidence_summary, `${skillRef}.evidence_summary`),
      supporting_receipt_ids: receiptIds,
      boundaries: Array.isArray(proposal.boundaries) ? proposal.boundaries.map(String).filter(Boolean).slice(0, 20) : [],
    };
  });
  return {
    proposals: normalized,
    skill_build_handoffs: normalized.map((proposal) => ({
      skill: "skill-build",
      runner: "improve",
      target_skill_ref: proposal.skill_ref,
      supporting_receipt_ids: proposal.supporting_receipt_ids,
      inputs: {
        objective: proposal.objective,
        target_dir: proposal.target_dir,
        receipt_id: proposal.supporting_receipt_ids[0],
        receipt_summary: `${proposal.evidence_summary} Supporting receipts: ${proposal.supporting_receipt_ids.join(", ")}.`,
      },
      boundaries: proposal.boundaries,
    })),
  };
}

function normalizeTarget(value) {
  const target = requiredString(value, "proposal.target_dir");
  const segments = target.split("/");
  if (
    target.startsWith("/")
    || target.includes("\\")
    || segments.some((segment) => !segment || segment === "." || segment === "..")
  ) throw new Error("proposal.target_dir must be a canonical repo-relative POSIX path");
  return segments.join("/");
}

function stringArray(value, field) {
  if (!Array.isArray(value) || value.length === 0) throw new Error(`${field} must be a non-empty array`);
  return [...new Set(value.map((entry) => requiredString(entry, field)))];
}

function requiredString(value, field) {
  const result = stringValue(value);
  if (!result) throw new Error(`${field} must be a non-empty string`);
  return result;
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}
