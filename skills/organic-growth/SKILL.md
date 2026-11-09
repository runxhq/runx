---
name: organic-growth
description: Turn bounded search, analytics, site, and market evidence into an evidence-bound SEO and answer-engine growth brief with prioritized actions and a measurement plan, without claiming execution or guaranteed visibility.
---

# Organic Growth

Use this skill when an operator needs to decide what to improve on a site for
organic discovery: technical search health, content demand, landing-page
performance, conversion quality, or the clarity and citeability that can help a
page appear in answer engines.

The output is a decision packet, not a bag of SEO tips. It separates what the
evidence shows from what the analyst infers, ranks the next actions by expected
impact, confidence, and effort, and says how each action should be measured. No
action is executed by this skill.

## Direct collection and reuse

The default `review` starts from an objective and the canonical site root. It
collects only missing evidence, then uses the same `analyze` phase as a composed
graph. Any supplied search, analytics or site evidence takes the reuse branch
immediately: no page fetch, measurement query or extra research turn runs.
Forward any prior `collection_notes` with those sources so access failures and
measurement gaps survive the handoff.
Choose `analyze` explicitly for a source-only graph that must never acquire data.

With no sources, the collection turn can use `web-fetch#web-fetch` for relevant
public pages and the read-only defaults `google-search-console#performance` or
`google-analytics#report` on the operator's selected compatible bindings. Google
is optional; no private report is required to inspect a public page. Supply known
property/date/measurement selectors or relevant page URLs in `source_context`.
Ask when a private selector is ambiguous. A missing binding or unavailable
report means unavailable evidence, not zero traffic or no commercial demand.

Collection returns actual reader packets and gaps, not a growth strategy. The
existing analysis owns priorities, causal caution and measurement. For public
page text, pass the unchanged `provider_operation.data.result` from a successful
`web-fetch` with `extract: text` into `site_evidence`; its final URL, full body
digest, extracted text and provenance survive the handoff. Preserve complete
Google normalized evidence packets, including their request and coverage caveats.
Never manufacture a digest, rewrite a report as live, or treat snippets as page
content. Failed reads stay in `collection_notes`; they are not SEO observations.

A long page yields a labelled bounded text view for analysis, while its original
source identity stays bound. Omitted content was not assessed. Focus another
source read only if the decision needs evidence outside that view; do not turn a
presentation ceiling into a claim that the site itself is unsupported.

## Composes

<!-- Generated from the native execution closure; run pnpm core-skills:composes:generate. -->

- `google-analytics#report`
- `google-search-console#performance`
- `web-fetch#web-fetch`

## Evidence comes first

The supplied-evidence `analyze` method accepts a bounded objective, the site, optional
business constraints, and any combination of:

- search-performance evidence, such as a sealed
  `google-search-console` performance packet;
- analytics evidence, such as a sealed `google-analytics` report packet;
- site or market evidence with a SHA-256 source/content digest, source reference,
  kind, and bounded extracted summary.

Google is optional. The canonical workflow depends on evidence contracts, not a
provider, connector, credential store, or hosted tenant. A local crawler,
self-hosted analytics service, another search provider, third-party connector,
or Runx-hosted connector can supply evidence when its packet has a stable digest
and useful provenance. Connector ids, credential handles, URLs, and tenant
configuration never belong in this skill.

The admission step rejects invalid packet schemas, failed validation, missing
digests, and unbounded evidence. It gives the analysis agent the rows, metadata,
caveats, and business context needed for judgment, capped to an explicit analysis
view. The complete source packet remains bound by digest; the model must not
pretend a bounded view is exhaustive.

Every material claim and recommended action must cite one or more admitted source
digests. Runx's native evidence verifier rejects invented references, changed
objective/site identity, empty claims, missing decision fields, and unsupported
external-effect claims. A failure becomes `needs_more_evidence`, not a polished
guess.

## Reading the combined signal

Search evidence explains discovery: queries, pages, impressions, clicks, CTR,
position, freshness, and coverage. Analytics explains behavior and business
outcomes under the property's measurement model. Site evidence explains what is
actually published, crawlable, structured, internally linked, and supportable.

Use their disagreement. Rising impressions with weak CTR can justify inspecting
snippet fit, intent, brand recognition, or SERP composition. Strong clicks with
weak engagement can justify checking landing-page promise, speed, navigation,
or measurement. Analytics decline without a matching Search Console decline can
point toward tagging, channel classification, consent, conversion configuration,
or non-Google traffic. These are hypotheses until the relevant evidence is
collected.

