// Domain normalization only. Native Runx owns input schemas and source digests.
export function enrichFacts({
  subject,
  sources,
  requested_fields,
  as_of,
  max_age_days = 90,
}) {
  const asOf = Date.parse(as_of);
  const candidates = new Map();
  const admitted = [];
  const blockers = [];

  for (const source of sources) {
    const age = (asOf - Date.parse(source.observed_at)) / 86_400_000;
    if (!Number.isFinite(age) || age < 0 || age > max_age_days) {
      blockers.push(
        `${source.source_id}: observation is stale or future-dated`,
      );
      continue;
    }
    if (
      source.subject_id !== subject.id ||
      source.match_confidence !== "high"
    ) {
      blockers.push(
        `${source.source_id}: intended subject is not confidently matched`,
      );
      continue;
    }
    if (admitted.some((item) => item.source_id === source.source_id)) {
      blockers.push(`${source.source_id}: source reference is duplicated`);
      continue;
    }
    admitted.push({
      source_id: source.source_id,
      source_digest: source.source_digest,
      observed_at: source.observed_at,
      match_confidence: source.match_confidence,
      provenance: "caller_supplied_source_digest",
    });
    for (const fact of source.facts) {
      if (!requested_fields.includes(fact.field)) continue;
      if (subject.kind === "company" && fact.field.startsWith("person.")) {
        blockers.push(
          `${source.source_id}: person facts cannot describe a company subject`,
        );
        continue;
      }
      const values = candidates.get(fact.field) ?? new Map();
      const references = values.get(fact.value) ?? new Set();
      references.add(source.source_id);
      values.set(fact.value, references);
      candidates.set(fact.field, values);
    }
  }

  const facts = [];
  const conflicts = [];
  const missing = [];
  for (const field of requested_fields) {
    const values = candidates.get(field);
    if (!values?.size) missing.push(field);
    else if (values.size === 1) {
      const [value, references] = [...values.entries()][0];
      facts.push({ field, value, source_ids: [...references] });
    } else
      conflicts.push({
        field,
        candidates: [...values.entries()].map(([value, references]) => ({
          value,
          source_ids: [...references],
        })),
      });
  }

  return {
    lead_enrichment_packet: {
      decision:
        facts.length === 0
          ? "needs_more_evidence"
          : missing.length || conflicts.length || blockers.length
            ? "partial"
            : "enriched",
      subject,
      as_of,
      facts,
      missing_fields: missing,
      conflicts,
      sources: admitted,
      blockers,
      acquisition: "supplied_evidence",
    },
  };
}

// Provider-specific entity matching and factual mapping, never transport or retry.
export function prepareLookup({ subject, selector, requested_fields }) {
  if (
    subject.kind === "company" &&
    requested_fields.some((field) => field.startsWith("person."))
  ) {
    throw new Error(
      "company lookup cannot return person facts; select the person first",
    );
  }
  return { lookup_request: selector };
}

