const PLACEHOLDER = /\{\{([a-z0-9_.]+)\}\}/gu;

export function admitRequest(inputs) {
  const template = record(inputs.template);
  const parties = record(inputs.parties);
  const terms = record(inputs.terms);
  const findings = [];
  const requiredTerms = uniqueStrings(template.required_terms);
  const clauses = Array.isArray(template.clauses) ? template.clauses.map(record) : [];
  if (!stringValue(template.id) || !stringValue(template.version) || !stringValue(template.title)) {
    findings.push({ code: "template.invalid", message: "template must carry id, version, and title." });
  }
  if (clauses.length === 0 || clauses.some((clause) => !stringValue(clause.id) || !stringValue(clause.heading) || !stringValue(clause.baseline))) {
    findings.push({ code: "template.invalid", message: "template.clauses must each carry id, heading, and baseline." });
  }
  if (Object.keys(parties).length === 0) {
    findings.push({ code: "parties.missing", message: "parties must name every party the template references." });
  }
  const missingTerms = requiredTerms.filter((key) => terms[key] === undefined || terms[key] === null || terms[key] === "");
  for (const key of missingTerms) {
    findings.push({ code: "terms.missing", message: `required term ${key} was not supplied.` });
  }
  return {
    draft_context: {
      path: findings.length === 0 ? "draft" : "stop",
      findings,
      missing_terms: missingTerms,
    },
  };
}

export function finalizeDraft(inputs) {
  const context = record(inputs.draft_context);
  if (context.path === "stop") {
    return packet({
      decision: "refused",
      reason: "Refused: the request is missing required template, party, or term evidence.",
      document: null,
      deviations: [],
      send_proposal: null,
      validation: { status: "fail", findings: Array.isArray(context.findings) ? context.findings : [] },
      template_digest: null,
      parties_digest: null,
      terms_digest: null,
    });
  }

  const template = record(inputs.template);
  const parties = record(inputs.parties);
  const terms = record(inputs.terms);
  const draft = record(record(inputs.draft_doc));
  const declared = (Array.isArray(draft.deviations) ? draft.deviations : []).map(record);
  const findings = [];
  const scope = { ...parties, terms };
  const draftClauses = (Array.isArray(draft.clauses) ? draft.clauses : []).map(record);
  const templateClauses = (Array.isArray(template.clauses) ? template.clauses : []).map(record);
  const draftById = new Map(draftClauses.map((clause) => [stringValue(clause.id), clause]));
  const declaredById = new Map(declared.map((deviation) => [stringValue(deviation.clause_id), deviation]));
  const confirmedDeviations = [];

  for (const templateClause of templateClauses) {
    const id = stringValue(templateClause.id);
    const draftClause = draftById.get(id);
    if (!draftClause) {
      findings.push({ code: "clause.missing", message: `draft is missing template clause ${id}.` });
      continue;
    }
    const text = stringValue(draftClause.text) ?? "";
    const { rendered, unresolved } = renderBaseline(templateClause.baseline, scope);
    if (text === rendered && unresolved.length === 0) {
      if (declaredById.has(id)) {
        findings.push({ code: "deviation.not_real", message: `clause ${id} declares a deviation but matches the rendered baseline.` });
      }
      continue;
    }
    const deviation = declaredById.get(id);
    if (!deviation || !stringValue(deviation.reason)) {
      findings.push({ code: "deviation.undeclared", message: `clause ${id} differs from the rendered baseline without a declared deviation reason.` });
      continue;
    }
    if (PLACEHOLDER.test(text)) {
      findings.push({ code: "placeholder.unresolved", message: `clause ${id} still contains unresolved placeholders.` });
      PLACEHOLDER.lastIndex = 0;
      continue;
    }
    confirmedDeviations.push({ clause_id: id, reason: stringValue(deviation.reason), baseline: rendered, text });
  }
  for (const id of declaredById.keys()) {
    if (!templateClauses.some((clause) => stringValue(clause.id) === id)) {
      findings.push({ code: "deviation.unknown_clause", message: `declared deviation targets unknown clause ${id}.` });
    }
  }

  const drafted = findings.length === 0;
  return packet({
    decision: drafted ? "drafted" : "refused",
    reason: drafted
      ? `Draft covers every template clause with all required terms bound and ${confirmedDeviations.length} declared deviation(s).`
      : "Refused: the draft does not deterministically reconcile with the template and declared deviations.",
    document: drafted
      ? {
          template_id: stringValue(template.id),
          template_version: stringValue(template.version),
          title: stringValue(draft.title) ?? stringValue(template.title),
          clauses: templateClauses.map((templateClause) => {
            const clause = draftById.get(stringValue(templateClause.id));
            return { id: stringValue(templateClause.id), heading: stringValue(templateClause.heading), text: stringValue(clause.text) ?? "" };
          }),
        }
      : null,
    deviations: confirmedDeviations,
    send_proposal: drafted
      ? { gate: "human-approver", delivery_skill: "message-send", sent: false }
      : null,
    validation: { status: drafted ? "pass" : "fail", findings },
    template_digest: requiredDigest(inputs.template_digest),
    parties_digest: requiredDigest(inputs.parties_digest),
    terms_digest: requiredDigest(inputs.terms_digest),
  });
}

function renderBaseline(baseline, scope) {
  const unresolved = [];
  const rendered = String(baseline ?? "").replace(PLACEHOLDER, (match, path) => {
    const value = path.split(".").reduce((node, key) => (node && typeof node === "object" ? node[key] : undefined), scope);
    if (value === undefined || value === null || (typeof value === "object" && !Array.isArray(value))) {
      unresolved.push(path);
      return match;
    }
    return Array.isArray(value) ? value.join(", ") : String(value);
  });
  return { rendered, unresolved };
}

function packet(body) {
  return { contract_draft: { schema: "runx.contract_draft.v1", ...body, delivery_performed: false } };
}

function requiredDigest(value) {
  if (typeof value !== "string" || !value.startsWith("sha256:")) {
    throw new Error("native digest evidence is missing");
  }
  return value;
}

function stringValue(value) {
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

function uniqueStrings(value) {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map(stringValue).filter(Boolean))];
}

function record(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}
