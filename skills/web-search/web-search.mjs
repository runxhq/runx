export function validateQuery({ start_published_at, end_published_at }) {
  if (start_published_at && end_published_at && Date.parse(start_published_at) > Date.parse(end_published_at)) {
    throw new Error("publication date bounds are reversed; no search was dispatched");
  }
  return { valid: true };
}

export function normalizeSearch({ query, limit, provider_operation }) {
  const search = provider_operation.result.search;
  if (!search || !Array.isArray(search.results)) throw new Error("search response has no result list");
  if (search.results.length > limit) throw new Error("provider exceeded the admitted result limit");
  const sources = [];
  const seen = new Set();
  let duplicateCount = 0;
  let unusableCount = 0;
  for (const candidate of search.results) {
    const url = candidate?.url;
    const title = candidate?.title;
    // Do not repair a malformed identity into a different source.
    if (typeof url !== "string" || url.length > 8000 || typeof title !== "string" || !title.trim() || title.length > 2000) {
      unusableCount++;
      continue;
    }
    // Only an absolute http(s) URL, as runx parses it, without credentials.
    // The source keeps the exact string the provider returned; web-fetch owns
    // network admission.
    const parsed = parsedUrl(url);
    if (!parsed || !["http:", "https:"].includes(parsed.protocol)
        || parsed.username || parsed.password || /\s/u.test(url)) {
      unusableCount++;
      continue;
    }
    if (seen.has(url)) { duplicateCount++; continue; }
    seen.add(url);
    sources.push({
      url,
      title: title.trim(),
      published_at: typeof candidate.publishedDate === "string" ? candidate.publishedDate : null,
      excerpt: typeof candidate.text === "string" ? candidate.text.slice(0, 4000) : null,
      content_status: "not_fetched",
    });
  }
  return { search_result: {
    decision: sources.length ? "sources_found" : search.results.length ? "unusable_results" : "no_results",
    query,
    request_id: typeof search.requestId === "string" ? search.requestId : null,
    sources,
    returned_count: search.results.length,
    duplicate_count: duplicateCount,
    unusable_count: unusableCount,
    reported_cost_usd: typeof search.costDollars?.total === "number" && Number.isFinite(search.costDollars.total) && search.costDollars.total >= 0
      ? search.costDollars.total : null,
    coverage: "one_query",
  } };
}

function parsedUrl(value) {
  try {
    return Runx.parseUrl(value);
  } catch {
    return null;
  }
}
