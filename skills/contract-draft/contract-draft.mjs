const PLACEHOLDER = /\{\{([^{}]+)\}\}/gu;

export function admitRequest({template, parties, terms}) {
  const findings = [];
  const missingTerms = template.required_terms.filter(key =>
    !Object.hasOwn(terms, key) || terms[key] === null || terms[key] === ""
  );
  for (const key of missingTerms) {
    findings.push({code: "terms.missing", message: "required term " + key + " was not supplied."});
  }
  if (new Set(template.clauses.map(clause => clause.id)).size !== template.clauses.length) {
    findings.push({code: "template.duplicate_clause", message: "template clause ids must be unique."});
  }
  const scope = {...parties, terms};
  for (const clause of template.clauses) {
    for (const path of renderBaseline(clause.baseline, scope).unresolved) {
      findings.push({code: "template.unresolved", message: "clause " + clause.id + " has an unresolved or unsupported placeholder: " + path + "."});
    }
  }
  return {draft_context: {path: findings.length ? "stop" : "draft", findings, missing_terms: missingTerms}};
}

export function finalizeDraft(inputs) {
  const {draft_context: context, template, parties, terms, draft_doc: draft} = inputs;
  if (context.path === "stop") {
    return packet({
      decision: "refused",
      reason: "Refused: the supplied template or referenced evidence is incomplete or inconsistent.",
      document: null, deviations: [], send_proposal: null,
      validation: {status: "fail", findings: context.findings},
      template_digest: null, parties_digest: null, terms_digest: null,
    });
  }

  const findings = [];
  const templateById = new Map(template.clauses.map(clause => [clause.id, clause]));
  const draftById = new Map(draft.clauses.map(clause => [clause.id, clause]));
  const declaredById = new Map(draft.deviations.map(deviation => [deviation.clause_id, deviation]));
  const confirmedDeviations = [];
  if (draft.title !== template.title) {
    findings.push({code: "title.changed", message: "draft title must match the versioned template."});
  }
  if (draftById.size !== draft.clauses.length) {
    findings.push({code: "clause.duplicate", message: "draft clause ids must be unique."});
  }
  if (declaredById.size !== draft.deviations.length) {
    findings.push({code: "deviation.duplicate", message: "declare each clause deviation once."});
  }
  for (const clause of draft.clauses) {
    if (!templateById.has(clause.id)) {
      findings.push({code: "clause.unknown", message: "draft contains a clause outside the versioned template: " + clause.id + "."});
    }
  }

  const scope = {...parties, terms};
  for (const templateClause of template.clauses) {
    const {id} = templateClause;
    const clause = draftById.get(id);
    if (!clause) {
      findings.push({code: "clause.missing", message: "draft is missing template clause " + id + "."});
      continue;
    }
    if (clause.heading !== templateClause.heading) {
      findings.push({code: "clause.heading_changed", message: "draft heading differs from the versioned template for clause " + id + "."});
    }
    const {rendered, unresolved} = renderBaseline(templateClause.baseline, scope);
    if (unresolved.length) {
      findings.push({code: "template.unresolved", message: "cannot reconcile clause " + id + " until all referenced evidence is supplied."});
      continue;
    }
    const deviation = declaredById.get(id);
    if (clause.text === rendered) {
      if (deviation) findings.push({code: "deviation.not_real", message: "clause " + id + " declares a deviation but matches the rendered baseline."});
      continue;
    }
    if (!deviation) {
      findings.push({code: "deviation.undeclared", message: "clause " + id + " differs from the rendered baseline without a declared deviation reason."});
      continue;
    }
    if (clause.text.search(PLACEHOLDER) !== -1) {
      findings.push({code: "placeholder.unresolved", message: "clause " + id + " still contains unresolved placeholders."});
      continue;
    }
    confirmedDeviations.push({clause_id: id, reason: deviation.reason, baseline: rendered, text: clause.text});
  }
  for (const id of declaredById.keys()) {
    if (!templateById.has(id)) findings.push({code: "deviation.unknown_clause", message: "declared deviation targets unknown clause " + id + "."});
  }

  const drafted = findings.length === 0;
  return packet({
    decision: drafted ? "drafted" : "refused",
    reason: drafted
      ? "Draft covers every template clause with all required terms bound and " + confirmedDeviations.length + " declared deviation(s)."
      : "Refused: the draft does not deterministically reconcile with the template and declared deviations.",
    document: drafted ? {
      template_id: template.id, template_version: template.version, title: template.title,
      clauses: template.clauses.map(clause => draftById.get(clause.id)),
    } : null,
    deviations: confirmedDeviations,
    send_proposal: drafted ? {gate: "human-approver", delivery_skill: "message-send", sent: false} : null,
    validation: {status: drafted ? "pass" : "fail", findings},
    template_digest: inputs.template_digest, parties_digest: inputs.parties_digest, terms_digest: inputs.terms_digest,
  });
}

function renderBaseline(baseline, scope) {
  const unresolved = [];
  const rendered = baseline.replace(PLACEHOLDER, (placeholder, path) => {
    const value = /^[a-z0-9_]+(?:\.[a-z0-9_]+)*$/u.test(path)
      ? path.split(".").reduce((node, key) => node && typeof node === "object" && Object.hasOwn(node, key) ? node[key] : undefined, scope)
      : undefined;
    if (value === undefined || value === null || (typeof value === "object" &&
        (!Array.isArray(value) || value.some(item => item === null || typeof item === "object")))) {
      unresolved.push(path);
      return placeholder;
    }
    return Array.isArray(value) ? value.join(", ") : String(value);
  });
  return {rendered, unresolved};
}

function packet(body) {
  return {contract_draft: {schema: "runx/contract-draft.draft.v1", ...body, delivery_performed: false}};
}
