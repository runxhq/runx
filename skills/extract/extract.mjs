export function fileContent({ page }) {
  if (page.eof !== true || page.offset !== 0 || typeof page.data !== "string") {
    throw new Error("Extraction requires one complete admitted text document within the one-million-byte bound; partial artifact pages are not a document.");
  }
  return { content: page.data };
}

export function finalize(inputs) {
  const { value, content, citations, validation } = inputs;
  const findings = [];
  if (validation.results.length !== 1 || !validation.valid) findings.push("Extracted value does not satisfy the caller's schema.");
  const leaves = new Set(leafPointers(value));
  const cited = new Set();
  for (const citation of citations) {
    if (!leaves.has(citation.pointer) || !content.includes(citation.quote)) {
      findings.push(`Unbound extraction citation: ${citation.pointer}`);
    } else cited.add(citation.pointer);
  }
  for (const pointer of leaves) if (!cited.has(pointer)) findings.push(`Missing source quotation: ${pointer}`);
  return { structured_extraction_result: {
    schema: "runx.structured_extraction.result.v1",
    decision: findings.length ? "held" : "ready",
    source_ref: inputs.source_ref,
    source_digest: inputs.source_digest,
    schema_digest: validation.schema_digest,
    output_digest: inputs.output_digest,
    value: findings.length ? null : value,
    citations,
    missing_fields: inputs.missing_fields,
    validation: { ...validation, findings },
  }};
}

function leafPointers(value, pointer = "") {
  if (value === null) return [];
  if (typeof value !== "object") return [pointer];
  return Object.entries(value).flatMap(([key, child]) => leafPointers(child, `${pointer}/${key.replaceAll("~", "~0").replaceAll("/", "~1")}`));
}
