---
name: product-analytics
description: Review saved product funnel and retention insights or supplied aggregate evidence with deterministic rates, explicit measurement definitions and source-bound recommendations; expose instrumentation and comparison gaps without claiming lift.
---

# Product analytics

Explain where a defined product journey loses people or accounts, what the
retention evidence shows, and which measurement or product change to consider.
This skill covers activation and product usage. `organic-growth` owns search and
acquisition strategy; it may consume the relevant findings.

`review` is the default. Give it a product question and, when known, the saved
current and baseline insight ids. It reads the selected PostHog project through
the operator's bound connector. Without ids, a bounded selector searches saved
insight metadata, checks the actual query, and reads only relevant candidates.
An ambiguous choice is a request for context, never permission to invent events
or pick an unrelated chart. List reads use basic metadata rather than cached
result tables. Reads use the cache; they do not force a new query.

Supplying `reports` bypasses collection entirely. `analyze` is the explicit
provider-neutral phase for prepared aggregates from any source. `read-posthog`
reads one exact saved insight; `normalize-posthog` reuses a retained native
provider packet artifact; `from-posthog` accepts current and optional baseline
artifacts and continues analysis. A composed workflow should enter the narrowest
phase for the evidence it already has. No phase captures events, changes
instrumentation, creates tasks or executes arbitrary SQL.

## Measurement admission

Each report carries the definition its own source was computed under: product,
timezone, actual aggregation identity (person, account, configured group or custom unit), cohort, filters, funnel rule and retention
rule. Read it from that report's saved query; never copy one report's definition
onto another. Funnel counts must count the same eligible entities through
ordered steps, not independent event totals. Retention needs an explicit
eligible cohort and return period. Mark incomplete provider results incomplete.
Keep provider source artifacts and exact query semantics. Sampled estimates remain
finite fractional counts; never round them into exact people. Retain observation time; a cached PostHog result is not fresh merely because its
HTTP read happened today.

The pure calculation step preserves counts and computes rates. Zero eligible
entities produce `null`, not a measured 0% conversion or retention rate. Ordered
counts cannot increase and retained entities cannot exceed the eligible cohort.
Compare a baseline only when its definition equals the current definition field
for field, with the same ordered events and retention period over an
equal-duration, nonoverlapping window. Rate changes are percentage points, never
mislabeled relative percentage lift. Invalid or incomplete current measurements withhold recommendations. An invalid
or incompatible baseline withholds the comparison without erasing a valid current
report. Explain `comparison_blockers` and never infer lift across unlike
definitions. Repeated event names are legitimate ordered steps; position
distinguishes each occurrence. Step count is bounded by native turn/byte budgets,
not an arbitrary total journey length.

## Saved-insight semantics and limits

The PostHog reader preserves the actual query scope, chart title and description
in measurement_context, plus the retained source descriptor. The semantic digest
binds those definitions for comparison; it does not replace the analyst's context. It
requires the provider-resolved absolute date window, measurement timezone and
cached computation time. It supports ordered step-count funnels and count-based
retention. Breakdown/comparison series, cumulative retention, missing query
metadata and other insight kinds return a precise source gap with their artifact
retained; use an explicitly selected prepared measurement for those semantics.
Do not flatten different series or silently reinterpret property sums as people.

Retention uses the selected return interval (default 1, the first interval after
cohort entry). Only cohort rows with fully observed return windows are included.
Provider cohort boundaries own calendar/timezone semantics; daily rolling windows
allow the last possible cohort entry its full follow-up. Recent and boundary-
unresolved rows are counted as omitted, never failed retention. Rates are weighted
from summed eligible and returned counts, not averages of percentages. The exact
period and maturity selection travel with the result. Named actions and dynamic
cohorts without historical definition snapshots withhold cross-period comparisons.

A retained artifact proves which bytes were supplied, not independent provider
truth. In a real collected run, the parent receipt also records the provider read.
Test readbacks remain test evidence. Missing credentials or a failed baseline
read must be stated, with usable current measurements retained. Native artifact
reads are bounded; oversized raw results remain behind their source reference for
a source owner to prepare, rather than entering an agent prompt as an archive.

## Interpretation

Use the deterministic rates; do not replace or round them into different facts.
When `comparison` is null, explain its blockers and use only independently valid
current-period observations for recommendations.
Distinguish observations, hypotheses and actions. Concentrate on the consequential
drop-off, plausible instrumentation problems and a small next experiment. A change
between periods does not establish causality or statistical significance. Counts
alone cannot establish sample comparability, seasonality or experiment validity.
Do not average cohort percentages or claim significance without the actual method
and experiment evidence. State those limits when relevant.

Each material finding and proposed action must cite admitted source digests.
Recommend a concrete measurement, owner role and stop/reconsider condition; do
not invent a named owner, deadline, revenue impact or work already completed.
Native evidence verification binds the resulting review to the exact measurement
context. The calling application owns continuing work, scheduling and approved
changes.

## Agent task: product-analytics-analyze

Return `analytics_candidate` with `decision: ready` or `needs_more_evidence`, the exact `objective`, a
concise `summary`, `findings` and `limitations`. Each finding has `claim`, `kind`
(`observed`, `hypothesis` or `action`) and `source_digests`. An action also has
`measurement`, `owner_role` and `stop_condition`. Every finding and action is
verified through the same native claim binding. Cite only admitted digests. Explain zero
denominators and uncertainty plainly. Return `needs_more_evidence` when the
bounded evidence cannot support useful recommendations. Never claim execution.

## Agent task: product-analytics-collect

Return `selection` with `current`, `baseline` and `notes`. Each source is the exact
native ProjectArtifact descriptor from a successful offered `posthog/insight.read`
call, or null when unavailable. Use the provider packet artifact in the call
result, not a new descriptor, model-authored counts, a result digest or a receipt
id. Preserve actual read failures and ambiguity in notes. Respect explicit ids;
otherwise use saved-insight metadata search/paging to select a relevant funnel
or retention query. Do not continue an unbounded catalog scan. Stop and ask for
a more precise chart when context or the admitted turn budget is insufficient.
Do not read a supplied aggregate again or silently refresh a cached result. A
failed optional baseline must not discard an acquired current source.