export function normalizeLookup({
  subject,
  selector,
  requested_fields,
  observed_at,
  provider_operation: operation,
}) {
  const payload = operation.result.match;
  if (!payload || typeof payload !== "object" || Array.isArray(payload))
    throw new Error("Apollo returned no structured match response");
  const person = subject.kind === "person" ? payload.person : null;
  const company =
    subject.kind === "company" ? payload.organization : person?.organization;
  const record = subject.kind === "person" ? person : company;
  const blockers = [];
  const confidence =
    subject.kind === "person"
      ? payload.match_confidence
      : record
        ? "high"
        : "none";
  let decision = !record
    ? "no_match"
    : subject.kind === "person" && confidence !== "high"
      ? "ambiguous"
      : "matched";
  if (
    record &&
    decision === "matched" &&
    !matchesIdentity(subject.kind, selector, record, company)
  ) {
    decision = "identity_conflict";
    blockers.push("returned identifiers do not establish the selected subject");
  }
  if (decision === "no_match")
    blockers.push(
      "Apollo returned no matched record; a successful HTTP response is not enrichment",
    );
  if (decision === "ambiguous")
    blockers.push(
      "person match is not high confidence; provide a stronger selector before another lookup",
    );
  const email = person?.email;
  const usableEmail =
    typeof email === "string" &&
    /^[^\s@]+@[^\s@]+\.[^\s@]+$/u.test(email) &&
    !/email_not_unlocked|not_unlocked|redacted|masked|\*{2}/iu.test(email);
  const values = {
    "person.name": person?.name,
    "person.email": usableEmail ? email : undefined,
    "person.title": person?.title,
    "person.linkedin_url": person?.linkedin_url,
    "company.name": company?.name,
    "company.domain": company?.primary_domain,
    "company.industry": company?.industry,
    "company.employee_count": company?.estimated_num_employees,
    "company.country": company?.country,
    "company.website": company?.website_url,
  };
  const facts =
    decision === "matched"
      ? requested_fields.flatMap((field) => {
          const value = values[field];
          const valid =
            field === "company.employee_count"
              ? Number.isInteger(value) && value >= 0 && value <= 1000000000
              : typeof value === "string" &&
                value.trim().length > 0 &&
                value.length <= 2000;
          return valid ? [{ field, value }] : [];
        })
      : [];
  const sourceId =
    "apollo:" +
    (subject.kind === "person" ? "person:" : "company:") +
    (record?.id || operation.result_digest);
  const normalized = enrichFacts({
    subject,
    requested_fields,
    as_of: observed_at,
    sources: facts.length
      ? [
          {
            source_id: sourceId,
            source_digest: operation.result_digest,
            subject_id: subject.id,
            observed_at,
            match_confidence: "high",
            facts,
          },
        ]
      : [],
  });
  normalized.lead_enrichment_packet.blockers.push(...blockers);
  return {
    ...normalized,
    lookup_evidence: {
      decision,
      match_confidence: ["high", "medium", "low", "none"].includes(confidence)
        ? confidence
        : "unknown",
      readback_uri: operation.readback_uri,
      result_digest: operation.result_digest,
      observed_at,
      unavailable_fields: normalized.lead_enrichment_packet.missing_fields,
      blockers,
      ...(operation.cost && { cost: operation.cost }),
    },
  };
}

function matchesIdentity(kind, selector, record, company) {
  const equal = (a, b) =>
    typeof a === "string" &&
    typeof b === "string" &&
    a.trim().toLowerCase() === b.trim().toLowerCase();
  const sameUrl = (a, b) =>
    typeof a === "string" &&
    typeof b === "string" &&
    a
      .split(/[?#]/u)[0]
      .replace(/\/$/u, "")
      .replace("://www.", "://")
      .toLowerCase() ===
      b
        .split(/[?#]/u)[0]
        .replace(/\/$/u, "")
        .replace("://www.", "://")
        .toLowerCase();
  if (kind === "company") {
    if (selector.domain && !equal(selector.domain, record.primary_domain))
      return false;
    if (
      selector.linkedin_url &&
      !sameUrl(selector.linkedin_url, record.linkedin_url)
    )
      return false;
    if (selector.website && !sameUrl(selector.website, record.website_url))
      return false;
    return !!(selector.domain || selector.linkedin_url || selector.website);
  }
  if (selector.id && !equal(selector.id, record.id)) return false;
  if (
    selector.linkedin_url &&
    !sameUrl(selector.linkedin_url, record.linkedin_url)
  )
    return false;
  if (
    selector.email &&
    record.email &&
    !/not_unlocked|redacted|masked|\*/iu.test(record.email) &&
    !equal(selector.email, record.email)
  )
    return false;
  if (selector.name && !equal(selector.name, record.name)) return false;
  if (selector.first_name && !equal(selector.first_name, record.first_name))
    return false;
  if (selector.last_name && !equal(selector.last_name, record.last_name))
    return false;
  if (selector.domain && !equal(selector.domain, company?.primary_domain))
    return false;
  if (
    selector.organization_name &&
    !equal(selector.organization_name, company?.name)
  )
    return false;
  return true; // Native request schema requires a strong selector; Apollo must also report high confidence.
}
