export default function validateRoute(inputs) {
const catalog = record(inputs.available_sources);
const proposal = record(inputs.route_proposal);
const sources = indexed(catalog.sources, "source");
const owners = indexed(catalog.owners, "owner");
const skills = indexed(catalog.skills, "skill");
const verdict = String(proposal.verdict || "");

if (!["routed", "needs_more_context", "manual_review"].includes(verdict)) {
  throw new Error("verdict must be routed, needs_more_context, or manual_review");
}

const route = record(proposal.route);
const sourceIds = stringArray(route.source_ids);
const sourceMatches = Array.isArray(proposal.source_matches) ? proposal.source_matches.map(record) : [];
const ownerRecommendation = record(proposal.owner_recommendation);
const nextSkill = record(proposal.next_skill);
const unknownEntries = [];

for (const id of sourceIds) {
  if (!sources.has(id)) unknownEntries.push({ kind: "source", id });
}
for (const match of sourceMatches) {
  const id = stringValue(match.source_id);
  if (!id || !sources.has(id)) unknownEntries.push({ kind: "source", id: id || "<missing>" });
}

const ownerId = stringValue(ownerRecommendation.owner_id);
if (ownerId && !owners.has(ownerId)) unknownEntries.push({ kind: "owner", id: ownerId });
const nextSkillName = stringValue(nextSkill.skill);
if (nextSkillName && !skills.has(nextSkillName)) unknownEntries.push({ kind: "skill", id: nextSkillName });

if (unknownEntries.length > 0) {
  return rejected("route contains references outside the supplied catalog", unknownEntries, inputs.constraints);
}
if (verdict === "routed" && sourceIds.length === 0) {
  return rejected("a routed verdict requires at least one validated source reference", [], inputs.constraints);
}
if (verdict === "needs_more_context" && (sourceIds.length > 0 || ownerId || nextSkillName)) {
  return rejected("needs_more_context must not claim a source, owner, or follow-up skill", [], inputs.constraints);
}

const catalogDigest = stringValue(inputs.catalog_digest);
if (!catalogDigest || !/^sha256:[0-9a-f]{64}$/u.test(catalogDigest)) {
  return rejected("native catalog digest is missing", [], inputs.constraints);
}

return {
  knowledge_route: {
    schema: "runx/knowledge-router.route.v1",
    verdict,
    route: {
      domain: stringValue(route.domain),
      rationale: stringValue(route.rationale),
      source_ids: sourceIds,
    },
    source_matches: sourceMatches.map((match) => ({
      source_id: stringValue(match.source_id),
      matching_signal: stringValue(match.matching_signal),
      reason: stringValue(match.reason),
    })),
    owner_recommendation: ownerId ? {
      owner_id: ownerId,
      rationale: stringValue(ownerRecommendation.rationale),
      escalation_required: ownerRecommendation.escalation_required === true,
    } : null,
    next_skill: nextSkillName ? {
      skill: nextSkillName,
      rationale: stringValue(nextSkill.rationale),
    } : null,
    validation: {
      valid: true,
      catalog_digest: catalogDigest,
      unknown_entries: [],
      constraints_applied: inputs.constraints || null,
    },
  },
};
}

function indexed(value, label) {
  if (!Array.isArray(value)) throw new Error(`available_sources.${label}s must be an array`);
  const map = new Map();
  for (const raw of value) {
    const entry = record(raw);
    const id = stringValue(entry.id);
    if (!id) throw new Error(`every ${label} entry needs an id`);
    if (map.has(id)) throw new Error(`duplicate ${label} id: ${id}`);
    map.set(id, entry);
  }
  return map;
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function stringArray(value) {
  return Array.isArray(value) ? [...new Set(value.map(stringValue).filter(Boolean))] : [];
}

function rejected(reason, findings, constraints) {
  return {
    knowledge_route: {
      schema: "runx/knowledge-router.route.v1",
      verdict: "refused",
      route: {
        domain: null,
        rationale: reason,
        source_ids: [],
      },
      source_matches: [],
      owner_recommendation: null,
      next_skill: null,
      validation: {
        valid: false,
        catalog_digest: null,
        unknown_entries: findings,
        constraints_applied: constraints || null,
      },
    },
  };
}
