const MAX_PROVIDER_ROWS = 500;
const MAX_SITE_SOURCES = 20;
const MAX_SITE_SUMMARY = 12000;

export function admitGrowthEvidence(inputs) {
  const objective = text(inputs.objective);
  const siteUrl = text(inputs.site_url);
  const findings = [];
  const blockers = [];
  const caveats = [];
  const sources = [];
  const sourceRecords = [];
  const evidence = [];
  const seenDigests = new Set();

  if (!objective) {
    findings.push(finding("growth.objective.missing", "objective is required"));
  }
  if (!validSite(siteUrl)) {
    findings.push(finding("growth.site_url.invalid", "site_url must be an absolute HTTPS site root"));
  }

  admitSearch(
    object(inputs.search_evidence),
    siteUrl,
    findings,
    caveats,
    sources,
    sourceRecords,
    evidence,
    seenDigests,
  );
  admitAnalytics(
    object(inputs.analytics_evidence),
    findings,
    caveats,
    sources,
    sourceRecords,
    evidence,
    seenDigests,
  );
  admitSite(
    inputs.site_evidence,
    findings,
    caveats,
    sources,
    sourceRecords,
    evidence,
    seenDigests,
  );

  if (sources.length === 0) {
    blockers.push("No valid search, analytics, site, or market evidence was admitted.");
  }
  if (findings.length > 0) {
    blockers.push("One or more supplied evidence records failed admission.");
  }

  const sourceKinds = sources.map((source) => source.kind);
  return {
    growth_context: {
      decision: blockers.length === 0 ? "ready" : "needs_more_evidence",
      objective,
      site_url: siteUrl,
      business_context: object(inputs.business_context),
      constraints: object(inputs.constraints),
      evidence,
      sources,
      source_digests: sources.map((source) => source.source_digest),
      source_records: sourceRecords,
      analysis_scope: {
        source_count: sources.length,
        source_kinds: sourceKinds,
        search_rows_exposed: exposedRows(evidence, "search_performance"),
        analytics_rows_exposed: exposedRows(evidence, "analytics_report"),
        max_rows_per_provider_packet: MAX_PROVIDER_ROWS,
        complete_source_packets_bound_by_digest: true,
      },
      caveats,
      blockers,
      validation: {
        status: blockers.length === 0 ? "pass" : "fail",
        findings,
      },
    },
  };
}

function admitSearch(packet, siteUrl, findings, caveats, sources, sourceRecords, evidence, seen) {
  if (Object.keys(packet).length === 0) return;
  const local = [];
  if (text(packet.schema) !== "runx/google-search-console.performance_evidence.v1") {
    local.push(finding("growth.search.schema_invalid", "search evidence schema is not supported"));
  }
  if (text(object(packet.validation).status) !== "pass" || text(packet.decision) === "blocked") {
    local.push(finding("growth.search.validation_failed", "search evidence did not pass its domain validation"));
  }
  const sourceDigest = text(packet.evidence_digest);
  if (!isDigest(sourceDigest)) {
    local.push(finding("growth.search.digest_missing", "search evidence requires a SHA-256 evidence_digest"));
  }
  if (siteUrl && text(packet.property) && !propertyCovers(text(packet.property), siteUrl)) {
    local.push(finding("growth.search.site_mismatch", "search property does not cover site_url"));
  }
  if (local.length > 0) {
    findings.push(...local);
    return;
  }
  if (!register(sourceDigest, "search_performance", {
    provider: text(packet.provider),
    provider_status: text(packet.provider_status),
    source_status: text(packet.source_status),
    property: text(packet.property),
    decision: text(packet.decision),
  }, sources, sourceRecords, seen)) return;

  const rows = Array.isArray(packet.rows) ? packet.rows : [];
  const packetCaveats = stringArray(packet.caveats);
  caveats.push(...packetCaveats.map((value) => `Search evidence: ${value}`));
  if (rows.length > MAX_PROVIDER_ROWS) {
    caveats.push(`Search analysis view exposes the first ${MAX_PROVIDER_ROWS} rows; the complete packet remains digest-bound.`);
  }
  if (text(packet.provider_status) === "not_called") {
    caveats.push("Search evidence was supplied rather than read through the current Runx provider boundary.");
  }
  evidence.push({
    kind: "search_performance",
    source_digest: sourceDigest,
    property: text(packet.property),
    request: object(packet.request),
    rows: rows.slice(0, MAX_PROVIDER_ROWS),
    row_count: integer(packet.row_count, rows.length),
    pagination: object(packet.pagination),
    freshness: object(packet.freshness),
    caveats: packetCaveats,
  });
}

