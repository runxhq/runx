import assert from "node:assert/strict";
import test from "node:test";
import { normalizeSearch, validateQuery } from "./web-search.mjs";

// The worker's Runx.parseUrl, which WHATWG URL parsing in Node matches.
globalThis.Runx = {
  parseUrl(value) {
    const url = new URL(value);
    return {
      href: url.href, origin: url.origin, protocol: url.protocol, hostname: url.hostname,
      username: url.username, password: url.password,
    };
  },
};

const normalize = (results, extra = {}, limit = 8) => normalizeSearch({
  query: "Example public API documentation", limit,
  provider_operation: { result: { search: { results, ...extra } } },
}).search_result;

test("preserves source identity, removes exact duplicates and reports cost without claiming page retrieval", () => {
  const candidate = { url: "https://example.com/docs?version=1", title: "Example documentation" };
  const result = normalize([candidate, candidate, { url: "javascript:alert(1)", title: "bad" }, { url: "https://user:secret@example.com/", title: "bad" }], { requestId: "request-1", costDollars: { total: 0.005 } });
  assert.equal(result.decision, "sources_found");
  assert.equal(result.sources[0].url, candidate.url);
  assert.equal(result.sources[0].content_status, "not_fetched");
  assert.equal(result.duplicate_count, 1);
  assert.equal(result.unusable_count, 2);
  assert.equal(result.reported_cost_usd, 0.005);
});

test("distinguishes empty results from unusable results and unknown cost", () => {
  assert.equal(normalize([]).decision, "no_results");
  const result = normalize([{ url: "not a URL", title: "Example" }]);
  assert.equal(result.decision, "unusable_results");
  assert.equal(result.reported_cost_usd, null);
  // An authority that does not parse is not a source.
  const malformed = normalize([{ url: "https://example.com:bad/path", title: "Example" }]);
  assert.equal(malformed.decision, "unusable_results");
  assert.equal(malformed.unusable_count, 1);
});

test("refuses a provider response exceeding the admitted bound", () => {
  assert.throws(() => normalize([{ url: "https://example.com/a", title: "A" }, { url: "https://example.com/b", title: "B" }], {}, 1), /exceeded/);
});

test("refuses reversed publication bounds before the search stage", () => {
  assert.throws(() => validateQuery({ start_published_at: "2026-10-10T00:00:00Z", end_published_at: "2026-10-09T00:00:00Z" }), /no search was dispatched/);
});
