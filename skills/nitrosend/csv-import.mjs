// The CSV import a call asks for: a preview validates the file against its
// reservation without uploading; a live import uploads and imports it.
export function planCsvImport(inputs) {
  const listIds = Array.isArray(inputs.list_ids) ? inputs.list_ids : [];
  const preview = inputs.dry_run !== false;
  return {
    csv_import: {
      mode: preview ? "preview" : "import",
      input: {
        csv_path: inputs.csv_path,
        source_id: inputs.source_id,
        consent_basis: inputs.consent_basis,
        ...(inputs.columns ? { columns: inputs.columns } : {}),
        ...(listIds.length > 0 ? { options: { list_ids: listIds } } : {}),
        dry_run: preview,
        ...(inputs.brand_sid ? { brand_sid: inputs.brand_sid } : {}),
      },
    },
  };
}

// The import tool's evidence, as the operation returned it.
export function presentCsvImport(inputs) {
  const packet = inputs.provider_operation ?? {};
  return { provider_evidence: packet.result ?? null };
}
