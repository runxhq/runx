// The catalog collection each selection limit names entries of.
const LIMITS = {allowed_sources: "sources", excluded_sources: "sources",
  allowed_owners: "owners", allowed_skills: "skills"};

export default function validateRoute(inputs) {
  const catalog = indexes(inputs.available_sources);
  return refusedLimits(catalog, inputs) ?? checkedRoute(catalog, inputs);
}

// The refusal that selection limits naming an entry outside the catalog earn,
// or null. Such limits are the caller's, so no proposal can satisfy them.
export function limitsRefusal(inputs) {
  return refusedLimits(indexes(inputs.available_sources), inputs);
}

function refusedLimits(catalog, inputs) {
  const constraints = inputs.constraints ?? {};
  for (const [field, kind] of Object.entries(LIMITS)) {
    if (!Object.hasOwn(constraints, field)) continue;
    const outside = constraints[field].find(id => !catalog[kind].has(id));
    if (outside !== undefined) {
      return rejected("selection limits must reference entries in the supplied catalog: " + field + " names " +
        outside + ", which the catalog doesn't supply", [], inputs.constraints);
    }
  }
  return null;
}

function checkedRoute({sources, owners, skills}, inputs) {
  const {route_proposal: proposal} = inputs;
  const constraints = inputs.constraints ?? {};
  const sourceIds = proposal.route.source_ids;
  const matchIds = proposal.source_matches.map(match => match.source_id);
  const ownerId = proposal.owner_recommendation?.owner_id;
  const nextSkill = proposal.next_skill?.skill;
  const unknownEntries = [];
  const unknownIn = [];
  for (const [kind, index, field, ids] of [
    ["source", sources, "route.source_ids", sourceIds],
    ["source", sources, "source_matches", matchIds],
    ["owner", owners, "owner_recommendation", ownerId ? [ownerId] : []],
    ["skill", skills, "next_skill", nextSkill ? [nextSkill] : []],
  ]) {
    const unknown = ids.filter(id => !index.has(id));
    if (unknown.length) unknownIn.push(field);
    for (const id of unknown) {
      if (!unknownEntries.some(entry => entry.kind === kind && entry.id === id)) unknownEntries.push({kind, id});
    }
  }
  if (unknownEntries.length) {
    return rejected("route contains references outside the supplied catalog, in " + listed(unknownIn),
      unknownEntries, inputs.constraints);
  }
  if (proposal.verdict === "routed" && sourceIds.length === 0) {
    return rejected("a routed verdict requires at least one validated source reference, and route.source_ids is empty",
      [], inputs.constraints);
  }
  if (proposal.verdict === "needs_more_context") {
    const toEmpty = [];
    if (sourceIds.length) toEmpty.push("route.source_ids");
    if (matchIds.length) toEmpty.push("source_matches");
    const toNull = [];
    if (ownerId) toNull.push("owner_recommendation");
    if (nextSkill) toNull.push("next_skill");
    const claims = [toEmpty.length ? listed(toEmpty) + " must be empty" : null,
      toNull.length ? listed(toNull) + " must be null" : null].filter(Boolean);
    if (claims.length) {
      return rejected("needs_more_context must not claim a source, owner, or follow-up skill: " + claims.join(", and "),
        [], inputs.constraints);
    }
  }
  const selectedSources = new Set(sourceIds);
  const repeated = matchIds.find((id, index) => matchIds.indexOf(id) !== index);
  const unselected = matchIds.find(id => !selectedSources.has(id));
  const unmatched = sourceIds.find(id => !matchIds.includes(id));
  const mismatch = repeated !== undefined ? "it names " + repeated + " twice"
    : unselected !== undefined ? "it names " + unselected + ", which route.source_ids doesn't select"
      : unmatched !== undefined ? "it has no match for " + unmatched : null;
  if (mismatch) {
    return rejected("source_matches must name each selected route source exactly once: " + mismatch,
      [], inputs.constraints);
  }
  const selected = {sources: sourceIds, owners: ownerId ? [ownerId] : [], skills: nextSkill ? [nextSkill] : []};
  const selecting = {sources: "route.source_ids selects", owners: "owner_recommendation names",
    skills: "next_skill names"};
  for (const [field, kind] of Object.entries(LIMITS)) {
    if (!Object.hasOwn(constraints, field)) continue;
    const ids = new Set(constraints[field]);
    const excluded = field === "excluded_sources";
    const offending = selected[kind].find(id => excluded ? ids.has(id) : !ids.has(id));
    if (offending !== undefined) {
      return rejected("route violates " + field + ": " + selecting[kind] + " " + offending + ", which it " +
        (excluded ? "excludes" : "doesn't allow"), [], inputs.constraints);
    }
  }
  if (ownerId && !owners.get(ownerId).domains.includes(proposal.route.domain)) {
    return rejected("recommended owner does not declare the selected domain: route.domain is " +
      proposal.route.domain + ", which " + ownerId + " doesn't declare", [], inputs.constraints);
  }
  return {knowledge_route: {
    schema: "runx/knowledge-router.route.v1",
    ...proposal,
    validation: {valid: true, catalog_digest: inputs.catalog_digest,
      unknown_entries: [], constraints_applied: inputs.constraints ?? null},
  }};
}

// Fields in prose: "a", "a and b", "a, b and c".
function listed(fields) {
  return fields.length < 2 ? fields.join("") : fields.slice(0, -1).join(", ") + " and " + fields.at(-1);
}

function indexes(catalog) {
  return {sources: indexed(catalog.sources, "source"), owners: indexed(catalog.owners, "owner"),
    skills: indexed(catalog.skills, "skill")};
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
