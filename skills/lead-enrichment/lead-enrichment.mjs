// Domain normalization only. Native Runx owns input schemas and source digests.
export function enrichFacts({ subject, sources, requested_fields, as_of, max_age_days = 90 }) {
  const asOf = Date.parse(as_of);
  const candidates = new Map();
  const admitted = [];
  const blockers = [];

  for (const source of sources) {
    const age = (asOf - Date.parse(source.observed_at)) / 86_400_000;
    if (!Number.isFinite(age) || age < 0 || age > max_age_days) {
      blockers.push(`${source.source_id}: observation is stale or future-dated`);
      continue;
    }
    if (source.subject_id !== subject.id || source.match_confidence !== "high") {
      blockers.push(`${source.source_id}: intended subject is not confidently matched`);
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
        blockers.push(`${source.source_id}: person facts cannot describe a company subject`);
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
    } else conflicts.push({
      field,
      candidates: [...values.entries()].map(([value, references]) => ({ value, source_ids: [...references] })),
    });
  }

  return {
    lead_enrichment_packet: {
      decision: facts.length === 0 ? "needs_more_evidence"
        : missing.length || conflicts.length || blockers.length ? "partial" : "enriched",
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
