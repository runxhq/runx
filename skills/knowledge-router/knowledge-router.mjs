export default function validateRoute(inputs) {
  const {available_sources: catalog, route_proposal: proposal} = inputs;
  const constraints = inputs.constraints ?? {};
  const sources = indexed(catalog.sources, "source");
  const owners = indexed(catalog.owners, "owner");
  const skills = indexed(catalog.skills, "skill");
  const sourceIds = proposal.route.source_ids;
  const matchIds = proposal.source_matches.map(match => match.source_id);
  const ownerId = proposal.owner_recommendation?.owner_id;
  const nextSkill = proposal.next_skill?.skill;
  const unknownEntries = [];
  for (const [kind, index, ids] of [
    ["source", sources, new Set([...sourceIds, ...matchIds])],
    ["owner", owners, ownerId ? [ownerId] : []],
    ["skill", skills, nextSkill ? [nextSkill] : []],
  ]) {
    for (const id of ids) if (!index.has(id)) unknownEntries.push({kind, id});
  }
  if (unknownEntries.length) {
    return rejected("route contains references outside the supplied catalog", unknownEntries, inputs.constraints);
  }
  if (proposal.verdict === "routed" && sourceIds.length === 0) {
    return rejected("a routed verdict requires at least one validated source reference", [], inputs.constraints);
  }
  if (proposal.verdict === "needs_more_context" && (sourceIds.length || matchIds.length || ownerId || nextSkill)) {
    return rejected("needs_more_context must not claim a source, owner, or follow-up skill", [], inputs.constraints);
  }
  const selectedSources = new Set(sourceIds);
  if (new Set(matchIds).size !== matchIds.length || matchIds.length !== sourceIds.length ||
      matchIds.some(id => !selectedSources.has(id))) {
    return rejected("source_matches must name each selected route source exactly once", [], inputs.constraints);
  }
  for (const [field, index, selected] of [
    ["allowed_sources", sources, sourceIds],
    ["excluded_sources", sources, sourceIds],
    ["allowed_owners", owners, ownerId ? [ownerId] : []],
    ["allowed_skills", skills, nextSkill ? [nextSkill] : []],
  ]) {
    if (!Object.hasOwn(constraints, field)) continue;
    const ids = new Set(constraints[field]);
    for (const id of ids) {
      if (!index.has(id)) return rejected("selection limits must reference entries in the supplied catalog", [], inputs.constraints);
    }
    for (const id of selected) {
      if (field === "excluded_sources" ? ids.has(id) : !ids.has(id)) {
        return rejected("route violates " + field, [], inputs.constraints);
      }
    }
  }
  if (ownerId && !owners.get(ownerId).domains.includes(proposal.route.domain)) {
    return rejected("recommended owner does not declare the selected domain", [], inputs.constraints);
  }
  return {knowledge_route: {
    schema: "runx/knowledge-router.route.v1",
    ...proposal,
    validation: {valid: true, catalog_digest: inputs.catalog_digest,
      unknown_entries: [], constraints_applied: inputs.constraints ?? null},
  }};
}

function indexed(entries, label) {
  const map = new Map();
  for (const entry of entries) {
    if (map.has(entry.id)) throw new Error("duplicate " + label + " id: " + entry.id);
    map.set(entry.id, entry);
  }
  return map;
}

function rejected(reason, findings, constraints) {
  return {knowledge_route: {
    schema: "runx/knowledge-router.route.v1",
    verdict: "refused",
    route: {domain: null, rationale: reason, source_ids: []},
    source_matches: [], owner_recommendation: null, next_skill: null,
    validation: {valid: false, catalog_digest: null, unknown_entries: findings,
      constraints_applied: constraints ?? null},
  }};
}
