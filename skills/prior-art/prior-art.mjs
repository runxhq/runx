const MAX_FILES = 16;
const MAX_FILE_BYTES = 256 * 1024;
const MAX_TOTAL_BYTES = 1024 * 1024;

export function planSources(inputs) {
  const requested = new Set(strings(inputs.source_paths).map(normalizeRelative));
  const decomposition = record(inputs.decomposition);
  for (const skill of records(decomposition.required_skills)) {
    if (skill.exists === true) addSkillManifest(requested, skill.name);
  }
  for (const step of records(decomposition.orchestration_steps)) addSkillManifest(requested, step.skill);
  const paths = [...requested].sort();
  if (paths.length > MAX_FILES) throw new Error(`source_paths may resolve to at most ${MAX_FILES} files`);
  return {
    paths,
    limits: { max_files: MAX_FILES, max_file_bytes: MAX_FILE_BYTES, max_total_bytes: MAX_TOTAL_BYTES },
  };
}

export function indexSources(inputs) {
  const limits = requiredRecord(inputs.limits, "limits");
  const authoring = requiredRecord(inputs.authoring_context, "authoring_context");
  const bundle = record(inputs.file_read_bundle);
  const catalog = records(authoring.catalog_skills).map((entry) => ({
    name: stringValue(entry.name),
    path: stringValue(entry.path),
    kind: "skill-manifest",
    status: stringValue(entry.status) || "unknown",
  })).filter((entry) => entry.name && entry.path);
  const inspectedSources = records(bundle.files).map((file) => ({
    path: requiredString(file.path, "file_read_bundle.files.path"),
    kind: String(file.path).endsWith("/run.yaml") ? "skill-manifest" : "repo-document",
    bytes: Number.isFinite(file.bytes) ? file.bytes : 0,
    digest: requiredString(file.content_digest, "file_read_bundle.files.content_digest"),
  }));
  const totalBytes = inspectedSources.reduce((total, source) => total + source.bytes, 0);
  const missingSources = records(bundle.missing).map((entry) => ({
    path: requiredString(entry.path, "file_read_bundle.missing.path"),
    reason: stringValue(entry.reason) || "source could not be inspected",
  }));
  if (totalBytes > MAX_TOTAL_BYTES) throw new Error(`inspected sources exceed ${MAX_TOTAL_BYTES} bytes`);
  return {
    evidence_index: {
      schema: "runx/prior-art.evidence_index.v1",
      workspace: "local",
      catalog,
      inspected_sources: inspectedSources,
      missing_sources: missingSources,
      limits,
    },
  };
}

export function validatePriorArt(inputs) {
const index = requiredRecord(inputs.evidence_index, "evidence_index");
const draft = requiredRecord(inputs.prior_art_draft, "prior_art_draft");
const requestedDecision = enumValue(draft.decision, ["ready", "needs_more_evidence"], "decision");
const catalog = records(index.catalog);
const inspected = records(index.inspected_sources);
const missing = records(index.missing_sources);
const allowedPaths = new Set([...catalog, ...inspected].map((entry) => stringValue(entry.path)).filter(Boolean));
const catalogNames = new Set(catalog.map((entry) => stringValue(entry.name)).filter(Boolean));
const validationFindings = missing.map((entry) => ({
  code: "source.missing",
  path: stringValue(entry.path) || "unknown",
  message: "A requested source was not available to the deterministic inspection step.",
}));

const findings = records(draft.findings).map((entry, position) => {
  const source = requiredString(entry.source, `findings[${position}].source`);
  const confidence = enumValue(entry.confidence, ["verified", "likely", "unverified"], `findings[${position}].confidence`);
  if (confidence === "verified" && !allowedPaths.has(source)) {
    validationFindings.push({ code: "finding.unbound_source", path: source, message: "A verified finding must cite an inspected source." });
  }
  return {
    claim: requiredString(entry.claim, `findings[${position}].claim`),
    source,
    relevance: requiredString(entry.relevance, `findings[${position}].relevance`),
    confidence,
  };
});

const catalogFit = requiredRecord(draft.catalog_fit, "catalog_fit");
const adjacentSkills = strings(catalogFit.adjacent_skills);
for (const skill of adjacentSkills) {
  if (!catalogNames.has(skill)) validationFindings.push({ code: "catalog.unknown_skill", path: skill, message: "Adjacent skill is absent from the inspected catalog." });
}
const sources = records(draft.sources).map((entry, position) => {
  const sourcePath = requiredString(entry.path, `sources[${position}].path`);
  if (!allowedPaths.has(sourcePath)) validationFindings.push({ code: "source.unbound", path: sourcePath, message: "Source is absent from the inspection index." });
  return { path: sourcePath, kind: stringValue(entry.kind) || "repo-document" };
});

const decision = requestedDecision === "ready" && validationFindings.length === 0 ? "ready" : "needs_more_evidence";
return {
  decision,
  findings,
  catalog_fit: {
    decision: enumValue(catalogFit.decision, ["reuse", "amend", "new_work", "stop"], "catalog_fit.decision"),
    adjacent_skills: adjacentSkills,
    rationale: requiredString(catalogFit.rationale, "catalog_fit.rationale"),
  },
  quality_bar: record(draft.quality_bar),
  recommended_flow: records(draft.recommended_flow),
  sources,
  risks: records(draft.risks),
  evidence: {
    cited_paths: [...new Set([...findings.map((entry) => entry.source), ...sources.map((entry) => entry.path)])].sort(),
    inspected_sources: inspected,
    catalog_entries: adjacentSkills.map((name) => catalog.find((entry) => entry.name === name)).filter(Boolean),
  },
  validation: {
    status: decision === "ready" ? "pass" : "hold",
    findings: validationFindings,
  },
};
}

function addSkillManifest(target, value) {
  const name = stringValue(value)?.replace(/^\.\.\//u, "");
  if (name && /^[a-z0-9][a-z0-9-]*$/u.test(name)) target.add(`skills/${name}/run.yaml`);
}

function normalizeRelative(value) {
  const normalized = String(value).replaceAll("\\", "/").split("/").reduce((parts, part) => {
    if (!part || part === ".") return parts;
    if (part === "..") throw new Error(`source path must stay inside the workspace: ${value}`);
    parts.push(part);
    return parts;
  }, []).join("/");
  if (!normalized || normalized.startsWith("/")) throw new Error(`source path must stay inside the workspace: ${value}`);
  return normalized;
}

function records(value) {
  return Array.isArray(value) ? value.map(record) : [];
}

function strings(value) {
  return Array.isArray(value) ? [...new Set(value.map(stringValue).filter(Boolean))] : [];
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function requiredRecord(value, field) {
  const parsed = record(value);
  if (Object.keys(parsed).length === 0) throw new Error(`${field} must be a non-empty object`);
  return parsed;
}

function enumValue(value, allowed, field) {
  if (!allowed.includes(value)) throw new Error(`${field} must be one of ${allowed.join(", ")}`);
  return value;
}

function requiredString(value, field) {
  const parsed = stringValue(value);
  if (!parsed) throw new Error(`${field} must be a non-empty string`);
  return parsed;
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}
