# PostHog

Read saved insights from one selected PostHog Cloud region and project. Supply
`api_origin` and `project_id` when binding the connector, and a personal API key
with `insight:read` access through native credential custody. An event-capture
project key does not grant management access. US and EU accounts do not fall
back to each other. Self-hosted deployments are not implemented by this manifest.

`insights.list` reads one page with an explicit limit and offset, with optional
`search` and `short_id` filters. The page size bounds each request, not the total
number of saved insights. It preserves
the provider's `count`, `next`, `previous` and `results`; it does not follow
provider-supplied URLs. `insight.read` accepts a numeric primary key or the
`short_id` shown in an insight URL. Both request cached results with `refresh=force_cache`. Preserve
`query`, `last_refresh`, `result` and any incomplete-result flags: an empty or
old cache is not a fresh report and must not be presented as one.

This connector does not execute SQL, refresh reports, create dashboards or
capture events. Saved trend, funnel and retention definitions can supply
bounded evidence to `product-analytics`, once the caller has established their
event, identity, cohort, window and denominator definitions. Do not pass person
identifiers or arbitrary event properties to a model when aggregate evidence
is sufficient.

Provider contract: [Insights API](https://posthog.com/docs/api/insights).