Compare identical date windows, page identities, search types, dimensions,
timezone assumptions, and definitions. Preserve incomplete Search Console data,
GA4 thresholding, sampling, `(other)`-row loss, pagination, and imported-source
status as caveats. Do not convert correlation into causation.

## SEO and answer-engine work

Technical SEO recommendations require site evidence. Check indexability,
canonicalization, robots rules, sitemap membership, status codes, rendering,
structured data eligibility, internal discovery paths, and page duplication
before prescribing a fix. Search Console URL inspection is a point observation,
not authority to mutate the site or request indexing.

Content recommendations should connect a demonstrated audience need to a page,
business outcome, and information gap. Avoid producing pages merely because a
keyword exists. Consolidation, deletion, template repair, internal linking, or a
better primary answer may beat another article.

For answer-engine or "GEO" work, optimize the underlying source quality:
unambiguous entities, direct answers, original evidence, clear authorship and
dates, machine-readable structure, stable URLs, strong internal context, and
claims that another system can verify. There is no reliable switch for AI
citations and no basis for guaranteeing inclusion. Treat answer-engine
visibility as an observed channel that needs its own evidence, not as a mystical
ranking score.

## Action packet

A ready packet contains:

- an executive summary and bounded analysis scope;
- evidence-bound observations and explicit hypotheses;
- prioritized opportunities;
- recommended actions with rationale, source digests, owner type, effort,
  confidence, expected signal, stop condition, and verification method;
- a dedicated answer-engine review where evidence supports it;
- a measurement plan with baselines, leading and business outcomes, comparison
  windows, and rollback or reconsideration triggers;
- risks, missing evidence, and the authoritative source inventory.

The action list remains `external_status: not_executed`. Route implementation
to the owning repository or business workflow. Content changes may move through
a governed content pipeline; code and template work belongs in the site's
repository workflow; publication and provider mutations keep their own approval
and readback gates.

## A useful operating loop

Run a narrow baseline, choose one evidence-bound action, make the change through
its owner, wait for an appropriate observation window, then rerun the same
queries. Preserve the original packet digests. Change several variables together
only when the operator accepts that attribution will be weaker.

Stop when no source is admitted, page/property identity cannot be aligned, the
date windows are incomparable, privacy or pagination caveats defeat the claim,
the requested recommendation needs technical/content evidence that is absent,
the agent cites a source outside the admitted set, or the objective asks for
guaranteed ranking, mass low-value content, deceptive markup, or unapproved
publication.

The deterministic module only admits and bounds organic-growth domain evidence.
It does not hash evidence, call providers, or implement generic verification.
Runx owns native digest provenance, evidence verification, agent receipts, and
all downstream effect gates.

## Agent task: organic-growth-analyze

Return `growth_candidate` with `decision: ready`, the exact objective and
site_url, executive_summary, observations, hypotheses, opportunities,
recommended_actions, geo_review, measurement_plan, claims, risks,
missing_evidence, and `external_status: not_executed`.

Each claim requires a non-empty `claim` and admitted `source_digests`. Every
recommended action requires non-empty `source_digests` plus a rationale,
priority, confidence, effort, expected_signal, verification, and stop_condition.
Mark statements as observed, inferred, or hypothesized. Preserve source caveats,
do not invent metrics or causes, and do not claim a change, publication,
provider call, ranking result, or AI citation occurred.

## Agent task: organic-growth-collect

Acquire relevant missing evidence for the exact objective/site, using the named
read-only skill defaults and native asks if a genuine selector is missing. Do
not submit sitemaps, alter analytics, publish, crawl unrelated sites or invent
missing metrics. Use the returned tool schemas. Google sources are optional.
Read a useful relevant public page if no source was supplied. Fetch explicit
page URLs directly; avoid redundant reads and broad speculative crawling.

Return exactly `search_evidence`, `analytics_evidence`, `site_evidence` and
`collection_notes`. Use empty objects/array for absent sources. Forward actual
normalized Google packet data unchanged. For a text page, forward the unchanged
`provider_operation.data.result` as an individual site-evidence record, never
the provider packet wrapper. Preserve final URL, digest, text and provenance.
Record inaccessible sources, actual read errors and missing measurement context
as notes. Do not perform synthesis; the next analysis turn owns it. No source
means honest missing evidence. Do not claim a provider call you did not make.
