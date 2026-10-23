---
name: google-search-console
description: Read and normalize Google Search Console performance, indexing, site, and sitemap evidence through any compatible Runx connector; plan and govern sitemap submission without coupling the skill to credential custody or one connector implementation.
---

# Google Search Console

Use this skill when an operator needs Search Console evidence that another model or
workflow can act on: which queries and pages are gaining or losing visibility,
whether a URL is indexed, which properties and sitemaps are visible, or whether
an exact sitemap should be submitted.

The skill owns the Search Console procedure and evidence shape. It does not own
OAuth, credential storage, tenant configuration, or connector routing. A local,
self-hosted, third-party, or Runx-hosted connector is equally valid when it
implements the declared provider operations and scopes. Connector identifiers,
credential handles, provider-config keys, service URLs, and tenant ids belong in
the operator's Runx grant and connector configuration, never in this package or
its packets.

## Operating model

Start with the narrowest question.

- Use `performance` for live query/page/search-performance evidence.
- Use `inspect-url` for the current indexed state of one canonical URL.
- Use `sites` or `sitemaps` to discover accessible resources before assuming
  a property or sitemap exists.
- Use `normalize-performance` only when Search Console-shaped results were
  already supplied by a trusted import or connector test. Its packet says
  `provider_status: not_called`; it is not live-provider proof.
- Use `plan-sitemap` and then `submit-sitemap` for an exact, reviewable sitemap
  mutation. Submission requires a matching scoped grant and provider readback.
- Use `plan-indexing` to test whether a request belongs in Google's restricted
  Indexing API lane. Ordinary page-indexing requests are refused.

A useful performance run names the property, date range, search type, data state,
and row bound. It defaults to the `page` dimension; supply another ordered set only
when the decision needs it. Keep dimensions minimal: adding query, page, country,
device, date, hour, or search appearance changes aggregation and can produce a
much larger, privacy-filtered result. Use `hourly_all` only with the `hour`
dimension; those rows are provisional and their timestamps use Google's reported
offset.

HTTP and HTTPS URL-prefix properties are distinct Search Console identities.
Keep the exact scheme returned by Google, especially during migration diagnosis;
do not silently rewrite an HTTP property to HTTPS. Compare like with like and do
not interpret a differently grouped query as a trend.

The performance packet preserves clicks, impressions, CTR, average position,
aggregation type, pagination state, and Google's incomplete-data metadata.
`decision: usable_with_caveats` means the evidence can inform exploration but
must not be treated as a settled period. A packet with `pagination.complete:
false` is a bounded slice, not the whole property. Search Console can omit
low-volume queries and does not guarantee that row totals reproduce every chart
total, so never manufacture missing rows or causal explanations.

URL inspection is a point-in-time observation. A non-indexed result does not by
itself prove a defect or authorize a mutation. Check the canonical URL, robots
state, fetch state, last crawl, sitemap references, and any rich-result verdict
before proposing remediation.

## Sitemap change boundary

`plan-sitemap` validates the property boundary and produces a native
digest-bound plan. It does not call Google. `submit-sitemap` recomputes that
binding, requires an admitted `sitemaps.submit` grant for the exact property and
sitemap URL, sends one idempotent mutation through the configured connector, and
reads the same sitemap back. Only the final submission packet may say `provider_status:
readback_verified`.

A failed, missing, or identity-mismatched readback is not success. Keep the plan
and receipt, inspect connector/provider evidence, and retry only with the same
idempotency key when the intended mutation is unchanged.

Google's Indexing API is not a general "index this URL" API. This skill refuses
ordinary articles, product pages, home pages, and arbitrary URLs. Eligible
JobPosting or BroadcastEvent requests are marked `specialist_required`; this
package deliberately does not execute them.

## Connector capability contract

A compatible connector exposes provider `google-search-console` with these
stable operations and scopes:

- `sites.list` under `sites.read`, projecting `sites`.
- `search.analytics.query` under `search.analytics.read`, projecting the
  property, request fields, rows, counts, aggregation, incomplete-data metadata,
  pagination, and fetch time.
- `url.inspect` under `url.inspect`, projecting the inspected URL, property,
  index verdict fields, inspection link, and fetch time.
- `sitemaps.list` and `sitemaps.get` under `sitemaps.read`.
- `sitemaps.submit` under `sitemaps.submit`, accepting an idempotency key and
  returning the exact property and sitemap identity for readback.

The connector may acquire and refresh credentials however its operator chooses.
It must enforce the Runx grant, project only declared result fields, keep secrets
out of results and receipts, and preserve provider errors rather than converting
them into empty success.

## Handoffs and stop conditions

This is an evidence and bounded-provider skill. Feed its sealed packets to the
operator or to a higher-level organic-growth workflow for comparison, diagnosis,
and prioritized recommendations. Do not jump from a traffic change to an SEO
claim without analytics, deployment, content, and business-context evidence.

Stop when the property is ambiguous, the connector lacks the exact capability,
the date range or dimensions do not match the decision, data is incomplete
beyond the operator's tolerance, a supplied packet lacks provenance, the sitemap
falls outside the property, provider authority is absent, or provider readback cannot bind
the exact resource.

The deterministic module exists only for Search Console domain work that generic
Runx tools cannot express: mapping ordered row keys to named dimensions,
preserving incomplete-data semantics, validating property coverage, and
classifying restricted indexing requests. Runx owns digesting, authority,
provider execution, idempotency, and receipts.