function admitAnalytics(packet, findings, caveats, sources, sourceRecords, evidence, seen) {
  if (Object.keys(packet).length === 0) return;
  const local = [];
  if (text(packet.schema) !== "runx/google-analytics.report_evidence.v1") {
    local.push(finding("growth.analytics.schema_invalid", "analytics evidence schema is not supported"));
  }
  if (text(object(packet.validation).status) !== "pass" || text(packet.decision) === "blocked") {
    local.push(finding("growth.analytics.validation_failed", "analytics evidence did not pass its domain validation"));
  }
  const sourceDigest = text(packet.evidence_digest);
  if (!isDigest(sourceDigest)) {
    local.push(finding("growth.analytics.digest_missing", "analytics evidence requires a SHA-256 evidence_digest"));
  }
  if (local.length > 0) {
    findings.push(...local);
    return;
  }
  if (!register(sourceDigest, "analytics_report", {
    provider: text(packet.provider),
    provider_status: text(packet.provider_status),
    source_status: text(packet.source_status),
    property: text(packet.property),
    decision: text(packet.decision),
  }, sources, sourceRecords, seen)) return;

  const rows = Array.isArray(packet.rows) ? packet.rows : [];
  const packetCaveats = stringArray(packet.caveats);
  caveats.push(...packetCaveats.map((value) => `Analytics evidence: ${value}`));
  if (rows.length > MAX_PROVIDER_ROWS) {
    caveats.push(`Analytics analysis view exposes the first ${MAX_PROVIDER_ROWS} rows; the complete packet remains digest-bound.`);
  }
  if (text(packet.provider_status) === "not_called") {
    caveats.push("Analytics evidence was supplied rather than read through the current Runx provider boundary.");
  }
  evidence.push({
    kind: "analytics_report",
    source_digest: sourceDigest,
    property: text(packet.property),
    report_type: text(packet.report_type),
    request: object(packet.request),
    rows: rows.slice(0, MAX_PROVIDER_ROWS),
    row_count: integer(packet.row_count, rows.length),
    pagination: object(packet.pagination),
    measurement: object(packet.measurement),
    privacy: object(packet.privacy),
    caveats: packetCaveats,
  });
}

function admitSite(value, findings, caveats, sources, sourceRecords, evidence, seen) {
  const records = Array.isArray(value) ? value : [];
  if (records.length > MAX_SITE_SOURCES) {
    findings.push(finding("growth.site_evidence.too_many", `site_evidence cannot exceed ${MAX_SITE_SOURCES} records`));
  }
  records.slice(0, MAX_SITE_SOURCES).forEach((raw, index) => {
    const record = object(raw);
    const sourceDigest = text(record.source_digest || record.evidence_digest || record.content_digest);
    const sourceUri = text(record.source_uri || record.final_url);
    const kind = text(record.kind || "site_evidence");
    const summary = text(record.summary || record.extracted);
    const local = [];

    if (!isDigest(sourceDigest)) {
      local.push(finding("growth.site_evidence.digest_missing", `site evidence ${index} requires a SHA-256 digest`));
    }
    if (!sourceUri) {
      local.push(finding("growth.site_evidence.uri_missing", `site evidence ${index} requires source_uri or final_url`));
    }
    if (!summary) {
      local.push(finding("growth.site_evidence.summary_missing", `site evidence ${index} requires a bounded summary or extracted value`));
    }
    if (summary.length > MAX_SITE_SUMMARY) {
      local.push(finding("growth.site_evidence.summary_too_large", `site evidence ${index} exceeds ${MAX_SITE_SUMMARY} characters`));
    }
    if (local.length > 0) {
      findings.push(...local);
      return;
    }
    if (!register(sourceDigest, kind, {
      source_uri: sourceUri,
      observed_at: text(record.observed_at),
      decision: text(record.decision),
    }, sources, sourceRecords, seen)) return;

    evidence.push({
      kind,
      source_digest: sourceDigest,
      source_uri: sourceUri,
      observed_at: text(record.observed_at),
      summary,
    });
    const recordCaveats = stringArray(record.caveats);
    caveats.push(...recordCaveats.map((item) => `${kind}: ${item}`));
  });
}

function register(digest, kind, details, sources, records, seen) {
  if (seen.has(digest)) return false;
  seen.add(digest);
  sources.push({ kind, source_digest: digest, ...details });
  records.push({ source_digest: digest });
  return true;
}

function exposedRows(evidence, kind) {
  const entry = evidence.find((item) => item.kind === kind);
  return entry && Array.isArray(entry.rows) ? entry.rows.length : 0;
}

function propertyCovers(property, siteUrl) {
  try {
    const site = Runx.parseUrl(siteUrl);
    if (property.startsWith("sc-domain:")) {
      const domain = property.slice("sc-domain:".length).toLowerCase();
      const host = site.hostname.toLowerCase();
      return host === domain || host.endsWith(`.${domain}`);
    }
    return site.href.startsWith(property) || property.startsWith(site.origin);
  } catch {
    return false;
  }
}

function validSite(value) {
  try {
    const parsed = Runx.parseUrl(value);
    return parsed.protocol === "https:" && Boolean(parsed.hostname);
  } catch {
    return false;
  }
}

function isDigest(value) {
  return /^sha256:[0-9a-f]{64}$/u.test(value);
}

function integer(value, fallback) {
  return Number.isInteger(value) && value >= 0 ? value : fallback;
}

function finding(code, message) {
  return { code, message };
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function text(value) {
  return typeof value === "string" ? value.trim() : "";
}

function stringArray(value) {
  return Array.isArray(value) ? value.map((item) => text(item)).filter(Boolean) : [];
}
